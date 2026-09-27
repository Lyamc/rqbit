//! Registering rqbit as the handler for `magnet:` links and `.torrent` files.
//!
//! Standard library only, so it can be cross-compiled on its own and
//! exercised under wine.
//!
//! - Windows: per-user (HKCU) ProgIDs `rqbit.Magnet` / `rqbit.Torrent`, a
//!   `Capabilities` key listed under `RegisteredApplications` (so rqbit shows
//!   up in Settings > Default apps) and `.torrent\OpenWithProgids`. If nothing
//!   handles magnet links / .torrent files yet, rqbit also becomes the plain
//!   per-user handler. Windows 10/11 don't let an app take over an existing
//!   default silently: the user confirms in Settings (opened afterwards).
//! - Linux: `~/.local/share/applications/rqbit-gpui.desktop` with the MIME
//!   types, then `xdg-mime default` (falling back to editing mimeapps.list).
//! - macOS needs an app bundle (`CFBundleURLTypes`), which this build isn't.

use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "rqbit";
pub const MAGNET_PROGID: &str = "rqbit.Magnet";
pub const TORRENT_PROGID: &str = "rqbit.Torrent";
pub const CAPABILITIES_KEY: &str = r"Software\rqbit\Capabilities";
pub const DESKTOP_FILE: &str = "rqbit-gpui.desktop";
pub const MIME_TYPES: [&str; 2] = ["x-scheme-handler/magnet", "application/x-bittorrent"];

/// How the OS should start rqbit: the executable plus leading arguments
/// (`gui` when running as `rqbit gui`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub exe: PathBuf,
    pub prefix_args: Vec<String>,
}

impl Launcher {
    pub fn new(exe: PathBuf) -> Self {
        // Split on both separators so Windows paths parse the same everywhere.
        let full = exe.to_string_lossy().to_ascii_lowercase();
        let stem = full.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
        let prefix_args = if stem.starts_with("rqbit-gpui") {
            Vec::new()
        } else {
            vec!["gui".to_owned()]
        };
        Self { exe, prefix_args }
    }

    pub fn current() -> std::io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?))
    }

    /// `"C:\path\rqbit-gpui.exe" "%1"` (Windows `shell\open\command`).
    pub fn windows_command(&self) -> String {
        let mut s = format!("\"{}\"", self.exe.display());
        for a in &self.prefix_args {
            s.push(' ');
            s.push_str(a);
        }
        s.push_str(" \"%1\"");
        s
    }

    /// `Exec=` value for the .desktop file (quoted per the desktop entry spec).
    pub fn desktop_exec(&self) -> String {
        let mut parts = vec![desktop_quote(&self.exe.to_string_lossy())];
        parts.extend(self.prefix_args.iter().map(|a| desktop_quote(a)));
        parts.push("%U".to_owned());
        parts.join(" ")
    }
}

fn desktop_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@".contains(c));
    if plain {
        return s.to_owned();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c)
            }
            // A literal % must be doubled in Exec.
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One `HKCU\<key>` value (`name: None` is the key's default value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub key: String,
    pub name: Option<String>,
    pub data: String,
}

fn rv(key: &str, name: Option<&str>, data: &str) -> RegValue {
    RegValue {
        key: key.to_owned(),
        name: name.map(str::to_owned),
        data: data.to_owned(),
    }
}

