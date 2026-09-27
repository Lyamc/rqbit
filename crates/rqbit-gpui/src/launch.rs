//! Things the app was asked to open: a magnet link or .torrent path on the
//! command line (the OS default-handler launch), forwarded from a second
//! instance, or the browser's `#add=<magnet>` fragment (registered with
//! `navigator.registerProtocolHandler`). They are queued on a channel that
//! the main window drains into the Add panel.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchItem {
    /// magnet: or http(s) URL (staged in the Add panel's URL list).
    Link(String),
    /// A local .torrent file (native only).
    File(PathBuf),
}

impl LaunchItem {
    /// The string form (forwarded to a running instance, which reclassifies it).
    pub fn as_arg(&self) -> String {
        match self {
            LaunchItem::Link(s) => s.clone(),
            LaunchItem::File(p) => p.to_string_lossy().into_owned(),
        }
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// What a command line argument / forwarded string asks to open.
pub fn classify(arg: &str) -> Option<LaunchItem> {
    let a = arg.trim();
    if a.is_empty() {
        return None;
    }
    if starts_with_ci(a, "magnet:") {
        return Some(LaunchItem::Link(a.to_owned()));
    }
    if starts_with_ci(a, "http://") || starts_with_ci(a, "https://") {
        return Some(LaunchItem::Link(a.to_owned()));
    }
    if starts_with_ci(a, "file://") {
        let path = file_url_to_path(a)?;
        return Some(LaunchItem::File(path));
    }
    if a.to_ascii_lowercase().ends_with(".torrent") {
        return Some(LaunchItem::File(PathBuf::from(a)));
    }
    None
}

#[cfg(not(target_family = "wasm"))]
fn file_url_to_path(u: &str) -> Option<PathBuf> {
    url::Url::parse(u).ok()?.to_file_path().ok()
}

#[cfg(target_family = "wasm")]
fn file_url_to_path(_: &str) -> Option<PathBuf> {
    None
}

/// The link in a `#add=<url-encoded link>` fragment (web protocol handler).
/// Only magnet / http(s) links are accepted.
pub fn parse_add_fragment(hash: &str) -> Option<String> {
    let frag = hash.trim_start_matches('#');
    let (_, v) = url::form_urlencoded::parse(frag.as_bytes()).find(|(k, _)| k == "add")?;
    match classify(&v)? {
        LaunchItem::Link(l) => Some(l),
        LaunchItem::File(_) => None,
    }
}

type Chan = (
    UnboundedSender<Vec<LaunchItem>>,
    Mutex<Option<UnboundedReceiver<Vec<LaunchItem>>>>,
);

fn chan() -> &'static Chan {
    static CHAN: OnceLock<Chan> = OnceLock::new();
    CHAN.get_or_init(|| {
        let (tx, rx) = unbounded();
        (tx, Mutex::new(Some(rx)))
    })
}

/// Queue items for the main window (no-op for an empty list).
pub fn send(items: Vec<LaunchItem>) {
    if !items.is_empty() {
        let _ = chan().0.unbounded_send(items);
    }
}

/// Queue items; an empty list just raises the window (second launch
/// without arguments).
#[cfg(not(target_family = "wasm"))]
pub fn send_or_activate(items: Vec<LaunchItem>) {
    let _ = chan().0.unbounded_send(items);
}

/// The receiving end (taken once, by the first main window).
pub fn take_receiver() -> Option<UnboundedReceiver<Vec<LaunchItem>>> {
    chan().1.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Browser: queue the page's `#add=` link (if any) and drop the fragment so
/// a reload doesn't stage it again.
#[cfg(target_family = "wasm")]
pub fn queue_page_fragment() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let Ok(hash) = location.hash() else { return };
    if let Some(link) = parse_add_fragment(&hash) {
        send(vec![LaunchItem::Link(link)]);
        if let (Ok(history), Ok(path), Ok(search)) =
            (window.history(), location.pathname(), location.search())
        {
            let _ = history.replace_state_with_url(
                &wasm_bindgen::JsValue::NULL,
                "",
                Some(&format!("{path}{search}")),
            );
        }
    }
}

/// Browser: `navigator.registerProtocolHandler('magnet', <this page>#add=%s)`.
/// Browsers only allow this from a secure (https / localhost) page and ask the
/// user to confirm.
#[cfg(target_family = "wasm")]
pub fn register_browser_magnet_handler() -> Result<String, String> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();
    let origin = location.origin().map_err(|_| "no origin")?;
    let path = location.pathname().map_err(|_| "no path")?;
    let target = format!("{origin}{path}#add=%s");
    let nav: wasm_bindgen::JsValue = window.navigator().into();
    let f = js_sys::Reflect::get(&nav, &"registerProtocolHandler".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or("This browser doesn't support registering protocol handlers.")?;
    f.call2(&nav, &"magnet".into(), &target.clone().into())
        .map_err(|e| {
            format!(
                "The browser refused: {}",
                e.as_string()
                    .or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok()?.as_string())
                    .unwrap_or_else(|| "unknown error".into())
            )
        })?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_args() {
        let m = "magnet:?xt=urn:btih:abc&dn=A+B";
        assert_eq!(classify(m), Some(LaunchItem::Link(m.into())));
        assert_eq!(
            classify(" MAGNET:?xt=urn:btih:abc "),
            Some(LaunchItem::Link("MAGNET:?xt=urn:btih:abc".into()))
        );
        assert_eq!(
            classify("https://x/y.torrent"),
            Some(LaunchItem::Link("https://x/y.torrent".into()))
        );
        assert_eq!(
            classify("/home/l/Downloads/a b.TORRENT"),
            Some(LaunchItem::File("/home/l/Downloads/a b.TORRENT".into()))
        );
        assert_eq!(
            classify("C:\\Users\\Lyam\\x.torrent"),
            Some(LaunchItem::File("C:\\Users\\Lyam\\x.torrent".into()))
        );
        assert_eq!(classify("--url"), None);
        assert_eq!(classify("notes.txt"), None);
        assert_eq!(classify(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_urls_are_decoded() {
        assert_eq!(
            classify("file:///tmp/a%20b.torrent"),
            Some(LaunchItem::File("/tmp/a b.torrent".into()))
        );
    }

    #[test]
    fn add_fragment() {
        // registerProtocolHandler substitutes the escaped link for %s.
        assert_eq!(
            parse_add_fragment("#add=magnet%3A%3Fxt%3Durn%3Abtih%3Aabc%26dn%3DA%2BB").as_deref(),
            Some("magnet:?xt=urn:btih:abc&dn=A+B")
        );
        assert_eq!(
            parse_add_fragment("add=https%3A%2F%2Fx%2Fy.torrent").as_deref(),
            Some("https://x/y.torrent")
        );
        assert_eq!(parse_add_fragment("#add=javascript%3Aalert(1)"), None);
        assert_eq!(parse_add_fragment("#add=%2Fetc%2Fpasswd.torrent"), None);
        assert_eq!(parse_add_fragment("#other=1"), None);
        assert_eq!(parse_add_fragment(""), None);
    }

    #[test]
    fn channel_delivers_once() {
        send(vec![LaunchItem::Link("magnet:?x".into())]);
        send(Vec::new());
        let mut rx = take_receiver().expect("receiver");
        assert!(take_receiver().is_none());
        let got = rx.try_recv().unwrap();
        assert_eq!(got, vec![LaunchItem::Link("magnet:?x".into())]);
        assert!(rx.try_recv().is_err()); // empty
    }
}
