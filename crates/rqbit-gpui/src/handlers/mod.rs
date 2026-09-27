//! Native only: default-handler registration ([`register`]) and single
//! instance forwarding. A second `rqbit-gpui <magnet|file.torrent>` hands its
//! arguments to the running instance over a loopback socket (port + random
//! token in `rqbit-gpui/instance.json` in the user's config directory) and
//! exits; the running window shows them in the Add panel.

pub mod register;

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::launch::{self, LaunchItem};

#[derive(Serialize, Deserialize)]
struct InstanceFile {
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct ForwardMsg {
    token: String,
    items: Vec<String>,
}

fn instance_path() -> Option<PathBuf> {
    Some(crate::store::dir()?.join("instance.json"))
}

fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut s = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s
}

/// Makes relative .torrent paths absolute (the running instance has another cwd).
fn absolutize(items: &[LaunchItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            LaunchItem::File(p) if p.is_relative() => std::env::current_dir()
                .map(|d| d.join(p).to_string_lossy().into_owned())
                .unwrap_or_else(|_| i.as_arg()),
            _ => i.as_arg(),
        })
        .collect()
}

/// Sends `items` to a running instance. `true` if it accepted them.
pub fn forward(items: &[LaunchItem]) -> bool {
    let Some(f) = instance_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<InstanceFile>(&b).ok())
    else {
        return false;
    };
    forward_to(f.port, &f.token, &absolutize(items)).is_ok()
}

fn forward_to(port: u16, token: &str, items: &[String]) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let msg = ForwardMsg {
        token: token.to_owned(),
        items: items.to_vec(),
    };
    let mut line = serde_json::to_string(&msg)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("rejected: {}", reply.trim())))
    }
}

/// Handles one forwarding connection; returns the accepted items.
fn handle_conn(stream: TcpStream, token: &str) -> Option<Vec<LaunchItem>> {
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut w = stream.try_clone().ok()?;
    let mut line = String::new();
    // Bounded read: at most 1 MiB.
    BufReader::new(std::io::Read::take(&stream, 1 << 20))
        .read_line(&mut line)
        .ok()?;
    let msg: ForwardMsg = match serde_json::from_str(line.trim()) {
        Ok(m) => m,
        Err(_) => {
            let _ = w.write_all(b"bad request\n");
            return None;
        }
    };
    if msg.token != token {
        let _ = w.write_all(b"bad token\n");
        return None;
    }
    let _ = w.write_all(b"ok\n");
    Some(msg.items.iter().filter_map(|s| launch::classify(s)).collect())
}

/// Starts accepting forwarded launches (background thread) and records the
/// port for later instances. Failures only disable forwarding.
pub fn listen() {
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
        return;
    };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else {
        return;
    };
    let token = random_token();
    let Some(path) = instance_path() else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let file = InstanceFile {
        port,
        token: token.clone(),
    };
    if serde_json::to_vec(&file)
        .ok()
        .and_then(|b| std::fs::write(&path, b).ok())
        .is_none()
    {
        return;
    }
    std::thread::Builder::new()
        .name("rqbit-gpui-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(items) = handle_conn(stream, &token) {
                    // An empty list still raises the window.
                    launch::send_or_activate(items);
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarding_round_trip() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..2 {
                let (s, _) = listener.accept().unwrap();
                got.push(handle_conn(s, "secret"));
            }
            got
        });
        let items = vec![
            "magnet:?xt=urn:btih:abc".to_owned(),
            "/tmp/x.torrent".to_owned(),
            "junk".to_owned(),
        ];
        assert!(forward_to(port, "wrong", &items).is_err());
        forward_to(port, "secret", &items).unwrap();
        let got = t.join().unwrap();
        assert!(got[0].is_none());
        assert_eq!(
            got[1].clone().unwrap(),
            vec![
                LaunchItem::Link("magnet:?xt=urn:btih:abc".into()),
                LaunchItem::File("/tmp/x.torrent".into()),
            ]
        );
    }

    #[test]
    fn relative_paths_are_absolutized() {
        let v = absolutize(&[
            LaunchItem::File("rel/a.torrent".into()),
            LaunchItem::Link("magnet:?x".into()),
        ]);
        assert!(PathBuf::from(&v[0]).is_absolute());
        assert!(v[0].ends_with("a.torrent"));
        assert_eq!(v[1], "magnet:?x");
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }
}