/// Values always written on Windows (all under HKCU).
pub fn windows_values(l: &Launcher) -> Vec<RegValue> {
    let cmd = l.windows_command();
    let icon = format!("\"{}\",0", l.exe.display());
    let m = format!(r"Software\Classes\{MAGNET_PROGID}");
    let t = format!(r"Software\Classes\{TORRENT_PROGID}");
    vec![
        rv(&m, None, "URL:Magnet link (rqbit)"),
        rv(&m, Some("URL Protocol"), ""),
        rv(&format!(r"{m}\DefaultIcon"), None, &icon),
        rv(&format!(r"{m}\shell\open\command"), None, &cmd),
        rv(&t, None, "BitTorrent file (rqbit)"),
        rv(&format!(r"{t}\DefaultIcon"), None, &icon),
        rv(&format!(r"{t}\shell\open\command"), None, &cmd),
        rv(r"Software\Classes\.torrent\OpenWithProgids", Some(TORRENT_PROGID), ""),
        rv(CAPABILITIES_KEY, Some("ApplicationName"), APP_NAME),
        rv(
            CAPABILITIES_KEY,
            Some("ApplicationDescription"),
            "Adds magnet links and .torrent files to your rqbit server.",
        ),
        rv(CAPABILITIES_KEY, Some("ApplicationIcon"), &icon),
        rv(&format!(r"{CAPABILITIES_KEY}\URLAssociations"), Some("magnet"), MAGNET_PROGID),
        rv(
            &format!(r"{CAPABILITIES_KEY}\FileAssociations"),
            Some(".torrent"),
            TORRENT_PROGID,
        ),
        rv(r"Software\RegisteredApplications", Some(APP_NAME), CAPABILITIES_KEY),
    ]
}

/// Per-user plain `magnet:` handler, written only when no handler exists.
pub fn windows_magnet_fallback(l: &Launcher) -> Vec<RegValue> {
    vec![
        rv(r"Software\Classes\magnet", None, "URL:Magnet link"),
        rv(r"Software\Classes\magnet", Some("URL Protocol"), ""),
        rv(
            r"Software\Classes\magnet\shell\open\command",
            None,
            &l.windows_command(),
        ),
    ]
}

/// Per-user `.torrent` default, written only when no default exists.
pub fn windows_torrent_fallback() -> Vec<RegValue> {
    vec![rv(r"Software\Classes\.torrent", None, TORRENT_PROGID)]
}

fn reg_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A `.reg` file registering (or removing) rqbit for the current user; used
/// by the installer / manual setup script. It only adds rqbit as a choice
/// (no fallbacks): pick it in Settings > Default apps afterwards.
pub fn windows_reg_file(l: &Launcher, register: bool) -> String {
    let mut out = String::from("Windows Registry Editor Version 5.00\r\n");
    if register {
        let mut last_key = String::new();
        for v in windows_values(l) {
            if v.key != last_key {
                out.push_str(&format!("\r\n[HKEY_CURRENT_USER\\{}]\r\n", v.key));
                last_key = v.key.clone();
            }
            let name = match &v.name {
                None => "@".to_owned(),
                Some(n) => format!("\"{}\"", reg_escape(n)),
            };
            out.push_str(&format!("{name}=\"{}\"\r\n", reg_escape(&v.data)));
        }
    } else {
        for k in [
            format!(r"Software\Classes\{MAGNET_PROGID}"),
            format!(r"Software\Classes\{TORRENT_PROGID}"),
            r"Software\rqbit".to_owned(),
        ] {
            out.push_str(&format!("\r\n[-HKEY_CURRENT_USER\\{k}]\r\n"));
        }
        out.push_str(&format!(
            "\r\n[HKEY_CURRENT_USER\\Software\\RegisteredApplications]\r\n\"{APP_NAME}\"=-\r\n"
        ));
        out.push_str(&format!(
            "\r\n[HKEY_CURRENT_USER\\Software\\Classes\\.torrent\\OpenWithProgids]\r\n\"{TORRENT_PROGID}\"=-\r\n"
        ));
    }
    out
}

/// The `.desktop` file content.
pub fn desktop_entry(l: &Launcher) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=rqbit\n\
         GenericName=BitTorrent client\n\
         Comment=Add magnet links and .torrent files to your rqbit server\n\
         Exec={}\n\
         Icon=application-x-bittorrent\n\
         Terminal=false\n\
         Categories=Network;FileTransfer;P2P;\n\
         MimeType={};\n",
        l.desktop_exec(),
        MIME_TYPES.join(";")
    )
}

