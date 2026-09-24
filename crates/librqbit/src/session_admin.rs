use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Process/server settings that typically need a restart to apply.
/// Persisted as `admin.json` next to `preferences.json`.
///
/// Precedence at startup: process environment / CLI flags override these values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdminConfig {
    /// e.g. "0.0.0.0:9030". Applied on next process start if env/CLI unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api_listen_addr: Option<String>,

    /// `username:password`. None / empty disables file-based basic auth.
    /// Applied on next process start if `RQBIT_HTTP_BASIC_AUTH_USERPASS` unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth_userpass: Option<String>,
}

impl AdminConfig {
    pub fn sanitize(mut self) -> Self {
        if let Some(s) = self.http_api_listen_addr.as_mut() {
            let t = s.trim().to_owned();
            if t.is_empty() {
                self.http_api_listen_addr = None;
            } else {
                *s = t;
            }
        }
        if let Some(s) = self.basic_auth_userpass.as_mut() {
            let t = s.trim().to_owned();
            if t.is_empty() {
                self.basic_auth_userpass = None;
            } else {
                *s = t;
            }
        }
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdminConfigPublic {
    pub http_api_listen_addr: Option<String>,
    pub basic_auth_enabled: bool,
    pub basic_auth_user: Option<String>,
    pub basic_auth_password_set: bool,
}

/// PATCH body for admin config. Password is write-only.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdminConfigUpdate {
    #[serde(default)]
    pub http_api_listen_addr: Option<String>,
    /// When Some(false), clears auth. When Some(true), expects user (+ optional password).
    #[serde(default)]
    pub basic_auth_enabled: Option<bool>,
    #[serde(default)]
    pub basic_auth_user: Option<String>,
    /// If None while enabling/keeping auth, preserve existing password.
    #[serde(default)]
    pub basic_auth_password: Option<String>,
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
                // Allow updating user/password without toggling enabled flag
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
        self.update(cfg.clone()).await?;
        Ok(cfg)
    }
}
