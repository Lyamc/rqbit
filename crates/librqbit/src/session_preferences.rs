use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::media_classify::{AutoOrganizeFolders, MediaType};

/// One step in the ordered on-complete action pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompletionAction {
    /// Run a shell command (`sh -c` / `cmd /C`). Same env vars as the legacy hook.
    Shell {
        command: String,
    },
    /// Relocate torrent content into `path` (move, or copy when `copy` is true).
    Move {
        path: String,
        #[serde(default)]
        copy: bool,
    },
    /// Classify media and relocate under `auto_organize_root` / type subfolder.
    Organize,
    /// Strip the configured incomplete-extension suffix from on-disk filenames.
    DropIncompleteExt,
}

/// Persisted session-level user preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPreferences {
    /// When true, I/O write errors invalidate only the failed piece and redownload it
    /// instead of fatally erroring the torrent.
    #[serde(default)]
    pub soft_recover_on_io_error: bool,

    /// Legacy: shell command when a torrent finishes. Migrated into `completion_actions`
    /// when the actions list is empty (see [`SessionPreferences::effective_actions`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_complete_hook: Option<String>,

    /// Legacy: move/copy completed content here. See `effective_actions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_completed_path: Option<String>,

    /// Legacy: copy instead of move for `move_completed_path`.
    #[serde(default)]
    pub move_completed_copy: bool,

    /// When true (and not overridden by an explicit actions list that omits Organize),
    /// completed torrents are classified and moved into a type subfolder.
    /// **Disabled by default** — heuristics can mis-classify.
    #[serde(default)]
    pub auto_organize_enabled: bool,

    /// Root for auto-organize. Empty/None = session default download folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_organize_root: Option<String>,

    /// Mapping of media type → subfolder name under the organize root.
    #[serde(default)]
    pub auto_organize_folders: AutoOrganizeFolders,

    /// While downloading, append this suffix to on-disk filenames (e.g. `.!qB`, `.part`).
    /// Empty / unset = disabled. On completion, `DropIncompleteExt` removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_extension: Option<String>,

    /// Ordered on-complete pipeline. When non-empty, this is the sole source of actions
    /// (legacy hook/move fields and the auto_organize/incomplete toggles are ignored for
    /// execution — configure them as explicit actions instead). When empty, actions are
    /// synthesized from legacy fields + toggles for backward compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completion_actions: Vec<CompletionAction>,

    /// Default max connected peers per torrent for newly added torrents.
    /// Applied live when preferences are saved. None / unset = engine default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_limit: Option<usize>,
}

impl Default for SessionPreferences {
    fn default() -> Self {
        Self {
            soft_recover_on_io_error: false,
            on_complete_hook: None,
            move_completed_path: None,
            move_completed_copy: false,
            auto_organize_enabled: false,
            auto_organize_root: None,
            auto_organize_folders: AutoOrganizeFolders::default(),
            incomplete_extension: None,
            completion_actions: Vec::new(),
            peer_limit: None,
        }
    }
}

impl SessionPreferences {
    /// Normalize empty strings to None for optional path/command fields.
    pub fn sanitize(mut self) -> Self {
        fn empty_to_none(v: &mut Option<String>) {
            if let Some(s) = v {
                if s.trim().is_empty() {
                    *v = None;
                }
            }
        }
        empty_to_none(&mut self.on_complete_hook);
        empty_to_none(&mut self.move_completed_path);
        empty_to_none(&mut self.auto_organize_root);
        empty_to_none(&mut self.incomplete_extension);
        self.completion_actions.retain(|a| match a {
            CompletionAction::Shell { command } => !command.trim().is_empty(),
            CompletionAction::Move { path, .. } => !path.trim().is_empty(),
            CompletionAction::Organize | CompletionAction::DropIncompleteExt => true,
        });
        self
    }

    /// Actions to run on torrent completion, in order.
    pub fn effective_actions(&self) -> Vec<CompletionAction> {
        if !self.completion_actions.is_empty() {
            return self.completion_actions.clone();
        }
        let mut actions = Vec::new();
        if self
            .incomplete_extension
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            actions.push(CompletionAction::DropIncompleteExt);
        }
        if self.auto_organize_enabled {
            actions.push(CompletionAction::Organize);
        }
        if let Some(path) = self.move_completed_path.clone() {
            actions.push(CompletionAction::Move {
                path,
                copy: self.move_completed_copy,
            });
        }
        if let Some(command) = self.on_complete_hook.clone() {
            actions.push(CompletionAction::Shell { command });
        }
        actions
    }

    pub fn incomplete_extension_str(&self) -> Option<&str> {
        self.incomplete_extension
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn folder_for_media(&self, t: MediaType) -> &str {
        self.auto_organize_folders.folder_for(t)
    }
}