/// mimeapps.list with `desktop` set as the default for `mimes` (the fallback
/// when `xdg-mime` isn't installed).
pub fn mimeapps_set_defaults(content: &str, desktop: &str, mimes: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_defaults = false;
    let mut seen_defaults = false;
    let flush = |out: &mut Vec<String>| {
        for m in mimes {
            out.push(format!("{m}={desktop};"));
        }
    };
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_defaults {
                flush(&mut out);
            }
            in_defaults = t == "[Default Applications]";
            seen_defaults |= in_defaults;
            out.push(line.to_owned());
            continue;
        }
        if in_defaults
            && let Some((k, _)) = t.split_once('=')
            && mimes.contains(&k.trim())
        {
            continue;
        }
        out.push(line.to_owned());
    }
    if in_defaults {
        flush(&mut out);
    }
    if !seen_defaults {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[Default Applications]".to_owned());
        flush(&mut out);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// mimeapps.list without any reference to `desktop` (unregister).
pub fn mimeapps_remove(content: &str, desktop: &str) -> String {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') || t.starts_with('#') {
            out.push(line.to_owned());
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|(_, v)| v.contains(desktop)) else {
            // Lines not mentioning rqbit stay byte-for-byte.
            out.push(line.to_owned());
            continue;
        };
        let rest: Vec<&str> = v
            .split(';')
            .filter(|s| !s.trim().is_empty() && s.trim() != desktop)
            .collect();
        if !rest.is_empty() {
            out.push(format!("{k}={};", rest.join(";")));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Whether rqbit is the default (per handler) and registered at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    pub registered: bool,
    pub magnet_default: bool,
    pub torrent_default: bool,
}

impl Status {
    pub fn is_default(&self) -> bool {
        self.magnet_default && self.torrent_default
    }

    pub fn describe(&self) -> String {
        match (self.registered, self.magnet_default, self.torrent_default) {
            (_, true, true) => "rqbit is the default for magnet links and .torrent files.".into(),
            (_, true, false) => "rqbit opens magnet links; .torrent files open with another app.".into(),
            (_, false, true) => "rqbit opens .torrent files; magnet links open with another app.".into(),
            (true, false, false) => {
                "rqbit is registered but isn't the default yet (choose it in the system's default apps settings).".into()
            }
            (false, false, false) => "rqbit isn't registered for magnet links or .torrent files.".into(),
        }
    }
}

fn user_data_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share")))
}

fn user_config_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
}

/// `~/.local/share/applications/rqbit-gpui.desktop`.
pub fn desktop_file_path() -> Option<PathBuf> {
    Some(user_data_dir()?.join("applications").join(DESKTOP_FILE))
}

fn run(cmd: &str, args: &[&str]) -> std::io::Result<(bool, String)> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let out = c.output()?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

// ---------------- Windows (reg.exe) ----------------

fn reg_add(v: &RegValue) -> Result<(), String> {
    let key = format!(r"HKCU\{}", v.key);
    let mut args = vec!["add", key.as_str()];
    match &v.name {
        None => args.push("/ve"),
        Some(n) => {
            args.push("/v");
            args.push(n);
        }
    }
    args.extend(["/t", "REG_SZ", "/d", v.data.as_str(), "/f"]);
    match run("reg", &args) {
        Ok((true, _)) => Ok(()),
        Ok((false, _)) => Err(format!("reg add {key} failed")),
        Err(e) => Err(format!("can't run reg.exe: {e}")),
    }
}

/// Default value (or a named one) from `reg query` output.
pub fn parse_reg_query(out: &str, name: Option<&str>) -> Option<String> {
    for line in out.lines() {
        let t = line.trim();
        for ty in ["REG_SZ", "REG_EXPAND_SZ"] {
            let Some(pos) = t.find(ty) else { continue };
            let n = t[..pos].trim();
            let matches = match name {
                None => n.is_empty() || n.starts_with('(') || n.eq_ignore_ascii_case("@"),
                Some(want) => n.eq_ignore_ascii_case(want),
            };
            if matches {
                let v = t[pos + ty.len()..].trim();
                // Windows prints a key without a default value as "(value not set)".
                if v.starts_with('(') && v.ends_with(')') && !v.contains('\\') {
                    return None;
                }
                return Some(v.to_owned());
            }
        }
    }
    None
}

