use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Persisted session-level user preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPreferences {
    /// When true, I/O write errors invalidate only the failed piece and redownload it
    /// instead of fatally erroring the torrent.
    #[serde(default)]
    pub soft_recover_on_io_error: bool,
}

impl Default for SessionPreferences {
    fn default() -> Self {
        Self {
            soft_recover_on_io_error: false,
        }
    }
}

/// Runtime store for preferences with atomic reads and durable JSON writes.
pub struct SessionPreferencesStore {
    path: PathBuf,
    soft_recover_on_io_error: AtomicBool,
}

impl SessionPreferencesStore {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load_or_default(path: PathBuf) -> Self {
        let prefs = match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<SessionPreferences>(&bytes) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error=?e, ?path, "failed to parse preferences.json; using defaults");
                    SessionPreferences::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SessionPreferences::default(),
            Err(e) => {
                warn!(error=?e, ?path, "failed to read preferences.json; using defaults");
                SessionPreferences::default()
            }
        };
        info!(
            ?path,
            soft_recover_on_io_error = prefs.soft_recover_on_io_error,
            "loaded session preferences"
        );
        Self {
            path,
            soft_recover_on_io_error: AtomicBool::new(prefs.soft_recover_on_io_error),
        }
    }

    pub fn get(&self) -> SessionPreferences {
        SessionPreferences {
            soft_recover_on_io_error: self.soft_recover_on_io_error.load(Ordering::Relaxed),
        }
    }

    pub fn soft_recover_on_io_error(&self) -> bool {
        self.soft_recover_on_io_error.load(Ordering::Relaxed)
    }

    pub async fn update(&self, prefs: SessionPreferences) -> anyhow::Result<()> {
        self.soft_recover_on_io_error
            .store(prefs.soft_recover_on_io_error, Ordering::Relaxed);
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&prefs)?;
        tokio::fs::write(&tmp, &data).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        info!(
            path=?self.path,
            soft_recover_on_io_error = prefs.soft_recover_on_io_error,
            "saved session preferences"
        );
        Ok(())
    }
}
