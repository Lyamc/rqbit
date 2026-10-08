use std::{net::SocketAddr, str::FromStr, time::Duration};

use itertools::Itertools;
use serde::{Deserialize, Serialize};

use crate::{AddTorrentOptions, PeerConnectionOptions};

pub struct OnlyFiles(Vec<usize>);
pub struct InitialPeers(pub Vec<SocketAddr>);

pub use crate::torrent_state::peer::stats::snapshot::{PeerStatsFilter, PeerStatsSnapshot};

#[derive(Serialize, Deserialize, Default)]
pub struct TorrentAddQueryParams {
    pub overwrite: Option<bool>,
    pub output_folder: Option<String>,
    pub sub_folder: Option<String>,
    pub only_files_regex: Option<String>,
    pub only_files: Option<OnlyFiles>,
    pub peer_connect_timeout: Option<u64>,
    pub peer_read_write_timeout: Option<u64>,
    pub initial_peers: Option<InitialPeers>,
    // Will force interpreting the content as a URL.
    pub is_url: Option<bool>,
    /// Read a .torrent from this server filesystem path (must be under browse roots).
    pub from_server_path: Option<String>,
    pub list_only: Option<bool>,
    /// Optional Newznab/Torznab category id (e.g. 2000=Movies, 5070=Anime).
    pub torznab_category: Option<u32>,
    /// Category display name (free text, at most 100 characters).
    pub category: Option<String>,
    /// Where the category comes from, e.g. "nyaa" (a-z, 0-9, _ or -).
    pub category_source: Option<String>,
    /// The source's own category id, e.g. "1_2".
    pub category_id: Option<String>,
    /// Adopt another client's data in output_folder before the initial check
    /// (transfer from other client). Supported: "auto" ("qbit" = alias).
    pub adopt_foreign_incomplete: Option<String>,
    /// Client-chosen id to poll / cancel this add (see /add_jobs).
    pub add_job_id: Option<String>,
    /// Magnets: length of one metadata resolve attempt (seconds). When it runs out the
    /// magnet keeps resolving in the background after a backoff; it never fails the add.
    pub magnet_timeout_secs: Option<u64>,
    /// Accepted for compatibility; no effect. Magnets are always queued at once with a
    /// torrent id and resolved in the background ("Resolving metadata").
    pub defer_metadata: Option<bool>,
    /// Magnets: wait for the metadata before answering (old behaviour). If it doesn't
    /// arrive in time the magnet is queued as a resolving placeholder anyway (still a
    /// 200 with `resolving: true`), never an error.
    pub wait_for_metadata: Option<bool>,
    /// Add paused (`true`) or started (`false`). Unset: the "When a torrent is added"
    /// preference. Magnets still fetch their metadata while paused (no data).
    pub paused: Option<bool>,
    /// Sent by Add dialogs: with "Start after I finish the Add dialog" on, the torrent
    /// is held paused until `POST /add_dialog/{id}/finish` (see [`crate::add_dialog`]).
    pub add_dialog_id: Option<String>,
}

impl Serialize for OnlyFiles {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = self.0.iter().map(|id| id.to_string()).join(",");
        s.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OnlyFiles {
    fn deserialize<D>(deserializer: D) -> core::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let s = String::deserialize(deserializer)?;
        let list = s
            .split(',')
            .try_fold(Vec::<usize>::new(), |mut acc, c| match c.parse() {
                Ok(i) => {
                    acc.push(i);
                    Ok(acc)
                }
                Err(_) => Err(D::Error::custom(format!(
                    "only_files: failed to parse {c:?} as integer"
                ))),
            })?;
        if list.is_empty() {
            return Err(D::Error::custom(
                "only_files: should contain at least one file id",
            ));
        }
        Ok(OnlyFiles(list))
    }
}

impl<'de> Deserialize<'de> for InitialPeers {
    fn deserialize<D>(deserializer: D) -> std::prelude::v1::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let string = String::deserialize(deserializer)?;
        let mut addrs = Vec::new();
        for addr_str in string.split(',').filter(|s| !s.is_empty()) {
            addrs.push(SocketAddr::from_str(addr_str).map_err(D::Error::custom)?);
        }
        Ok(InitialPeers(addrs))
    }
}

impl Serialize for InitialPeers {
    fn serialize<S>(&self, serializer: S) -> std::prelude::v1::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0
            .iter()
            .map(|s| s.to_string())
            .join(",")
            .serialize(serializer)
    }
}

impl TorrentAddQueryParams {
    pub fn into_add_torrent_options(self) -> AddTorrentOptions {
        AddTorrentOptions {
            overwrite: self.overwrite.unwrap_or(false),
            only_files_regex: self.only_files_regex,
            only_files: self.only_files.map(|o| o.0),
            output_folder: self.output_folder,
            sub_folder: self.sub_folder,
            list_only: self.list_only.unwrap_or(false),
            initial_peers: self.initial_peers.map(|i| i.0),
            peer_opts: Some(PeerConnectionOptions {
                connect_timeout: self.peer_connect_timeout.map(Duration::from_secs),
                read_write_timeout: self.peer_read_write_timeout.map(Duration::from_secs),
                ..Default::default()
            }),
            torznab_category: self.torznab_category,
            category: self.category,
            category_source: self.category_source,
            category_id: self.category_id,
            adopt_foreign_incomplete: self.adopt_foreign_incomplete,
            add_job_id: self.add_job_id,
            magnet_resolve_timeout: self.magnet_timeout_secs.map(Duration::from_secs),
            defer_metadata: self.defer_metadata.unwrap_or(false),
            wait_for_metadata: self.wait_for_metadata.unwrap_or(false),
            paused: self.paused.unwrap_or(false),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod category_param_tests {
    use super::*;

    fn parse(qs: &str) -> Result<TorrentAddQueryParams, serde_urlencoded::de::Error> {
        serde_urlencoded::from_str(qs)
    }

    #[test]
    fn category_params() {
        let p = parse("category=Anime%20-%20Raw&category_source=nyaa&category_id=1_4&torznab_category=5070").unwrap();
        let mut o = p.into_add_torrent_options();
        o.normalize_category().unwrap();
        assert_eq!(o.category.as_deref(), Some("Anime - Raw"));
        assert_eq!(o.category_source.as_deref(), Some("nyaa"));
        assert_eq!(o.category_id.as_deref(), Some("1_4"));
        assert_eq!(o.torznab_category, Some(5070));
        // Optional; empty means none.
        let mut o = parse("category=&category_source=NYAA").unwrap().into_add_torrent_options();
        o.normalize_category().unwrap();
        assert_eq!((o.category, o.category_source.as_deref()), (None, Some("nyaa")));
        let mut o = parse("category_source=bad%20source").unwrap().into_add_torrent_options();
        assert!(o.normalize_category().is_err());
        let mut o = parse("category_id=1%2F2").unwrap().into_add_torrent_options();
        assert!(o.normalize_category().is_err());
    }

    #[test]
    fn torznab_category_stays_numeric() {
        assert!(parse("torznab_category=Anime").is_err());
        assert!(parse("torznab_category=1_2").is_err());
    }
}
