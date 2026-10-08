use super::*;
use serde_json::json;

const SNAPSHOTS: &str = include_str!("testdata/list_snapshots.json");
const GOLDEN_PATH: &str = "src/list_feed/testdata/list_messages.json";

fn snapshots() -> Vec<Value> {
    let v: Value = serde_json::from_str(SNAPSHOTS).unwrap();
    v["snapshots"].as_array().unwrap().clone()
}

fn states() -> Vec<FeedState> {
    snapshots()
        .into_iter()
        .map(FeedState::from_list_value)
        .collect()
}

/// Snapshot of state 0, then a delta to each following state (what a WebSocket
/// client receives). The same messages are checked in by the web UI and GPUI tests.
fn messages() -> Vec<Value> {
    let st = states();
    let mut out = vec![snapshot_msg("test", 1, &st[0])];
    for i in 1..st.len() {
        out.push(delta_msg(
            "test",
            i as u64,
            i as u64 + 1,
            &st[i - 1],
            &st[i],
        ));
    }
    out
}

#[test]
fn lean_row_drops_unused_and_converts_speed_and_eta() {
    let full = snapshots()[0]["torrents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["stats"]["live"]["time_remaining"].is_object())
        .unwrap()
        .clone();
    let row = lean_row(full.clone());
    let live = &row["stats"]["live"];
    assert!(live.get("download_speed").is_none() && live.get("time_remaining").is_none());
    assert!(live.get("average_piece_download_time").is_none());
    let snap = &live["snapshot"];
    for k in DROP_SNAPSHOT {
        assert!(snap.get(*k).is_none(), "{k}");
    }
    for k in DROP_PEER_STATS {
        assert!(snap["peer_stats"].get(*k).is_none(), "{k}");
    }
    // Kept: everything the UIs read.
    for k in [
        "fetched_bytes",
        "uploaded_bytes",
        "downloaded_and_checked_pieces",
    ] {
        assert_eq!(snap[k], full["stats"]["live"]["snapshot"][k], "{k}");
    }
    for k in ["queued", "connecting", "live", "seen", "dead", "not_needed"] {
        assert_eq!(
            snap["peer_stats"][k],
            full["stats"]["live"]["snapshot"]["peer_stats"][k]
        );
    }
    // Lossless: bps / 2^20 is the server's mbps, eta_ms is the duration.
    let fl = &full["stats"]["live"];
    let bps = live["download_bps"].as_u64().unwrap();
    assert_eq!(
        bps as f64 / 1_048_576.0,
        fl["download_speed"]["mbps"].as_f64().unwrap()
    );
    let d = &fl["time_remaining"]["duration"];
    assert_eq!(
        live["eta_ms"].as_u64().unwrap(),
        d["secs"].as_u64().unwrap() * 1000 + d["nanos"].as_u64().unwrap() / 1_000_000
    );
    // Nulls are left out.
    assert!(row["stats"].get("error").is_none());
    assert_eq!(row["name"], full["name"]);
}

#[test]
fn snapshot_plus_deltas_equals_full_state() {
    let st = states();
    let mut c = FeedClient::default();
    for (i, m) in messages().iter().enumerate() {
        c.apply(m).unwrap();
        assert_eq!(c.state, st[i], "state {i}");
        assert_eq!(c.seq, i as u64 + 1);
    }
    // Skipping ahead (a polling client that missed versions): one delta 1 → 4.
    let mut c = FeedClient::default();
    c.apply(&snapshot_msg("e", 1, &st[0])).unwrap();
    c.apply(&delta_msg("e", 1, 4, &st[0], &st[3])).unwrap();
    assert_eq!(c.state, st[3]);
}

#[test]
fn delta_covers_add_remove_static_change_and_order() {
    let st = states();
    let d = delta_msg("e", 3, 4, &st[2], &st[3]);
    assert_eq!(d["added"].as_array().unwrap().len(), 1);
    assert_eq!(d["added"][0]["id"], 9001);
    assert_eq!(d["removed"].as_array().unwrap().len(), 1);
    assert!(d.get("order").is_some(), "added mid-list → explicit order");
    let changed = d["changed"].as_array().unwrap();
    assert!(
        changed
            .iter()
            .any(|p| p["category_label"] == "Changed label")
    );
    // A cleared ETA is a null in the patch.
    assert!(
        changed
            .iter()
            .any(|p| p["stats"]["live"].get("eta_ms") == Some(&Value::Null))
    );
    // Unchanged static fields are not repeated.
    assert!(
        changed
            .iter()
            .all(|p| p.get("name").is_none() && p.get("output_folder").is_none())
    );
    // Consecutive real captures: no adds/removes/order, only changed fields.
    let d = delta_msg("e", 1, 2, &st[0], &st[1]);
    assert!(d.get("added").is_none() && d.get("removed").is_none() && d.get("order").is_none());
}

#[test]
fn deltas_are_much_smaller_than_snapshots() {
    let st = states();
    let snap = serde_json::to_vec(&snapshot_msg("e", 1, &st[1]))
        .unwrap()
        .len();
    let delta = serde_json::to_vec(&delta_msg("e", 1, 2, &st[0], &st[1]))
        .unwrap()
        .len();
    let full = serde_json::to_vec(&snapshots()[1]).unwrap().len();
    assert!(snap * 10 < full * 8, "lean snapshot {snap} vs full {full}");
    assert!(delta * 4 < snap, "delta {delta} vs snapshot {snap}");
}

#[test]
fn gaps_and_bad_messages_are_rejected() {
    let st = states();
    let mut c = FeedClient::default();
    c.apply(&snapshot_msg("e", 5, &st[0])).unwrap();
    assert_eq!(
        c.apply(&delta_msg("e", 4, 6, &st[0], &st[1])),
        Err(ApplyError::Gap)
    );
    assert_eq!(
        c.apply(&delta_msg("other", 5, 6, &st[0], &st[1])),
        Err(ApplyError::Gap)
    );
    assert_eq!(
        c.apply(&json!({"type": "delta", "proto": 99, "epoch": "e", "base": 5, "seq": 6})),
        Err(ApplyError::Invalid)
    );
    assert_eq!(
        c.apply(&json!({"type": "nope", "proto": 1, "epoch": "e", "seq": 6})),
        Err(ApplyError::Invalid)
    );
    // Still at 5 and intact; heartbeats are fine.
    assert_eq!((c.seq, &c.state), (5, &st[0]));
    c.apply(&json!({"type": "heartbeat", "epoch": "e", "seq": 5}))
        .unwrap();
    c.apply(&delta_msg("e", 5, 6, &st[0], &st[1])).unwrap();
    assert_eq!(c.state, st[1]);
}

#[test]
fn merge_patch_round_trips() {
    let cases = [
        (
            json!({"a": 1, "b": {"c": 2, "d": [1, 2]}}),
            json!({"a": 1, "b": {"c": 3, "d": [1, 2, 3]}}),
        ),
        (json!({"a": 1, "b": {"c": 2}}), json!({"b": {"e": "x"}})),
        (json!({"a": {"b": {"c": 1}}}), json!({"a": 5})),
        (json!({"a": 5}), json!({"a": {"b": {"c": 1}}})),
        (json!({"a": 1}), json!({"a": 1})),
    ];
    for (old, new) in cases {
        let mut t = old.clone();
        if let Some(p) = diff_value(&old, &new) {
            apply_patch(&mut t, &p);
        }
        assert_eq!(t, new, "{old} → {new}");
    }
    assert_eq!(diff_value(&json!({"a": 1}), &json!({"a": 1})), None);
}

/// The checked-in messages the web UI and GPUI tests replay. Regenerate with
/// `UPDATE_LIST_FEED_GOLDEN=1 cargo test -p librqbit list_feed`.
#[test]
fn golden_messages_are_current() {
    let generated = serde_json::to_string_pretty(&json!({
        "note": "Generated from list_snapshots.json by list_feed tests: a snapshot, then a delta to each following state.",
        "messages": messages(),
    }))
    .unwrap()
        + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH);
    if std::env::var_os("UPDATE_LIST_FEED_GOLDEN").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == generated,
        "{GOLDEN_PATH} is stale; regenerate it (see the doc comment)"
    );
}