fn reg_get(root_key: &str, name: Option<&str>) -> Option<String> {
    let mut args = vec!["query", root_key];
    match name {
        None => args.push("/ve"),
        Some(n) => {
            args.push("/v");
            args.push(n);
        }
    }
    match run("reg", &args) {
        Ok((true, out)) => parse_reg_query(&out, name),
        _ => None,
    }
}

fn reg_delete_key(key: &str) {
    let _ = run("reg", &["delete", &format!(r"HKCU\{key}"), "/f"]);
}

fn reg_delete_value(key: &str, name: Option<&str>) {
    let k = format!(r"HKCU\{key}");
    let mut args = vec!["delete", k.as_str()];
    match name {
        None => args.push("/ve"),
        Some(n) => {
            args.push("/v");
            args.push(n);
        }
    }
    args.push("/f");
    let _ = run("reg", &args);
}

#[cfg(windows)]
fn notify_shell() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(
            event: i32,
            flags: u32,
            item1: *const std::ffi::c_void,
            item2: *const std::ffi::c_void,
        );
    }
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    // SAFETY: documented no-argument notification (SHCNF_IDLIST, null items).
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, 0, std::ptr::null(), std::ptr::null()) };
}

#[cfg(not(windows))]
fn notify_shell() {}

pub fn windows_register(l: &Launcher) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for v in windows_values(l) {
        reg_add(&v)?;
    }
    if reg_get(r"HKCR\magnet\shell\open\command", None).is_none() {
        for v in windows_magnet_fallback(l) {
            reg_add(&v)?;
        }
        notes.push("No app handled magnet links, so rqbit now does.".to_owned());
    }
    if reg_get(r"HKCR\.torrent", None).is_none_or(|v| v.is_empty()) {
        for v in windows_torrent_fallback() {
            reg_add(&v)?;
        }
        notes.push("No app opened .torrent files, so rqbit now does.".to_owned());
    }
    notify_shell();
    Ok(notes)
}

pub fn windows_unregister(l: &Launcher) {
    let cmd = l.windows_command();
    if reg_get(r"HKCU\Software\Classes\magnet\shell\open\command", None).as_deref() == Some(cmd.as_str())
    {
        reg_delete_key(r"Software\Classes\magnet");
    }
    if reg_get(r"HKCU\Software\Classes\.torrent", None).as_deref() == Some(TORRENT_PROGID) {
        reg_delete_value(r"Software\Classes\.torrent", None);
    }
    reg_delete_key(&format!(r"Software\Classes\{MAGNET_PROGID}"));
    reg_delete_key(&format!(r"Software\Classes\{TORRENT_PROGID}"));
    reg_delete_key(r"Software\rqbit");
    reg_delete_value(r"Software\RegisteredApplications", Some(APP_NAME));
    reg_delete_value(r"Software\Classes\.torrent\OpenWithProgids", Some(TORRENT_PROGID));
    notify_shell();
}

pub fn windows_status(l: &Launcher) -> Status {
    let cmd = l.windows_command();
    let registered = reg_get(
        &format!(r"HKCU\Software\Classes\{MAGNET_PROGID}\shell\open\command"),
        None,
    )
    .as_deref()
        == Some(cmd.as_str());
    let magnet_choice = reg_get(
        r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\magnet\UserChoice",
        Some("ProgId"),
    );
    let magnet_default = match magnet_choice {
        Some(p) => p.eq_ignore_ascii_case(MAGNET_PROGID),
        None => reg_get(r"HKCR\magnet\shell\open\command", None).as_deref() == Some(cmd.as_str()),
    };
    let torrent_choice = reg_get(
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\UserChoice",
        Some("ProgId"),
    );
    let torrent_default = match torrent_choice {
        Some(p) => p.eq_ignore_ascii_case(TORRENT_PROGID),
        None => reg_get(r"HKCR\.torrent", None)
            .is_some_and(|p| p.eq_ignore_ascii_case(TORRENT_PROGID)),
    };
    Status {
        registered,
        magnet_default,
        torrent_default,
    }
}