/// Runtime store for preferences with durable JSON writes.
pub struct SessionPreferencesStore {
    path: PathBuf,
    soft_recover_on_io_error: AtomicBool,
    prefs: RwLock<SessionPreferences>,
}

impl SessionPreferencesStore {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load_or_default(path: PathBuf) -> Self {
        let prefs = match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<SessionPreferences>(&bytes) {
                Ok(p) => p.sanitize(),
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
            auto_organize_enabled = prefs.auto_organize_enabled,
            has_incomplete_extension = prefs.incomplete_extension_str().is_some(),
            completion_actions = prefs.completion_actions.len(),
            effective_actions = prefs.effective_actions().len(),
            "loaded session preferences"
        );
        Self {
            soft_recover_on_io_error: AtomicBool::new(prefs.soft_recover_on_io_error),
            prefs: RwLock::new(prefs),
            path,
        }
    }

    pub fn get(&self) -> SessionPreferences {
        self.prefs.read().clone()
    }

    pub fn soft_recover_on_io_error(&self) -> bool {
        self.soft_recover_on_io_error.load(Ordering::Relaxed)
    }

    pub fn on_complete_hook(&self) -> Option<String> {
        self.prefs.read().on_complete_hook.clone()
    }

    pub fn move_completed_path(&self) -> Option<String> {
        self.prefs.read().move_completed_path.clone()
    }

    pub fn move_completed_copy(&self) -> bool {
        self.prefs.read().move_completed_copy
    }

    pub fn incomplete_extension(&self) -> Option<String> {
        self.prefs
            .read()
            .incomplete_extension_str()
            .map(|s| s.to_owned())
    }

    pub async fn update(&self, prefs: SessionPreferences) -> anyhow::Result<()> {
        let prefs = prefs.sanitize();
        self.soft_recover_on_io_error
            .store(prefs.soft_recover_on_io_error, Ordering::Relaxed);
        *self.prefs.write() = prefs.clone();
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
            auto_organize_enabled = prefs.auto_organize_enabled,
            has_incomplete_extension = prefs.incomplete_extension_str().is_some(),
            completion_actions = prefs.completion_actions.len(),
            "saved session preferences"
        );
        Ok(())
    }
    /// Re-read preferences.json from disk into memory (force-reload).
    pub async fn reload_from_disk(&self) -> anyhow::Result<SessionPreferences> {
        let prefs = match tokio::fs::read(&self.path).await {
            Ok(bytes) => serde_json::from_slice::<SessionPreferences>(&bytes)
                .map(|p| p.sanitize())
                .map_err(|e| anyhow::anyhow!("parse preferences.json: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SessionPreferences::default(),
            Err(e) => return Err(e.into()),
        };
        self.soft_recover_on_io_error
            .store(prefs.soft_recover_on_io_error, Ordering::Relaxed);
        *self.prefs.write() = prefs.clone();
        info!(path=?self.path, "reloaded session preferences from disk");
        Ok(prefs)
    }
}

/// Build incomplete-extension renames for newly added torrents.
/// Returns a map of file_id -> relative path with the incomplete suffix appended.
pub fn apply_incomplete_suffix_to_renames(
    file_infos: &[(usize, PathBuf, bool)],
    existing: &std::collections::HashMap<usize, PathBuf>,
    incomplete_ext: &str,
) -> std::collections::HashMap<usize, PathBuf> {
    let mut out = existing.clone();
    for (file_id, relative, is_padding) in file_infos {
        if *is_padding {
            continue;
        }
        let base = out
            .get(file_id)
            .cloned()
            .unwrap_or_else(|| relative.clone());
        let s = base.to_string_lossy();
        if s.ends_with(incomplete_ext) {
            continue;
        }
        let mut os = base.into_os_string();
        os.push(incomplete_ext);
        out.insert(*file_id, PathBuf::from(os));
    }
    out
}

/// Spawn a shell hook without blocking the caller. Failures are logged, never fatal.
#[allow(dead_code)]
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
                    info!(?status, "completion shell action finished successfully");
                } else {
                    warn!(?status, "completion shell action exited with non-zero status");
                }
            }
            Ok(Err(e)) => warn!(error=?e, "completion shell action failed to start"),
            Err(e) => warn!(error=?e, "completion shell action task join error"),
        }
    });
}

/// Run a shell hook and wait for it (used inside the ordered action pipeline).
pub async fn run_shell_hook_async(hook: &str, env: &[(&str, String)]) {
    let hook = hook.to_owned();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let result = tokio::task::spawn_blocking(move || run_shell_hook_sync(&hook, &env)).await;
    match result {
        Ok(Ok(status)) => {
            if status.success() {
                info!(?status, "completion shell action finished successfully");
            } else {
                warn!(?status, "completion shell action exited with non-zero status");
            }
        }
        Ok(Err(e)) => warn!(error=?e, "completion shell action failed to start"),
        Err(e) => warn!(error=?e, "completion shell action task join error"),
    }
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
