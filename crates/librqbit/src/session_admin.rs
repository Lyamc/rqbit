use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Process/server settings that typically need a restart to apply.
/// Persisted as `admin.json` next to `preferences.json`.
///
/// Precedence at startup: process environment variables override these values.
/// CLI flags that map to the same env keys are already resolved by clap before
/// we overlay — we only apply admin.json when the corresponding env is unset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdminConfig {
    /// e.g. "0.0.0.0:9030". Applied on next process start if env/CLI unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api_listen_addr: Option<String>,

    /// `username:password`. None / empty disables file-based basic auth.
    /// Applied on next process start if `RQBIT_HTTP_BASIC_AUTH_USERPASS` unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth_userpass: Option<String>,

    // ---- Connection (restart required) ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_dht: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_dht_persistence: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_lsd: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_trackers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_utp_listen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_tcp_listen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_tcp_connect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_upnp_port_forward: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_device: Option<String>,

    // ---- BitTorrent / peers (mostly restart; peer_limit also in preferences for live) ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrent_init_limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_connect_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_read_write_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocklist_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowlist_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fastresume: Option<bool>,
}

impl AdminConfig {
    pub fn sanitize(mut self) -> Self {
        fn trim_opt(v: &mut Option<String>) {
            if let Some(s) = v {
                let t = s.trim().to_owned();
                if t.is_empty() {
                    *v = None;
                } else {
                    *s = t;
                }
            }
        }
        trim_opt(&mut self.http_api_listen_addr);
        trim_opt(&mut self.basic_auth_userpass);
        trim_opt(&mut self.socks_proxy_url);
        trim_opt(&mut self.bind_device);
        trim_opt(&mut self.blocklist_url);
        trim_opt(&mut self.allowlist_url);
        self
    }

    pub fn basic_auth_enabled(&self) -> bool {
        self.basic_auth_userpass
            .as_ref()
            .map(|s| s.contains(':') && !s.starts_with(':') && !s.ends_with(':'))
            .unwrap_or(false)
    }