// ---------------- Linux (xdg) ----------------

fn mimeapps_path() -> Option<PathBuf> {
    Some(user_config_dir()?.join("mimeapps.list"))
}

pub fn linux_register(l: &Launcher) -> Result<Vec<String>, String> {
    let path = desktop_file_path().ok_or("can't find the user data directory ($HOME)")?;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    std::fs::write(&path, desktop_entry(l)).map_err(|e| format!("write {}: {e}", path.display()))?;
    let _ = run("update-desktop-database", &[&dir.to_string_lossy()]);
    let mut notes = vec![format!("Wrote {}.", path.display())];
    // One call per type: some xdg-mime versions only apply the last of several.
    let xdg_ok = MIME_TYPES
        .iter()
        .all(|m| matches!(run("xdg-mime", &["default", DESKTOP_FILE, m]), Ok((true, _))));
    match xdg_ok {
        true => notes.push("Set as default with xdg-mime.".into()),
        false => {
            let mp = mimeapps_path().ok_or("can't find the user config directory")?;
            let cur = std::fs::read_to_string(&mp).unwrap_or_default();
            if let Some(d) = mp.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(&mp, mimeapps_set_defaults(&cur, DESKTOP_FILE, &MIME_TYPES))
                .map_err(|e| format!("write {}: {e}", mp.display()))?;
            notes.push(format!("xdg-mime unavailable; updated {}.", mp.display()));
        }
    }
    Ok(notes)
}

pub fn linux_unregister() {
    if let Some(p) = desktop_file_path() {
        let _ = std::fs::remove_file(&p);
        if let Some(d) = p.parent() {
            let _ = run("update-desktop-database", &[&d.to_string_lossy()]);
        }
    }
    if let Some(mp) = mimeapps_path()
        && let Ok(cur) = std::fs::read_to_string(&mp)
    {
        let new = mimeapps_remove(&cur, DESKTOP_FILE);
        if new != cur {
            let _ = std::fs::write(&mp, new);
        }
    }
}

pub fn linux_status() -> Status {
    let q = |m: &str| {
        run("xdg-mime", &["query", "default", m])
            .ok()
            .filter(|(ok, _)| *ok)
            .is_some_and(|(_, out)| out.trim() == DESKTOP_FILE)
    };
    Status {
        registered: desktop_file_path().is_some_and(|p| p.exists()),
        magnet_default: q(MIME_TYPES[0]),
        torrent_default: q(MIME_TYPES[1]),
    }
}

// ---------------- Platform dispatch ----------------

/// Register rqbit; returns notes to show the user.
pub fn register(l: &Launcher) -> Result<Vec<String>, String> {
    if cfg!(windows) {
        windows_register(l)
    } else if cfg!(target_os = "macos") {
        Err("Not supported on macOS yet (needs an app bundle).".into())
    } else {
        linux_register(l)
    }
}

pub fn unregister(l: &Launcher) {
    if cfg!(windows) {
        windows_unregister(l)
    } else if !cfg!(target_os = "macos") {
        let _ = l;
        linux_unregister()
    }
}

pub fn status(l: &Launcher) -> Status {
    if cfg!(windows) {
        windows_status(l)
    } else if cfg!(target_os = "macos") {
        Status::default()
    } else {
        let _ = l;
        linux_status()
    }
}

