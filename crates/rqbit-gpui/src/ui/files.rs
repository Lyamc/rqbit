//! Getting .torrent files into the Add panel.
//!
//! - Native: GPUI's file picker (`prompt_for_paths`) and OS drag-and-drop
//!   (`ExternalPaths`); files are read from disk.
//! - Browser: GPUI's web platform has neither (browsers hand out `File`
//!   objects, never paths), so this module drives a hidden
//!   `<input type="file">` and a window-level `drop` listener itself and
//!   forwards the file contents over a channel.

/// A .torrent (or other) file picked or dropped by the user.
pub struct PickedFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Max size read for one file (the server's default upload limit is 10 MiB).
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

#[cfg(not(target_family = "wasm"))]
pub fn read_paths(paths: &[std::path::PathBuf]) -> Vec<Result<PickedFile, (String, String)>> {
    paths
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            let meta = std::fs::metadata(p).map_err(|e| (name.clone(), e.to_string()))?;
            if meta.is_dir() {
                return Err((name, "is a folder".into()));
            }
            if meta.len() > MAX_FILE_SIZE {
                return Err((name, "file too large for a .torrent".into()));
            }
            std::fs::read(p)
                .map(|bytes| PickedFile {
                    name: name.clone(),
                    bytes,
                })
                .map_err(|e| (name, e.to_string()))
        })
        .collect()
}

#[cfg(target_family = "wasm")]
pub use web::{open_picker, web_files_channel};

#[cfg(target_family = "wasm")]
mod web {
    use std::cell::RefCell;

    use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
    use wasm_bindgen::{JsCast, prelude::*};

    use super::{MAX_FILE_SIZE, PickedFile};

    thread_local! {
        static SINK: RefCell<Option<UnboundedSender<Vec<PickedFile>>>> = const { RefCell::new(None) };
        static INPUT: RefCell<Option<web_sys::HtmlInputElement>> = const { RefCell::new(None) };
    }

    fn send(files: Vec<PickedFile>) {
        SINK.with(|s| {
            if let Some(tx) = s.borrow().as_ref() {
                let _ = tx.unbounded_send(files);
            }
        });
    }

    fn read_file_list(list: web_sys::FileList) {
        let files: Vec<web_sys::File> = (0..list.length()).filter_map(|i| list.get(i)).collect();
        if files.is_empty() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let mut out = Vec::new();
            for f in files {
                let name = f.name();
                if f.size() > MAX_FILE_SIZE as f64 {
                    log::warn!("skipping {name}: too large for a .torrent");
                    continue;
                }
                match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
                    Ok(buf) => out.push(PickedFile {
                        name,
                        bytes: js_sys::Uint8Array::new(&buf).to_vec(),
                    }),
                    Err(e) => log::warn!("error reading {name}: {e:?}"),
                }
            }
            send(out);
        });
    }

    /// Channel of files chosen with [`open_picker`] or dropped on the page.
    /// Installs the page-level drop listener; call once.
    pub fn web_files_channel() -> UnboundedReceiver<Vec<PickedFile>> {
        let (tx, rx) = unbounded();
        SINK.with(|s| *s.borrow_mut() = Some(tx));
        if let Some(window) = web_sys::window() {
            let dragover = Closure::<dyn FnMut(web_sys::Event)>::new(|e: web_sys::Event| {
                e.prevent_default();
            });
            let drop = Closure::<dyn FnMut(web_sys::DragEvent)>::new(|e: web_sys::DragEvent| {
                e.prevent_default();
                if let Some(list) = e.data_transfer().and_then(|dt| dt.files()) {
                    read_file_list(list);
                }
            });
            let _ = window
                .add_event_listener_with_callback("dragover", dragover.as_ref().unchecked_ref());
            let _ = window.add_event_listener_with_callback("drop", drop.as_ref().unchecked_ref());
            dragover.forget();
            drop.forget();
        }
        rx
    }

    fn input() -> Option<web_sys::HtmlInputElement> {
        if let Some(i) = INPUT.with(|i| i.borrow().clone()) {
            return Some(i);
        }
        let document = web_sys::window()?.document()?;
        let input: web_sys::HtmlInputElement =
            document.create_element("input").ok()?.dyn_into().ok()?;
        input.set_type("file");
        input.set_multiple(true);
        input.set_accept(".torrent,application/x-bittorrent");
        let _ = input.style().set_property("display", "none");
        let on_change = {
            let input = input.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
                if let Some(list) = input.files() {
                    read_file_list(list);
                }
                // Allow picking the same file again.
                input.set_value("");
            })
        };
        input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
        on_change.forget();
        document.body()?.append_child(&input).ok()?;
        INPUT.with(|i| *i.borrow_mut() = Some(input.clone()));
        Some(input)
    }

    /// Opens the browser's file chooser (must run during a user gesture,
    /// e.g. a click handler). Chosen files arrive on [`web_files_channel`].
    pub fn open_picker() {
        match input() {
            Some(i) => i.click(),
            None => log::error!("could not create the file input"),
        }
    }
}
