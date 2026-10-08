//! `TorrentFeed` against a fake server (std threads, no rqbit): WebSocket with
//! raw-deflate frames, gap → resync, a dropped socket → polling → reconnect, and a
//! proxy that refuses the Upgrade → delta polling. Data: the golden feed messages.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use serde_json::{Value, json};

use super::{Exec, TorrentFeed};
use crate::api::{
    ApiClient, ApiFuture, ListTorrentsResponse, TorrentListItem, Transport, parse_base_url,
};
use crate::feed::FeedClient;

struct ThreadExec;

impl Exec for ThreadExec {
    fn timer(&self, d: Duration) -> LocalBoxFuture<'static, ()> {
        let (tx, rx) = futures::channel::oneshot::channel::<()>();
        std::thread::spawn(move || {
            std::thread::sleep(d);
            let _ = tx.send(());
        });
        rx.map(|_| ()).boxed_local()
    }
    fn run<T: Send + 'static>(
        &self,
        f: ApiFuture<T>,
    ) -> LocalBoxFuture<'static, anyhow::Result<T>> {
        f.boxed_local()
    }
}

fn fixtures() -> (Vec<Value>, Vec<Value>) {
    let s: Value = serde_json::from_str(super::super::tests::SNAPSHOTS).unwrap();
    let m: Value = serde_json::from_str(super::super::tests::MESSAGES).unwrap();
    (
        s["snapshots"].as_array().unwrap().clone(),
        m["messages"].as_array().unwrap().clone(),
    )
}

/// A snapshot message of the state after `messages[..=i]`.
fn snapshot_at(messages: &[Value], i: usize) -> Value {
    let mut c = FeedClient::default();
    for m in &messages[..=i] {
        c.apply(m);
    }
    json!({"type": "snapshot", "proto": 1, "epoch": c.epoch, "seq": c.seq,
           "torrents": c.lean_rows()})
}

fn names(list: &[TorrentListItem]) -> Vec<(usize, Option<String>, String)> {
    list.iter()
        .map(|t| {
            let speed = t
                .stats
                .as_ref()
                .and_then(|s| s.live.as_ref())
                .and_then(|l| l.download_speed.human_readable.clone())
                .unwrap_or_default();
            (t.id, t.name.clone(), speed)
        })
        .collect()
}

fn full(snapshot: &Value) -> Vec<(usize, Option<String>, String)> {
    let r: ListTorrentsResponse = serde_json::from_value(snapshot.clone()).unwrap();
    names(&r.torrents)
}

#[derive(Clone, Copy, PartialEq)]
enum WsPlan {
    /// snapshot, delta 1, then a delta that skips one (gap), then on `resync` the
    /// latest snapshot.
    GapThenResync,
    /// First connection: snapshot + delta 1, then closes. Later ones: latest snapshot.
    DropThenReconnect,
    /// The "proxy" answers 403 to Upgrade requests.
    Refuse,
}

#[derive(Default)]
struct Seen {
    ws_connections: AtomicUsize,
    polls: Mutex<Vec<String>>,
    client_texts: Mutex<Vec<String>>,
    deflate: AtomicUsize,
}

fn serve(plan: WsPlan, messages: Vec<Value>) -> (String, Arc<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    let seen = Arc::new(Seen::default());
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            let (seen, messages) = (seen2.clone(), messages.clone());
            std::thread::spawn(move || handle(s, plan, &messages, &seen));
        }
    });
    (url, seen)
}

