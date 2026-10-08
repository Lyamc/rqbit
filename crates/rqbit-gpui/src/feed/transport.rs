//! Gets the torrent list to the UI: a WebSocket on `GET /stream/torrents` (snapshot,
//! then deltas, raw-deflate compressed), else `?since=` delta polling over the normal
//! HTTP client. The WebSocket is retried with backoff while polling. Callers only
//! see [`TorrentFeed::next`], one full list per update.
//!
//! - native: a blocking `tungstenite` socket on its own thread (GPUI has no tokio),
//!   with basic auth from the server URL; `wss://` needs a TLS feature
//!   (`native-tls` / `rustls`), else it falls back to polling;
//! - browser: `web_sys::WebSocket`; the browser adds its cookies / basic-auth login.

use std::time::Duration;

use futures::future::LocalBoxFuture;
use futures::{FutureExt, StreamExt};
use serde_json::Value;

use super::{Apply, FeedClient, Inflater, parse_lines};
use crate::api::{ApiClient, ApiFuture, TorrentListItem};
use crate::time::Instant;

/// What the feed needs from an executor (GPUI's background executor; tests use
/// threads).
pub trait Exec {
    fn timer(&self, d: Duration) -> LocalBoxFuture<'static, ()>;
    /// Runs an API request (native ones block while polled: off the UI thread).
    fn run<T: Send + 'static>(&self, f: ApiFuture<T>)
    -> LocalBoxFuture<'static, anyhow::Result<T>>;
}

impl Exec for gpui::BackgroundExecutor {
    fn timer(&self, d: Duration) -> LocalBoxFuture<'static, ()> {
        gpui::BackgroundExecutor::timer(self, d).boxed_local()
    }
    fn run<T: Send + 'static>(
        &self,
        f: ApiFuture<T>,
    ) -> LocalBoxFuture<'static, anyhow::Result<T>> {
        self.spawn(f).boxed_local()
    }
}

/// Same pace as the old polling loop.
pub const TICK: Duration = Duration::from_secs(1);
/// The server sends something at least every 15 s (heartbeat).
const WATCHDOG: Duration = Duration::from_secs(40);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WS_BACKOFF_MAX: Duration = Duration::from_secs(60);

pub enum WsEvent {
    Open,
    /// Decoded text: one or more JSON lines.
    Data(Vec<u8>),
    Closed(String),
}

enum Mode {
    Idle,
    Ws(ws::Socket),
    Poll,
}

pub struct TorrentFeed {
    client: ApiClient,
    feed: FeedClient,
    mode: Mode,
    ws_failures: u32,
    next_ws_try: Option<Instant>,
    /// The current socket has sent something.
    ws_live: bool,
    last_poll: Option<Instant>,
}

impl TorrentFeed {
    pub fn new(client: ApiClient) -> Self {
        Self {
            client,
            feed: FeedClient::default(),
            mode: Mode::Idle,
            ws_failures: 0,
            next_ws_try: None,
            ws_live: false,
            last_poll: None,
        }
    }