/// Windows 10/11: open Settings > Default apps on rqbit's page, where the
/// user confirms (apps can't set the default silently).
pub fn open_default_apps_settings() {
    if cfg!(windows) {
        let _ = run(
            "cmd",
            &["/C", "start", "", "ms-settings:defaultapps?registeredAppUser=rqbit"],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> Launcher {
        Launcher::new(PathBuf::from(r"C:\Program Files\rqbit\rqbit-gpui.exe"))
    }

    #[test]
    fn launcher_prefix() {
        assert!(win().prefix_args.is_empty());
        let l = Launcher::new(PathBuf::from("/usr/bin/rqbit"));
        assert_eq!(l.prefix_args, vec!["gui"]);
        assert_eq!(l.desktop_exec(), "/usr/bin/rqbit gui %U");
    }

    #[test]
    fn windows_command_line() {
        assert_eq!(
            win().windows_command(),
            r#""C:\Program Files\rqbit\rqbit-gpui.exe" "%1""#
        );
        let l = Launcher::new(PathBuf::from(r"C:\rqbit\rqbit.exe"));
        assert_eq!(l.windows_command(), r#""C:\rqbit\rqbit.exe" gui "%1""#);
    }

    #[test]
    fn windows_values_cover_capabilities() {
        let v = windows_values(&win());
        let get = |k: &str, n: Option<&str>| {
            v.iter()
                .find(|x| x.key == k && x.name.as_deref() == n)
                .map(|x| x.data.clone())
        };
        assert_eq!(
            get(r"Software\Classes\rqbit.Magnet\shell\open\command", None).unwrap(),
            win().windows_command()
        );
        assert_eq!(get(r"Software\Classes\rqbit.Magnet", Some("URL Protocol")).unwrap(), "");
        assert_eq!(
            get(r"Software\rqbit\Capabilities\URLAssociations", Some("magnet")).unwrap(),
            "rqbit.Magnet"
        );
        assert_eq!(
            get(r"Software\rqbit\Capabilities\FileAssociations", Some(".torrent")).unwrap(),
            "rqbit.Torrent"
        );
        assert_eq!(
            get(r"Software\RegisteredApplications", Some("rqbit")).unwrap(),
            r"Software\rqbit\Capabilities"
        );
        // Everything is per-user, and never the shared magnet/.torrent keys.
        assert!(v.iter().all(|x| x.key != r"Software\Classes\magnet"));
        assert!(v.iter().all(|x| !(x.key == r"Software\Classes\.torrent" && x.name.is_none())));
    }

    #[test]
    fn reg_file_escapes() {
        let f = windows_reg_file(&win(), true);
        assert!(f.starts_with("Windows Registry Editor Version 5.00\r\n"));
        assert!(f.contains(
            "[HKEY_CURRENT_USER\\Software\\Classes\\rqbit.Magnet\\shell\\open\\command]\r\n@=\"\\\"C:\\\\Program Files\\\\rqbit\\\\rqbit-gpui.exe\\\" \\\"%1\\\"\"\r\n"
        ));
        let u = windows_reg_file(&win(), false);
        assert!(u.contains("[-HKEY_CURRENT_USER\\Software\\rqbit]"));
        assert!(u.contains("\"rqbit\"=-"));
        assert!(!u.contains("[-HKEY_CURRENT_USER\\Software\\Classes\\magnet]"));
    }

    #[test]
    fn desktop_file() {
        let l = Launcher::new(PathBuf::from("/home/l/My Apps/rqbit-gpui"));
        let d = desktop_entry(&l);
        assert!(d.contains("Exec=\"/home/l/My Apps/rqbit-gpui\" %U\n"));
        assert!(d.contains("MimeType=x-scheme-handler/magnet;application/x-bittorrent;\n"));
        assert_eq!(desktop_quote("50%"), "\"50%%\"");
    }

    #[test]
    fn mimeapps_edits() {
        let cur = "[Added Associations]\nx-scheme-handler/magnet=qbittorrent.desktop;\n\n[Default Applications]\ntext/html=firefox.desktop\nx-scheme-handler/magnet=qbittorrent.desktop;\n";
        let set = mimeapps_set_defaults(cur, DESKTOP_FILE, &MIME_TYPES);
        assert!(set.contains("text/html=firefox.desktop\n"));
        assert!(set.contains("[Default Applications]"));
        assert!(set.contains("x-scheme-handler/magnet=rqbit-gpui.desktop;\n"));
        assert!(set.contains("application/x-bittorrent=rqbit-gpui.desktop;\n"));
        assert!(!set.contains("[Default Applications]\ntext/html=firefox.desktop\nx-scheme-handler/magnet=qbittorrent"));
        assert_eq!(set.matches("[Default Applications]").count(), 1);
        let empty = mimeapps_set_defaults("", DESKTOP_FILE, &MIME_TYPES);
        assert_eq!(
            empty,
            "[Default Applications]\nx-scheme-handler/magnet=rqbit-gpui.desktop;\napplication/x-bittorrent=rqbit-gpui.desktop;\n"
        );
        let removed = mimeapps_remove(&set, DESKTOP_FILE);
        assert!(!removed.contains("rqbit-gpui.desktop"));
        assert!(removed.contains("x-scheme-handler/magnet=qbittorrent.desktop;"));
        assert!(removed.contains("text/html=firefox.desktop\n"));
        assert!(!removed.contains("firefox.desktop;"));
        let mixed = mimeapps_remove("[Added Associations]\na/b=rqbit-gpui.desktop;other.desktop;\n", DESKTOP_FILE);
        assert_eq!(mixed, "[Added Associations]\na/b=other.desktop;\n");
    }

    #[test]
    fn reg_query_parsing() {
        let out = "\r\nHKEY_CLASSES_ROOT\\magnet\\shell\\open\\command\r\n    (Default)    REG_SZ    \"C:\\x\\qbittorrent.exe\" \"%1\"\r\n\r\n";
        assert_eq!(
            parse_reg_query(out, None).as_deref(),
            Some("\"C:\\x\\qbittorrent.exe\" \"%1\"")
        );
        // wine prints the default value with an empty name.
        let wine = "HKEY_CURRENT_USER\\Software\\Classes\\.torrent\n    REG_SZ    rqbit.Torrent\n";
        assert_eq!(parse_reg_query(wine, None).as_deref(), Some("rqbit.Torrent"));
        let named = "HKEY_CURRENT_USER\\...\\UserChoice\r\n    ProgId    REG_SZ    rqbit.Magnet\r\n    Hash    REG_SZ    abc=\r\n";
        assert_eq!(parse_reg_query(named, Some("ProgId")).as_deref(), Some("rqbit.Magnet"));
        assert_eq!(parse_reg_query(named, Some("Missing")), None);
        let unset = "HKEY_CLASSES_ROOT\\.torrent\r\n    (Default)    REG_SZ    (value not set)\r\n";
        assert_eq!(parse_reg_query(unset, None), None);
    }

    #[test]
    fn status_text() {
        let s = Status { registered: true, magnet_default: false, torrent_default: false };
        assert!(!s.is_default());
        assert!(s.describe().contains("registered"));
        let d = Status { registered: true, magnet_default: true, torrent_default: true };
        assert!(d.is_default());
    }

    /// The manual-setup script must write the same keys as the app.
    #[test]
    fn powershell_script_matches() {
        let ps = include_str!("../../packaging/windows/register-handlers.ps1");
        for v in windows_values(&win()) {
            let key = v
                .key
                .replace(r"Software\Classes\", "$Classes\\")
                .replace(r"Software\rqbit\Capabilities", "$Caps");
            let key = if key.starts_with('$') {
                key
            } else {
                format!("HKCU:\\{key}")
            };
            assert!(ps.contains(&key), "script lacks key {key}");
            if let Some(n) = &v.name {
                assert!(ps.contains(&format!("\"{n}\"")), "script lacks value {n}");
            }
            if !v.data.is_empty() && !v.data.contains("rqbit-gpui.exe") {
                assert!(ps.contains(&v.data), "script lacks data {}", v.data);
            }
        }
    }
}