fn handle(s: TcpStream, plan: WsPlan, messages: &[Value], seen: &Seen) {
    let mut buf = [0u8; 4096];
    let n = s.peek(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    if head.contains("upgrade: websocket") {
        if plan == WsPlan::Refuse {
            let mut s = s;
            let _ = s.write_all(
                b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
            return;
        }
        let nth = seen.ws_connections.fetch_add(1, Ordering::SeqCst);
        let deflate = path.contains("enc=deflate");
        if deflate {
            seen.deflate.fetch_add(1, Ordering::SeqCst);
        }
        let mut ws = tungstenite::accept(s).unwrap();
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        let mut send = |ws: &mut tungstenite::WebSocket<TcpStream>, v: &Value| {
            let text = format!("{v}\n");
            let msg = if deflate {
                enc.write_all(text.as_bytes()).unwrap();
                enc.flush().unwrap();
                tungstenite::Message::binary(std::mem::take(enc.get_mut()))
            } else {
                tungstenite::Message::text(text)
            };
            ws.send(msg).unwrap();
        };
        let last = messages.len() - 1;
        match (plan, nth) {
            (WsPlan::DropThenReconnect, 0) => {
                send(&mut ws, &messages[0]);
                send(&mut ws, &messages[1]);
                std::thread::sleep(Duration::from_millis(100));
                let _ = ws.close(None);
                let _ = ws.flush();
                return;
            }
            (WsPlan::DropThenReconnect, _) => send(&mut ws, &snapshot_at(messages, last)),
            (WsPlan::GapThenResync, _) => {
                send(&mut ws, &messages[0]);
                send(&mut ws, &messages[1]);
                send(&mut ws, &messages[3]); // base 3, client is at 2
            }
            (WsPlan::Refuse, _) => unreachable!(),
        }
        // Answer resyncs, keep the socket open.
        loop {
            match ws.read() {
                Ok(tungstenite::Message::Text(t)) => {
                    seen.client_texts.lock().unwrap().push(t.to_string());
                    if t.contains("resync") {
                        send(&mut ws, &snapshot_at(messages, last));
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    }
    // Plain HTTP: the polling fallback.
    let mut s = s;
    let _ = s.read(&mut buf);
    seen.polls.lock().unwrap().push(path.clone());
    let since = path
        .split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("since="))
        .and_then(|v| v.parse::<usize>().ok());
    let body = match since {
        None => messages[0].clone(),
        Some(seq) if seq < messages.len() => messages[seq].clone(),
        Some(_) => snapshot_at(messages, messages.len() - 1),
    }
    .to_string();
    let _ = write!(
        s,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn feed(url: &str) -> TorrentFeed {
    TorrentFeed::new(ApiClient::new(
        Transport::new().unwrap(),
        parse_base_url(url).unwrap(),
    ))
}

fn next(f: &mut TorrentFeed) -> Vec<TorrentListItem> {
    futures::executor::block_on(f.next(&ThreadExec)).unwrap()
}

#[test]
fn websocket_deltas_and_resync_on_gap() {
    let (snapshots, messages) = fixtures();
    let (url, seen) = serve(WsPlan::GapThenResync, messages);
    let mut f = feed(&url);
    assert_eq!(names(&next(&mut f)), full(&snapshots[0]));
    assert_eq!(f.transport(), "websocket");
    assert_eq!(names(&next(&mut f)), full(&snapshots[1]));
    // The gap is not applied; the resync snapshot brings the latest state.
    assert_eq!(names(&next(&mut f)), full(&snapshots[3]));
    assert_eq!(f.transport(), "websocket");
    assert_eq!(seen.ws_connections.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.deflate.load(Ordering::SeqCst),
        1,
        "asks for enc=deflate"
    );
    assert_eq!(*seen.client_texts.lock().unwrap(), [r#"{"type":"resync"}"#]);
    assert!(seen.polls.lock().unwrap().is_empty());
}

#[test]
fn falls_back_to_delta_polling_when_the_upgrade_is_refused() {
    let (snapshots, messages) = fixtures();
    let (url, seen) = serve(WsPlan::Refuse, messages);
    let mut f = feed(&url);
    let started = std::time::Instant::now();
    for s in &snapshots {
        assert_eq!(names(&next(&mut f)), full(s));
        assert_eq!(f.transport(), "polling");
    }
    let polls = seen.polls.lock().unwrap().clone();
    assert_eq!(polls[0], "/stream/torrents");
    assert!(
        polls[1].starts_with("/stream/torrents?since=1&epoch="),
        "{polls:?}"
    );
    assert!(
        polls[3].starts_with("/stream/torrents?since=3&epoch="),
        "{polls:?}"
    );
    // Paced at the tick, like the old polling.
    assert!(
        started.elapsed() >= Duration::from_millis(2900),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn dropped_socket_polls_then_reconnects() {
    let (snapshots, messages) = fixtures();
    let (url, seen) = serve(WsPlan::DropThenReconnect, messages);
    let mut f = feed(&url);
    assert_eq!(names(&next(&mut f)), full(&snapshots[0]));
    assert_eq!(names(&next(&mut f)), full(&snapshots[1]));
    // The socket closes: polling continues from seq 2 (a delta, no snapshot)...
    assert_eq!(names(&next(&mut f)), full(&snapshots[2]));
    assert_eq!(f.transport(), "polling");
    assert!(seen.polls.lock().unwrap()[0].starts_with("/stream/torrents?since=2&"));
    // ...until the WebSocket is retried (2 s backoff) and gives the latest snapshot.
    let mut last = Vec::new();
    for _ in 0..8 {
        last = names(&next(&mut f));
        if f.transport() == "websocket" {
            break;
        }
    }
    assert_eq!(f.transport(), "websocket");
    assert_eq!(seen.ws_connections.load(Ordering::SeqCst), 2);
    assert_eq!(last, full(&snapshots[3]));
}

#[test]
fn unreachable_server_is_an_error() {
    // Nothing listens here (bound, then dropped).
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut f = feed(&format!("http://127.0.0.1:{port}/"));
    assert!(futures::executor::block_on(f.next(&ThreadExec)).is_err());
}

/// Against a running rqbit (throwaway e2e):
/// `RQBIT_TEST_URL=http://127.0.0.1:3033/ cargo test -p rqbit-gpui real_server -- --ignored`
/// With `RQBIT_TEST_EXPECT=polling` (e.g. through a proxy that blocks Upgrade) it
/// checks the fallback instead.
#[test]
#[ignore]
fn real_server() {
    let url = std::env::var("RQBIT_TEST_URL").expect("RQBIT_TEST_URL");
    let expect = std::env::var("RQBIT_TEST_EXPECT").unwrap_or_else(|_| "websocket".into());
    let client = ApiClient::new(Transport::new().unwrap(), parse_base_url(&url).unwrap());
    let mut f = TorrentFeed::new(client.clone());
    let t0 = std::time::Instant::now();
    let mut list = next(&mut f);
    eprintln!("first list after {:?} via {}", t0.elapsed(), f.transport());
    assert_eq!(f.transport(), expect);
    let full = futures::executor::block_on(client.list_torrents()).unwrap();
    let ids = |l: &[TorrentListItem]| l.iter().map(|t| (t.id, t.name.clone())).collect::<Vec<_>>();
    assert_eq!(ids(&list), ids(&full.torrents));
    // A few updates (on a busy server: deltas; idle: polling answers / heartbeats).
    let n: usize = std::env::var("RQBIT_TEST_UPDATES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    for _ in 0..n {
        list = next(&mut f);
        assert_eq!(f.transport(), expect);
    }
    let full = futures::executor::block_on(client.list_torrents()).unwrap();
    assert_eq!(ids(&list), ids(&full.torrents));
    eprintln!(
        "{} torrents, {} updates in {:?}",
        list.len(),
        n + 1,
        t0.elapsed()
    );
}