    /// "websocket" / "polling" (logs).
    pub fn transport(&self) -> &'static str {
        match self.mode {
            Mode::Ws(_) => "websocket",
            _ => "polling",
        }
    }

    fn ws_failed(&mut self, why: &str) {
        self.ws_failures = self.ws_failures.saturating_add(1);
        let backoff = Duration::from_secs(1u64 << self.ws_failures.min(6)).min(WS_BACKOFF_MAX);
        log::info!(
            "torrent list: WebSocket unavailable ({why}); polling, retrying in {}s",
            backoff.as_secs()
        );
        self.next_ws_try = Some(Instant::now() + backoff);
        self.mode = Mode::Poll;
    }

    /// The next list state. An error means the server couldn't be reached (the
    /// caller shows it and calls again).
    pub async fn next(&mut self, bg: &impl Exec) -> anyhow::Result<Vec<TorrentListItem>> {
        loop {
            match &mut self.mode {
                Mode::Idle => {
                    let due = self.next_ws_try.is_none_or(|t| Instant::now() >= t);
                    match self.client.stream_url() {
                        Ok(url) if due => {
                            match ws::Socket::connect(url, self.client.basic_auth()) {
                                Ok(sock) => {
                                    // The server starts with a snapshot; until then the
                                    // current state stays (and is polled from if this fails).
                                    self.ws_live = false;
                                    self.mode = Mode::Ws(sock);
                                }
                                Err(e) => self.ws_failed(&format!("{e:#}")),
                            }
                        }
                        _ => self.mode = Mode::Poll,
                    }
                }
                Mode::Ws(sock) => {
                    let wait = if self.ws_live {
                        WATCHDOG
                    } else {
                        CONNECT_TIMEOUT
                    };
                    let ev = futures::select! {
                        ev = sock.rx.next() => ev,
                        _ = bg.timer(wait).fuse() => Some(WsEvent::Closed("no messages".into())),
                    };
                    match ev {
                        Some(WsEvent::Open) => {}
                        Some(WsEvent::Data(text)) => {
                            self.ws_live = true;
                            let mut updated = false;
                            let mut broken = None;
                            for msg in parse_lines(&text) {
                                match msg.map(|m| self.feed.apply(&m)) {
                                    Ok(Apply::Updated) => updated = true,
                                    Ok(Apply::Unchanged) => {}
                                    Ok(Apply::Gap) => sock.send(r#"{"type":"resync"}"#),
                                    Ok(Apply::Invalid) => {
                                        broken = Some("invalid message".to_owned())
                                    }
                                    Err(e) => broken = Some(format!("{e:#}")),
                                }
                            }
                            if let Some(why) = broken {
                                self.ws_failed(&why);
                            } else if updated {
                                self.ws_failures = 0;
                                self.next_ws_try = None;
                                return Ok(self.feed.torrents());
                            }
                        }
                        Some(WsEvent::Closed(why)) => self.ws_failed(&why),
                        None => self.ws_failed("closed"),
                    }
                }
                Mode::Poll => {
                    if self.next_ws_try.is_some_and(|t| Instant::now() >= t) {
                        self.mode = Mode::Idle;
                        continue;
                    }
                    if let Some(last) = self.last_poll {
                        let since = last.elapsed();
                        if since < TICK {
                            bg.timer(TICK - since).await;
                        }
                    }
                    self.last_poll = Some(Instant::now());
                    let since = self.feed.epoch.clone().map(|e| (self.feed.seq, e));
                    let msg: Value = bg.run(self.client.poll_torrent_list(since)).await?;
                    match self.feed.apply(&msg) {
                        Apply::Updated | Apply::Unchanged => return Ok(self.feed.torrents()),
                        Apply::Gap => {
                            self.feed.reset();
                            self.last_poll = None;
                        }
                        Apply::Invalid => anyhow::bail!("unexpected torrent list response"),
                    }
                }
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
mod ws {
    //! Native: blocking tungstenite on a thread, events over a channel.
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use anyhow::Context;
    use futures::channel::mpsc;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::{HandshakeError, Message};

    use super::{Inflater, WsEvent};

    pub struct Socket {
        pub rx: mpsc::UnboundedReceiver<WsEvent>,
        cmd: std::sync::mpsc::Sender<String>,
        stop: Arc<AtomicBool>,
    }

    impl Drop for Socket {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    impl Socket {
        pub fn connect(url: url::Url, auth: Option<(String, String)>) -> anyhow::Result<Self> {
            let (tx, rx) = mpsc::unbounded();
            let (cmd, cmd_rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = stop.clone();
            std::thread::Builder::new()
                .name("rqbit-list-ws".into())
                .spawn(move || {
                    let why = match run(url, auth, &tx, &cmd_rx, &stop2) {
                        Ok(()) => "closed".to_owned(),
                        Err(e) => format!("{e:#}"),
                    };
                    let _ = tx.unbounded_send(WsEvent::Closed(why));
                })
                .context("spawning WebSocket thread")?;
            Ok(Self { rx, cmd, stop })
        }

        pub fn send(&self, text: &str) {
            let _ = self.cmd.send(text.to_owned());
        }
    }

    fn run(
        url: url::Url,
        auth: Option<(String, String)>,
        tx: &mpsc::UnboundedSender<WsEvent>,
        cmd: &std::sync::mpsc::Receiver<String>,
        stop: &AtomicBool,
    ) -> anyhow::Result<()> {
        let host = url.host_str().context("no host")?.to_owned();
        let port = url.port_or_known_default().context("no port")?;
        let mut req = url.as_str().into_client_request()?;
        if let Some((u, p)) = auth {
            let v = format!(
                "Basic {}",
                crate::api::base64_encode(format!("{u}:{p}").as_bytes())
            );
            req.headers_mut().insert("Authorization", v.parse()?);
        }
        req.headers_mut().insert(
            "User-Agent",
            concat!("rqbit-gpui/", env!("CARGO_PKG_VERSION")).parse()?,
        );
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let mut last_err = anyhow::anyhow!("no address for {host}");
        let mut stream = None;
        for addr in (host.as_str(), port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last_err = e.into(),
            }
        }
        let stream = stream.ok_or(last_err)?;
        stream.set_nodelay(true).ok();
        let tcp = stream.try_clone()?;
        tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
        #[cfg(any(feature = "native-tls", feature = "rustls"))]
        let handshake = tungstenite::client_tls_with_config(req, stream, None, None);
        #[cfg(not(any(feature = "native-tls", feature = "rustls")))]
        let handshake = {
            if url.scheme() == "wss" {
                anyhow::bail!("built without TLS for wss://");
            }
            tungstenite::client::client_with_config(req, stream, None)
        };
        let (ws, _) = match handshake {
            Ok(x) => x,
            Err(HandshakeError::Failure(e)) => return Err(e.into()),
            Err(HandshakeError::Interrupted(_)) => anyhow::bail!("handshake timed out"),
        };
        // Short reads from here on, to notice `stop` and pending commands.
        tcp.set_read_timeout(Some(Duration::from_millis(250)))?;
        let _ = tx.unbounded_send(WsEvent::Open);
        pump(ws, tx, cmd, stop)
    }

    fn pump<S: std::io::Read + std::io::Write>(
        mut ws: tungstenite::WebSocket<S>,
        tx: &mpsc::UnboundedSender<WsEvent>,
        cmd: &std::sync::mpsc::Receiver<String>,
        stop: &AtomicBool,
    ) -> anyhow::Result<()> {
        let mut inflater = Inflater::default();
        loop {
            if stop.load(Ordering::Relaxed) || tx.is_closed() {
                let _ = ws.close(None);
                let _ = ws.flush();
                return Ok(());
            }
            while let Ok(c) = cmd.try_recv() {
                ws.send(Message::text(c))?;
            }
            match ws.read() {
                Ok(Message::Text(t)) => {
                    let _ = tx.unbounded_send(WsEvent::Data(t.as_bytes().to_vec()));
                }
                Ok(Message::Binary(b)) => {
                    let _ = tx.unbounded_send(WsEvent::Data(inflater.message(&b)?));
                }
                Ok(Message::Close(_)) => anyhow::bail!("closed by the server"),
                Ok(_) => {} // ping/pong: answered by tungstenite
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    // Sends any queued pong.
                    match ws.flush() {
                        Ok(()) => {}
                        Err(tungstenite::Error::Io(e))
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(target_family = "wasm")]
mod ws {
    //! Browser: `web_sys::WebSocket`, events over a channel.
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::channel::mpsc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    use super::{Inflater, WsEvent};

    pub struct Socket {
        pub rx: mpsc::UnboundedReceiver<WsEvent>,
        ws: web_sys::WebSocket,
        _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
        _on_open: Closure<dyn FnMut(web_sys::Event)>,
        _on_close: Closure<dyn FnMut(web_sys::CloseEvent)>,
        _on_error: Closure<dyn FnMut(web_sys::Event)>,
    }

    impl Drop for Socket {
        fn drop(&mut self) {
            self.ws.set_onmessage(None);
            self.ws.set_onopen(None);
            self.ws.set_onclose(None);
            self.ws.set_onerror(None);
            let _ = self.ws.close();
        }
    }

    impl Socket {
        /// The browser adds its credentials; `auth` from the URL isn't usable here.
        pub fn connect(url: url::Url, _auth: Option<(String, String)>) -> anyhow::Result<Self> {
            let ws = web_sys::WebSocket::new(url.as_str())
                .map_err(|e| anyhow::anyhow!("WebSocket: {e:?}"))?;
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            let (tx, rx) = mpsc::unbounded();
            let inflater = Rc::new(RefCell::new(Inflater::default()));
            let on_message = {
                let tx = tx.clone();
                Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                    let data = e.data();
                    let ev = if let Some(s) = data.as_string() {
                        WsEvent::Data(s.into_bytes())
                    } else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
                        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                        match inflater.borrow_mut().message(&bytes) {
                            Ok(d) => WsEvent::Data(d),
                            Err(e) => WsEvent::Closed(format!("{e:#}")),
                        }
                    } else {
                        return;
                    };
                    let _ = tx.unbounded_send(ev);
                })
            };
            let on_open = {
                let tx = tx.clone();
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                    let _ = tx.unbounded_send(WsEvent::Open);
                })
            };
            let on_close = {
                let tx = tx.clone();
                Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |e: web_sys::CloseEvent| {
                    let _ = tx.unbounded_send(WsEvent::Closed(format!("closed ({})", e.code())));
                })
            };
            let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                let _ = tx.unbounded_send(WsEvent::Closed("WebSocket error".into()));
            });
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            Ok(Self {
                rx,
                ws,
                _on_message: on_message,
                _on_open: on_open,
                _on_close: on_close,
                _on_error: on_error,
            })
        }

        pub fn send(&self, text: &str) {
            let _ = self.ws.send_with_str(text);
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "transport_tests.rs"]
mod tests;
