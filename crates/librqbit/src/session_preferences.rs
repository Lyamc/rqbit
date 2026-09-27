use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Persisted session-level user preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPreferences {
    /// When true, I/O write errors invalidate only the failed piece and redownload it
    /// instead of fatally erroring the torrent.
    #[serde(default)]
    pub soft_recover_on_io_error: bool,

    /// Shell command to run when a torrent finishes downloading.
    ///
    /// The command is executed with `sh -c` (Unix) or `cmd /C` (Windows).
    /// Environment variables provided:
    /// - `RQBIT_TORRENT_ID`
    /// - `RQBIT_INFO_HASH`
    /// - `RQBIT_NAME`
    /// - `RQBIT_OUTPUT_FOLDER`
    /// Example: `mpv /path/to/audio.mp3` or `notify-send "done" "$RQBIT_NAME"`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_complete_hook: Option<String>,

    /// If set, after a torrent finishes downloading, move (or copy) its content
    /// into this directory. Relative layout under the torrent is preserved.
    /// The torrent's output folder is updated so seeding continues from the new location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_completed_path: Option<String>,

    /// When `move_completed_path` is set, copy instead of rename/move.
    #[serde(default)]
    pub move_completed_copy: bool,
}

impl Default for SessionPreferences {
    fn default() -> Self {
        Self {
            soft_recover_on_io_error: false,
            on_complete_hook: None,
            move_completed_path: None,
            move_completed_copy: false,
        }
    }
}

/// Runtime store for preferences with atomic reads and durable JSON writes.
pub struct SessionPreferencesStore {
    path: PathBuf,
    soft_recover_on_io_error: AtomicBool,
    move_completed_copy: AtomicBool,
    on_complete_hook: RwLock<Option<String>>,
    move_completed_path: RwLock<Option<String>>,
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
            has_on_complete_hook = prefs.on_complete_hook.is_some(),
            has_move_completed_path = prefs.move_completed_path.is_some(),
            move_completed_copy = prefs.move_completed_copy,
            "loaded session preferences"
        );
        Self {
            path,
            soft_recover_on_io_error: AtomicBool::new(prefs.soft_recover_on_io_error),
            move_completed_copy: AtomicBool::new(prefs.move_completed_copy),
            on_complete_hook: RwLock::new(prefs.on_complete_hook),
            move_completed_path: RwLock::new(prefs.move_completed_path),
        }
    }

    pub fn get(&self) -> SessionPreferences {
        SessionPreferences {
            soft_recover_on_io_error: self.soft_recover_on_io_error.load(Ordering::Relaxed),
            on_complete_hook: self.on_complete_hook.read().clone(),
            move_completed_path: self.move_completed_path.read().clone(),
            move_completed_copy: self.move_completed_copy.load(Ordering::Relaxed),
        }
    }

    pub fn soft_recover_on_io_error(&self) -> bool {
        self.soft_recover_on_io_error.load(Ordering::Relaxed)
    }

    pub fn on_complete_hook(&self) -> Option<String> {
        self.on_complete_hook.read().clone()
    }

    pub fn move_completed_path(&self) -> Option<String> {
        self.move_completed_path.read().clone()
    }

    pub fn move_completed_copy(&self) -> bool {
        self.move_completed_copy.load(Ordering::Relaxed)
    }

    pub async fn update(&self, prefs: SessionPreferences) -> anyhow::Result<()> {
        self.soft_recover_on_io_error
            .store(prefs.soft_recover_on_io_error, Ordering::Relaxed);
        self.move_completed_copy
            .store(prefs.move_completed_copy, Ordering::Relaxed);
        *self.on_complete_hook.write() = prefs.on_complete_hook.clone();
        *self.move_completed_path.write() = prefs.move_completed_path.clone();
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
            has_on_complete_hook = prefs.on_complete_hook.is_some(),
            has_move_completed_path = prefs.move_completed_path.is_some(),
            move_completed_copy = prefs.move_completed_copy,
            "saved session preferences"
        );
        Ok(())
    }
}

/// Spawn a shell hook without blocking the caller. Failures are logged, never fatal.
pub fn spawn_shell_hook(hook: &str, env: &[(&str, String)]) {
    let hook = hook.to_owned();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || run_shell_hook_sync(&hook, &env)).await;
        match result {
            Ok(Ok(status)) => {
                if status.success() {
                    info!(?status, "on_complete_hook finished successfully");
                } else {
                    warn!(?status, "on_complete_hook exited with non-zero status");
                }
            }
            Ok(Err(e)) => warn!(error=?e, "on_complete_hook failed to start"),
            Err(e) => warn!(error=?e, "on_complete_hook task join error"),
        }
    });
}

fn run_shell_hook_sync(
    hook: &str,
    env: &[(String, String)],
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/C").arg(hook);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.status()
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(hook);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.status()
    }
}