    /// Redacted view for API responses (never returns the password).
    pub fn public_view(&self) -> AdminConfigPublic {
        let (user, has_password) = match &self.basic_auth_userpass {
            Some(up) => match up.split_once(':') {
                Some((u, p)) => (Some(u.to_owned()), !p.is_empty()),
                None => (None, false),
            },
            None => (None, false),
        };
        AdminConfigPublic {
            http_api_listen_addr: self.http_api_listen_addr.clone(),
            basic_auth_enabled: self.basic_auth_enabled(),
            basic_auth_user: user,
            basic_auth_password_set: has_password,
            listen_port: self.listen_port,
            announce_port: self.announce_port,
            disable_dht: self.disable_dht,
            disable_dht_persistence: self.disable_dht_persistence,
            disable_lsd: self.disable_lsd,
            disable_trackers: self.disable_trackers,
            enable_utp_listen: self.enable_utp_listen,
            disable_tcp_listen: self.disable_tcp_listen,
            disable_tcp_connect: self.disable_tcp_connect,
            disable_upnp_port_forward: self.disable_upnp_port_forward,
            socks_proxy_url: self.socks_proxy_url.clone(),
            ipv4_only: self.ipv4_only,
            bind_device: self.bind_device.clone(),
            peer_limit: self.peer_limit,
            concurrent_init_limit: self.concurrent_init_limit,
            peer_connect_timeout_secs: self.peer_connect_timeout_secs,
            peer_read_write_timeout_secs: self.peer_read_write_timeout_secs,
            blocklist_url: self.blocklist_url.clone(),
            allowlist_url: self.allowlist_url.clone(),
            fastresume: self.fastresume,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdminConfigPublic {
    pub http_api_listen_addr: Option<String>,
    pub basic_auth_enabled: bool,
    pub basic_auth_user: Option<String>,
    pub basic_auth_password_set: bool,
    pub listen_port: Option<u16>,
    pub announce_port: Option<u16>,
    pub disable_dht: Option<bool>,
    pub disable_dht_persistence: Option<bool>,
    pub disable_lsd: Option<bool>,
    pub disable_trackers: Option<bool>,
    pub enable_utp_listen: Option<bool>,
    pub disable_tcp_listen: Option<bool>,
    pub disable_tcp_connect: Option<bool>,
    pub disable_upnp_port_forward: Option<bool>,
    pub socks_proxy_url: Option<String>,
    pub ipv4_only: Option<bool>,
    pub bind_device: Option<String>,
    pub peer_limit: Option<usize>,
    pub concurrent_init_limit: Option<usize>,
    pub peer_connect_timeout_secs: Option<u64>,
    pub peer_read_write_timeout_secs: Option<u64>,
    pub blocklist_url: Option<String>,
    pub allowlist_url: Option<String>,
    pub fastresume: Option<bool>,
}

/// PATCH body for admin config. Password is write-only.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdminConfigUpdate {
    #[serde(default)]
    pub http_api_listen_addr: Option<String>,
    #[serde(default)]
    pub basic_auth_enabled: Option<bool>,
    #[serde(default)]
    pub basic_auth_user: Option<String>,
    #[serde(default)]
    pub basic_auth_password: Option<String>,

    #[serde(default)]
    pub listen_port: Option<u16>,
    /// When true, clear listen_port override (use CLI/env default).
    #[serde(default)]
    pub clear_listen_port: Option<bool>,
    #[serde(default)]
    pub announce_port: Option<u16>,
    #[serde(default)]
    pub clear_announce_port: Option<bool>,

    #[serde(default)]
    pub disable_dht: Option<bool>,
    #[serde(default)]
    pub clear_disable_dht: Option<bool>,
    #[serde(default)]
    pub disable_dht_persistence: Option<bool>,
    #[serde(default)]
    pub clear_disable_dht_persistence: Option<bool>,
    #[serde(default)]
    pub disable_lsd: Option<bool>,
    #[serde(default)]
    pub clear_disable_lsd: Option<bool>,
    #[serde(default)]
    pub disable_trackers: Option<bool>,
    #[serde(default)]
    pub clear_disable_trackers: Option<bool>,
    #[serde(default)]
    pub enable_utp_listen: Option<bool>,
    #[serde(default)]
    pub clear_enable_utp_listen: Option<bool>,
    #[serde(default)]
    pub disable_tcp_listen: Option<bool>,
    #[serde(default)]
    pub clear_disable_tcp_listen: Option<bool>,
    #[serde(default)]
    pub disable_tcp_connect: Option<bool>,
    #[serde(default)]
    pub clear_disable_tcp_connect: Option<bool>,
    #[serde(default)]
    pub disable_upnp_port_forward: Option<bool>,
    #[serde(default)]
    pub clear_disable_upnp_port_forward: Option<bool>,
    #[serde(default)]
    pub socks_proxy_url: Option<String>,
    #[serde(default)]
    pub ipv4_only: Option<bool>,
    #[serde(default)]
    pub clear_ipv4_only: Option<bool>,
    #[serde(default)]
    pub bind_device: Option<String>,

    #[serde(default)]
    pub peer_limit: Option<usize>,
    #[serde(default)]
    pub clear_peer_limit: Option<bool>,
    #[serde(default)]
    pub concurrent_init_limit: Option<usize>,
    #[serde(default)]
    pub clear_concurrent_init_limit: Option<bool>,
    #[serde(default)]
    pub peer_connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub clear_peer_connect_timeout_secs: Option<bool>,
    #[serde(default)]
    pub peer_read_write_timeout_secs: Option<u64>,
    #[serde(default)]
    pub clear_peer_read_write_timeout_secs: Option<bool>,
    #[serde(default)]
    pub blocklist_url: Option<String>,
    #[serde(default)]
    pub allowlist_url: Option<String>,
    #[serde(default)]
    pub fastresume: Option<bool>,
    #[serde(default)]
    pub clear_fastresume: Option<bool>,
}

pub struct AdminConfigStore {
    path: PathBuf,
    config: RwLock<AdminConfig>,
}

impl AdminConfigStore {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load_or_default(path: PathBuf) -> Self {
        let config = match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<AdminConfig>(&bytes) {
                Ok(c) => c.sanitize(),
                Err(e) => {
                    warn!(error=?e, ?path, "failed to parse admin.json; using defaults");
                    AdminConfig::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AdminConfig::default(),
            Err(e) => {
                warn!(error=?e, ?path, "failed to read admin.json; using defaults");
                AdminConfig::default()
            }
        };
        info!(
            ?path,
            has_listen = config.http_api_listen_addr.is_some(),
            basic_auth = config.basic_auth_enabled(),
            listen_port = ?config.listen_port,
            "loaded admin config"
        );
        Self {
            path,
            config: RwLock::new(config),
        }
    }

    pub fn get(&self) -> AdminConfig {
        self.config.read().clone()
    }

    pub async fn update(&self, config: AdminConfig) -> anyhow::Result<()> {
        let config = config.sanitize();
        *self.config.write() = config.clone();
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&config)?;
        tokio::fs::write(&tmp, &data).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        info!(
            path=?self.path,
            has_listen = config.http_api_listen_addr.is_some(),
            basic_auth = config.basic_auth_enabled(),
            "saved admin config"
        );
        Ok(())
    }

    pub async fn apply_update(&self, patch: AdminConfigUpdate) -> anyhow::Result<AdminConfig> {
        let mut cfg = self.get();
        if let Some(addr) = patch.http_api_listen_addr {
            cfg.http_api_listen_addr = if addr.trim().is_empty() {
                None
            } else {
                Some(addr)
            };
        }
        match patch.basic_auth_enabled {
            Some(false) => {
                cfg.basic_auth_userpass = None;
            }
            Some(true) => {
                let user = patch
                    .basic_auth_user
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_owned())
                    .or_else(|| {
                        cfg.basic_auth_userpass
                            .as_ref()
                            .and_then(|up| up.split_once(':').map(|(u, _)| u.to_owned()))
                    })
                    .ok_or_else(|| anyhow::anyhow!("basic auth username required"))?;
                let pass = match patch.basic_auth_password {
                    Some(p) if !p.is_empty() => p,
                    Some(_) => {
                        anyhow::bail!("basic auth password cannot be empty");
                    }
                    None => cfg
                        .basic_auth_userpass
                        .as_ref()
                        .and_then(|up| up.split_once(':').map(|(_, p)| p.to_owned()))
                        .filter(|p| !p.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!("basic auth password required when enabling")
                        })?,
                };
                cfg.basic_auth_userpass = Some(format!("{user}:{pass}"));
            }
            None => {
                if patch.basic_auth_user.is_some() || patch.basic_auth_password.is_some() {
                    if !cfg.basic_auth_enabled() && patch.basic_auth_password.is_none() {
                        // ignore partial updates when auth disabled
                    } else {
                        let user = patch
                            .basic_auth_user
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_owned())
                            .or_else(|| {
                                cfg.basic_auth_userpass
                                    .as_ref()
                                    .and_then(|up| up.split_once(':').map(|(u, _)| u.to_owned()))
                            })
                            .ok_or_else(|| anyhow::anyhow!("basic auth username required"))?;
                        let pass = match &patch.basic_auth_password {
                            Some(p) if !p.is_empty() => p.clone(),
                            Some(_) => anyhow::bail!("basic auth password cannot be empty"),
                            None => cfg
                                .basic_auth_userpass
                                .as_ref()
                                .and_then(|up| up.split_once(':').map(|(_, p)| p.to_owned()))
                                .filter(|p| !p.is_empty())
                                .ok_or_else(|| anyhow::anyhow!("basic auth password required"))?,
                        };
                        cfg.basic_auth_userpass = Some(format!("{user}:{pass}"));
                    }
                }
            }
        }

