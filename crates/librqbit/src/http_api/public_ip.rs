//! Public IP as seen from the server's own network (e.g. the VPN exit when
//! rqbit runs inside a VPN network namespace).
//!
//! A background task asks a couple of "what is my IP" services every
//! [`CHECK_INTERVAL`], separately over IPv4 and IPv6 (the socket is bound to
//! `0.0.0.0` / `::` so the family is forced), falling back between providers
//! with short timeouts. `GET /public_ip?refresh=true` re-checks on demand if
//! the last check is older than [`MIN_REFRESH`].
//!
//! Env:
//! - `RQBIT_PUBLIC_IP_CHECK=false` disables the lookups (they contact
//!   third-party services);
//! - `RQBIT_PUBLIC_IP_URLS_V4` / `RQBIT_PUBLIC_IP_URLS_V6`: comma separated
//!   provider URLs returning the bare address as text.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tracing::debug;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MIN_REFRESH: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

const DEFAULT_V4: &[&str] = &[
    "https://api.ipify.org",
    "https://ipv4.icanhazip.com",
    "https://v4.ident.me",
];
const DEFAULT_V6: &[&str] = &[
    "https://api6.ipify.org",
    "https://ipv6.icanhazip.com",
    "https://v6.ident.me",
];

#[derive(Debug, Clone, Default, Serialize)]
pub struct FamilyResult {
    /// The address, if a provider answered with one of the right family.
    pub ip: Option<String>,
    /// Provider that answered.
    pub source: Option<String>,
    /// Why no address (all providers failed / no route for this family).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PublicIpResponse {
    pub enabled: bool,
    pub ipv4: FamilyResult,
    pub ipv6: FamilyResult,
    /// UTC RFC 3339 time of the last completed check.
    pub checked_at: Option<String>,
    /// Seconds since the last completed check.
    pub age_secs: Option<u64>,
    pub check_interval_secs: u64,
    pub checking: bool,
}

#[derive(Default)]
struct State {
    ipv4: FamilyResult,
    ipv6: FamilyResult,
    checked_at: Option<String>,
    checked_instant: Option<Instant>,
    checking: bool,
}

pub struct PublicIpMonitor {
    enabled: bool,
    v4: Vec<String>,
    v6: Vec<String>,
    state: Mutex<State>,
    /// Serialises checks (background + on-demand).
    check_lock: tokio::sync::Mutex<()>,
}

fn urls_from_env(key: &str, default: &[&str]) -> Vec<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => default.iter().map(|s| s.to_string()).collect(),
    }
}

/// Parses a provider answer; only an address of the wanted family counts.
fn parse_answer(body: &str, want_v6: bool) -> Option<IpAddr> {
    let ip: IpAddr = body.trim().parse().ok()?;
    match (ip, want_v6) {
        (IpAddr::V4(_), false) | (IpAddr::V6(_), true) => Some(ip),
        (IpAddr::V6(v6), false) => v6.to_ipv4_mapped().map(IpAddr::V4),
        _ => None,
    }
}

async fn check_family(urls: &[String], v6: bool) -> FamilyResult {
    let local: IpAddr = if v6 {
        Ipv6Addr::UNSPECIFIED.into()
    } else {
        Ipv4Addr::UNSPECIFIED.into()
    };
    // A plain client: no proxy (the question is this host's own egress).
    let client = match reqwest::Client::builder()
        .local_address(local)
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(Duration::from_secs(4))
        .user_agent(concat!("rqbit/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return FamilyResult {
                error: Some(format!("error building HTTP client: {e:#}")),
                ..Default::default()
            };
        }
    };
    let mut errors = Vec::new();
    for url in urls {
        let res = async {
            let r = client.get(url).send().await?.error_for_status()?;
            r.text().await
        }
        .await;
        match res {
            Ok(body) => match parse_answer(&body, v6) {
                Some(ip) => {
                    return FamilyResult {
                        ip: Some(ip.to_string()),
                        source: Some(url.clone()),
                        error: None,
                    };
                }
                None => errors.push(format!("{url}: unexpected answer")),
            },
            Err(e) => {
                let mut msg = if e.is_timeout() {
                    "timed out".to_owned()
                } else if e.is_connect() {
                    "can't connect".to_owned()
                } else {
                    e.to_string()
                };
                if let Some(s) = e.status() {
                    msg = format!("HTTP {s}");
                }
                errors.push(format!("{url}: {msg}"));
            }
        }
    }
    FamilyResult {
        ip: None,
        source: None,
        error: Some(if errors.is_empty() {
            "no providers configured".to_owned()
        } else {
            errors.join("; ")
        }),
    }
}

impl PublicIpMonitor {
    pub fn from_env() -> Arc<Self> {
        let enabled = !matches!(
            std::env::var("RQBIT_PUBLIC_IP_CHECK")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "false" | "0" | "no" | "off"
        );
        Arc::new(Self {
            enabled,
            v4: urls_from_env("RQBIT_PUBLIC_IP_URLS_V4", DEFAULT_V4),
            v6: urls_from_env("RQBIT_PUBLIC_IP_URLS_V6", DEFAULT_V6),
            state: Mutex::new(State::default()),
            check_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Background loop; call from inside the tokio runtime.
    pub fn spawn(self: &Arc<Self>) {
        if !self.enabled {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.check(false).await;
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        });
    }

    /// Runs a check unless one finished less than `MIN_REFRESH` ago (when
    /// `only_if_stale`) or another is running (then waits for it).
    async fn check(&self, only_if_stale: bool) {
        let _g = self.check_lock.lock().await;
        if only_if_stale
            && self
                .state
                .lock()
                .checked_instant
                .is_some_and(|t| t.elapsed() < MIN_REFRESH)
        {
            return;
        }
        self.state.lock().checking = true;
        let (v4, v6) = tokio::join!(check_family(&self.v4, false), check_family(&self.v6, true));
        debug!(ipv4 = ?v4.ip, ipv6 = ?v6.ip, "public IP check");
        let mut s = self.state.lock();
        s.ipv4 = v4;
        s.ipv6 = v6;
        s.checked_at = Some(crate::adopt::rfc3339_now());
        s.checked_instant = Some(Instant::now());
        s.checking = false;
    }

    pub async fn get(&self, refresh: bool) -> PublicIpResponse {
        if self.enabled && refresh {
            self.check(true).await;
        }
        let s = self.state.lock();
        PublicIpResponse {
            enabled: self.enabled,
            ipv4: s.ipv4.clone(),
            ipv6: s.ipv6.clone(),
            checked_at: s.checked_at.clone(),
            age_secs: s.checked_instant.map(|t| t.elapsed().as_secs()),
            check_interval_secs: CHECK_INTERVAL.as_secs(),
            checking: s.checking,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        assert_eq!(
            parse_answer("203.0.113.7\n", false),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(parse_answer("203.0.113.7", true), None);
        assert_eq!(
            parse_answer(" 2001:db8::1 ", true),
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(parse_answer("2001:db8::1", false), None);
        assert_eq!(
            parse_answer("::ffff:203.0.113.7", false),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(parse_answer("<html>", false), None);
    }
}