        macro_rules! opt_u {
            ($clear:expr, $set:expr, $field:ident) => {
                if $clear == Some(true) {
                    cfg.$field = None;
                } else if let Some(v) = $set {
                    cfg.$field = Some(v);
                }
            };
        }
        macro_rules! opt_bool {
            ($clear:expr, $set:expr, $field:ident) => {
                if $clear == Some(true) {
                    cfg.$field = None;
                } else if let Some(v) = $set {
                    cfg.$field = Some(v);
                }
            };
        }
        macro_rules! opt_str {
            ($set:expr, $field:ident) => {
                if let Some(s) = $set {
                    cfg.$field = if s.trim().is_empty() {
                        None
                    } else {
                        Some(s)
                    };
                }
            };
        }

        opt_u!(patch.clear_listen_port, patch.listen_port, listen_port);
        opt_u!(patch.clear_announce_port, patch.announce_port, announce_port);
        opt_bool!(patch.clear_disable_dht, patch.disable_dht, disable_dht);
        opt_bool!(
            patch.clear_disable_dht_persistence,
            patch.disable_dht_persistence,
            disable_dht_persistence
        );
        opt_bool!(patch.clear_disable_lsd, patch.disable_lsd, disable_lsd);
        opt_bool!(
            patch.clear_disable_trackers,
            patch.disable_trackers,
            disable_trackers
        );
        opt_bool!(
            patch.clear_enable_utp_listen,
            patch.enable_utp_listen,
            enable_utp_listen
        );
        opt_bool!(
            patch.clear_disable_tcp_listen,
            patch.disable_tcp_listen,
            disable_tcp_listen
        );
        opt_bool!(
            patch.clear_disable_tcp_connect,
            patch.disable_tcp_connect,
            disable_tcp_connect
        );
        opt_bool!(
            patch.clear_disable_upnp_port_forward,
            patch.disable_upnp_port_forward,
            disable_upnp_port_forward
        );
        opt_str!(patch.socks_proxy_url, socks_proxy_url);
        opt_bool!(patch.clear_ipv4_only, patch.ipv4_only, ipv4_only);
        opt_str!(patch.bind_device, bind_device);
        opt_u!(patch.clear_peer_limit, patch.peer_limit, peer_limit);
        opt_u!(
            patch.clear_concurrent_init_limit,
            patch.concurrent_init_limit,
            concurrent_init_limit
        );
        opt_u!(
            patch.clear_peer_connect_timeout_secs,
            patch.peer_connect_timeout_secs,
            peer_connect_timeout_secs
        );
        opt_u!(
            patch.clear_peer_read_write_timeout_secs,
            patch.peer_read_write_timeout_secs,
            peer_read_write_timeout_secs
        );
        opt_str!(patch.blocklist_url, blocklist_url);
        opt_str!(patch.allowlist_url, allowlist_url);
        opt_bool!(patch.clear_fastresume, patch.fastresume, fastresume);

        self.update(cfg.clone()).await?;
        Ok(cfg)
    }
}
