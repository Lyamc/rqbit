//! Per-torrent categories: a free-text name plus an optional source-specific id
//! (e.g. source "nyaa", id "1_2") next to the Newznab/Torznab number.
//!
//! The Torznab number drives auto-organize. When a torrent has none, it is looked up
//! from `(source, id)` in [`SOURCE_CATEGORIES`], then name heuristics take over.
//! Torznab 8000 (Other, e.g. Nyaa Pictures) has no media type of its own, so those
//! are classified by the heuristics too.

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// One known source category. `X_0` ids are the parent categories: an unknown
/// `X_n` id falls back to its `X_0` row.
#[derive(Debug, Clone, Copy)]
pub struct SourceCategoryEntry {
    pub source: &'static str,
    pub id: &'static str,
    pub name: &'static str,
    pub torznab: Option<u32>,
}

const fn e(
    source: &'static str,
    id: &'static str,
    name: &'static str,
    torznab: u32,
) -> SourceCategoryEntry {
    SourceCategoryEntry {
        source,
        id,
        name,
        torznab: Some(torznab),
    }
}

/// Known source categories and the Torznab number each maps to.
///
/// To update: edit rows here. `source` and `id` are matched case-insensitively.
#[rustfmt::skip]
pub const SOURCE_CATEGORIES: &[SourceCategoryEntry] = &[
    // nyaa
    e("nyaa", "1_0", "Anime", 5070),
    e("nyaa", "1_1", "Anime - Anime Music Video", 5070),
    e("nyaa", "1_2", "Anime - English-translated", 5070),
    e("nyaa", "1_3", "Anime - Non-English-translated", 5070),
    e("nyaa", "1_4", "Anime - Raw", 5070),
    e("nyaa", "2_0", "Audio", 3000),
    e("nyaa", "2_1", "Audio - Lossless", 3040),
    e("nyaa", "2_2", "Audio - Lossy", 3000),
    e("nyaa", "3_0", "Literature", 7000),
    e("nyaa", "3_1", "Literature - English-translated", 7000),
    e("nyaa", "3_2", "Literature - Non-English-translated", 7000),
    e("nyaa", "3_3", "Literature - Raw", 7000),
    e("nyaa", "4_0", "Live Action", 5000),
    e("nyaa", "4_1", "Live Action - English-translated", 5000),
    e("nyaa", "4_2", "Live Action - Idol/Promotional Video", 5000),
    e("nyaa", "4_3", "Live Action - Non-English-translated", 5000),
    e("nyaa", "4_4", "Live Action - Raw", 5000),
    e("nyaa", "5_0", "Pictures", 8000),
    e("nyaa", "5_1", "Pictures - Graphics", 8000),
    e("nyaa", "5_2", "Pictures - Photos", 8000),
    e("nyaa", "6_0", "Software", 4000),
    e("nyaa", "6_1", "Software - Applications", 4000),
    e("nyaa", "6_2", "Software - Games", 4050),
    // sukebei
    e("sukebei", "1_0", "Art", 6000),
    e("sukebei", "1_1", "Art - Anime", 6000),
    e("sukebei", "1_2", "Art - Doujinshi", 6000),
    e("sukebei", "1_3", "Art - Games", 6000),
    e("sukebei", "1_4", "Art - Manga", 6000),
    e("sukebei", "1_5", "Art - Pictures", 6000),
    e("sukebei", "2_0", "Real Life", 6000),
    e("sukebei", "2_1", "Real Life - Photobooks / Pictures", 6000),
    e("sukebei", "2_2", "Real Life - Videos", 6000),
];

/// The table entry for `(source, id)`: the exact id, else its parent `X_0`.
pub fn lookup(source: &str, id: Option<&str>) -> Option<&'static SourceCategoryEntry> {
    let id = id?;
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let find = |id: &str| {
        SOURCE_CATEGORIES
            .iter()
            .find(|r| same(r.source, source) && same(r.id, id))
    };
    find(id).or_else(|| {
        let (major, _) = id.split_once('_')?;
        find(&format!("{major}_0"))
    })
}

/// Display name of a Torznab number (thousands bucket; 5070 is Anime).
pub fn torznab_name(n: u32) -> Option<&'static str> {
    if (5070..5080).contains(&n) {
        return Some("Anime");
    }
    if n == 4050 {
        return Some("Games");
    }
    Some(match n / 1000 {
        1 => "Games",
        2 => "Movies",
        3 => "Audio",
        4 => "Software",
        5 => "TV",
        6 => "Adult",
        7 => "Books",
        8 => "Other",
        _ => return None,
    })
}

/// A torrent's category. Every part is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentCategory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torznab: Option<u32>,
}

pub const MAX_NAME_LEN: usize = 100;
pub const MAX_SOURCE_LEN: usize = 32;
pub const MAX_ID_LEN: usize = 32;

fn clean(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Validate and normalize a category name: trimmed, empty means none.
pub fn validate_name(v: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(s) = clean(v) else { return Ok(None) };
    if s.chars().count() > MAX_NAME_LEN {
        bail!("category is longer than {MAX_NAME_LEN} characters");
    }
    if s.chars().any(char::is_control) {
        bail!("category contains control characters");
    }
    Ok(Some(s))
}

/// Validate and normalize a category source: lowercase `[a-z0-9_-]`, 1-32 characters.
pub fn validate_source(v: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(s) = clean(v) else { return Ok(None) };
    let s = s.to_ascii_lowercase();
    if s.len() > MAX_SOURCE_LEN
        || !s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        bail!("category_source must be 1-{MAX_SOURCE_LEN} characters of a-z, 0-9, _ or -");
    }
    Ok(Some(s))
}

/// Validate a source-specific category id: 1-32 characters of `[A-Za-z0-9_.-]`.
pub fn validate_id(v: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(s) = clean(v) else { return Ok(None) };
    if s.len() > MAX_ID_LEN
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        bail!("category_id must be 1-{MAX_ID_LEN} characters of A-Z, a-z, 0-9, _, . or -");
    }
    Ok(Some(s))
}

impl TorrentCategory {
    /// Build from raw add parameters, validating every part.
    pub fn from_parts(
        name: Option<&str>,
        source: Option<&str>,
        id: Option<&str>,
        torznab: Option<u32>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            name: validate_name(name)?,
            source: validate_source(source)?,
            id: validate_id(id)?,
            torznab,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.source.is_none() && self.id.is_none() && self.torznab.is_none()
    }

    fn table_entry(&self) -> Option<&'static SourceCategoryEntry> {
        lookup(self.source.as_deref()?, self.id.as_deref())
    }

    /// The Torznab number auto-organize uses: the explicit one, else the table's.
    pub fn effective_torznab(&self) -> Option<u32> {
        self.torznab
            .or_else(|| self.table_entry().and_then(|e| e.torznab))
    }

    /// What the UIs show: the name, else the table name for `(source, id)`, else
    /// `"source id"`, else the Torznab name.
    pub fn label(&self) -> Option<String> {
        if let Some(n) = &self.name {
            return Some(n.clone());
        }
        if let Some(e) = self.table_entry() {
            return Some(e.name.to_owned());
        }
        match (&self.source, &self.id) {
            (Some(s), Some(i)) => return Some(format!("{s} {i}")),
            (Some(s), None) => return Some(s.clone()),
            _ => {}
        }
        self.torznab.and_then(torznab_name).map(str::to_owned)
    }
}

/// Category fields as the HTTP API returns them (flattened into torrent details).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CategoryView {
    #[serde(default, rename = "category", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        rename = "category_source",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<String>,
    #[serde(
        default,
        rename = "category_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<String>,
    #[serde(
        default,
        rename = "category_label",
        skip_serializing_if = "Option::is_none"
    )]
    pub label: Option<String>,
}

impl From<&TorrentCategory> for CategoryView {
    fn from(c: &TorrentCategory) -> Self {
        Self {
            name: c.name.clone(),
            source: c.source.clone(),
            id: c.id.clone(),
            label: c.label(),
        }
    }
}

/// `{"category", "category_source", "category_id", "torznab_category"}` for
/// `POST /torrents/{id}/category`: a missing key leaves that part alone, `null` clears it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CategoryUpdate {
    #[serde(default, deserialize_with = "some")]
    pub category: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    pub category_source: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    pub category_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    pub torznab_category: Option<Option<u32>>,
}

fn some<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl CategoryUpdate {
    /// Apply to `cur`, validating the new parts.
    pub fn apply(&self, cur: &TorrentCategory) -> anyhow::Result<TorrentCategory> {
        let mut c = cur.clone();
        if let Some(v) = &self.category {
            c.name = validate_name(v.as_deref())?;
        }
        if let Some(v) = &self.category_source {
            c.source = validate_source(v.as_deref())?;
        }
        if let Some(v) = &self.category_id {
            c.id = validate_id(v.as_deref())?;
        }
        if let Some(v) = self.torznab_category {
            c.torznab = v;
        }
        Ok(c)
    }

    pub fn is_noop(&self) -> bool {
        self.category.is_none()
            && self.category_source.is_none()
            && self.category_id.is_none()
            && self.torznab_category.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let c = TorrentCategory::from_parts(
            Some("  Anime - English-translated "),
            Some("Nyaa"),
            Some("1_2"),
            Some(5070),
        )
        .unwrap();
        assert_eq!(c.name.as_deref(), Some("Anime - English-translated"));
        assert_eq!(c.source.as_deref(), Some("nyaa"));
        assert_eq!(c.id.as_deref(), Some("1_2"));
        let empty = TorrentCategory::from_parts(Some("  "), Some(""), None, None).unwrap();
        assert!(empty.is_empty());
        assert!(validate_name(Some(&"x".repeat(101))).is_err());
        assert!(validate_name(Some("a\nb")).is_err());
        assert!(validate_source(Some("ny aa")).is_err());
        assert!(validate_source(Some(&"a".repeat(33))).is_err());
        assert!(validate_id(Some("1/2")).is_err());
        assert!(validate_id(Some("1_2")).is_ok());
    }

    #[test]
    fn lookup_and_effective_torznab() {
        let c = |s: &str, i: Option<&str>, t: Option<u32>| TorrentCategory {
            source: Some(s.into()),
            id: i.map(Into::into),
            torznab: t,
            ..Default::default()
        };
        // Explicit Torznab wins.
        assert_eq!(
            c("nyaa", Some("1_2"), Some(2000)).effective_torznab(),
            Some(2000)
        );
        // Table rows.
        for (src, id, n, name) in [
            ("nyaa", "1_2", 5070, "Anime - English-translated"),
            ("nyaa", "1_4", 5070, "Anime - Raw"),
            ("nyaa", "2_1", 3040, "Audio - Lossless"),
            ("nyaa", "2_2", 3000, "Audio - Lossy"),
            ("nyaa", "3_1", 7000, "Literature - English-translated"),
            ("nyaa", "4_2", 5000, "Live Action - Idol/Promotional Video"),
            ("nyaa", "5_1", 8000, "Pictures - Graphics"),
            ("nyaa", "6_1", 4000, "Software - Applications"),
            ("nyaa", "6_2", 4050, "Software - Games"),
            ("sukebei", "1_2", 6000, "Art - Doujinshi"),
            ("sukebei", "2_1", 6000, "Real Life - Photobooks / Pictures"),
            ("sukebei", "2_2", 6000, "Real Life - Videos"),
        ] {
            let cat = c(src, Some(id), None);
            assert_eq!(cat.effective_torznab(), Some(n), "{src} {id}");
            assert_eq!(cat.label().as_deref(), Some(name), "{src} {id}");
        }
        assert_eq!(c("NYAA", Some("6_2"), None).effective_torznab(), Some(4050));
        // Unknown sub-id: its parent X_0 row.
        assert_eq!(c("nyaa", Some("1_9"), None).effective_torznab(), Some(5070));
        assert_eq!(
            c("sukebei", Some("2_9"), None).effective_torznab(),
            Some(6000)
        );
        // Unknown parent / source / no id: nothing.
        assert_eq!(c("sukebei", Some("9_9"), None).effective_torznab(), None);
        assert_eq!(c("other", Some("1_2"), None).effective_torznab(), None);
        assert_eq!(c("nyaa", None, None).effective_torznab(), None);
        assert_eq!(TorrentCategory::default().effective_torznab(), None);
        // Every row is well-formed.
        for r in SOURCE_CATEGORIES {
            assert!(validate_source(Some(r.source)).unwrap().as_deref() == Some(r.source));
            assert!(validate_id(Some(r.id)).unwrap().as_deref() == Some(r.id));
            assert!(validate_name(Some(r.name)).is_ok());
        }
    }

    #[test]
    fn organize_priority() {
        use crate::media_classify::{MediaType, media_type_from_torznab_category as m};
        let media = |c: &TorrentCategory| c.effective_torznab().and_then(m);
        // Explicit Torznab number first.
        let c = TorrentCategory::from_parts(None, Some("nyaa"), Some("1_2"), Some(2000)).unwrap();
        assert_eq!(media(&c), Some(MediaType::Movie));
        // Then the (source, id) table.
        let c = TorrentCategory::from_parts(None, Some("nyaa"), Some("1_2"), None).unwrap();
        assert_eq!(media(&c), Some(MediaType::Anime));
        let c = TorrentCategory::from_parts(None, Some("sukebei"), Some("1_1"), None).unwrap();
        assert_eq!(media(&c), Some(MediaType::Porn));
        let c = TorrentCategory::from_parts(None, Some("nyaa"), Some("6_2"), None).unwrap();
        assert_eq!(media(&c), Some(MediaType::Game));
        let c = TorrentCategory::from_parts(None, Some("nyaa"), Some("2_1"), None).unwrap();
        assert_eq!(media(&c), Some(MediaType::Music));
        // Pictures (8000) and unknown ids: name heuristics decide (None here).
        let c =
            TorrentCategory::from_parts(Some("Whatever"), Some("nyaa"), Some("5_1"), None).unwrap();
        assert_eq!(media(&c), None);
        let c = TorrentCategory::from_parts(None, Some("other"), Some("1_2"), None).unwrap();
        assert_eq!(media(&c), None);
    }

    #[test]
    fn labels() {
        let mut c = TorrentCategory {
            source: Some("nyaa".into()),
            id: Some("1_2".into()),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("Anime - English-translated"));
        c.name = Some("Custom".into());
        assert_eq!(c.label().as_deref(), Some("Custom"));
        let c = TorrentCategory {
            source: Some("nyaa".into()),
            id: Some("1_9".into()),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("Anime"));
        let c = TorrentCategory {
            source: Some("sukebei".into()),
            id: Some("2_9".into()),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("Real Life"));
        let c = TorrentCategory {
            source: Some("sukebei".into()),
            id: Some("9_9".into()),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("sukebei 9_9"));
        let c = TorrentCategory {
            source: Some("other".into()),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("other"));
        let c = TorrentCategory {
            torznab: Some(5070),
            ..Default::default()
        };
        assert_eq!(c.label().as_deref(), Some("Anime"));
        assert_eq!(
            TorrentCategory {
                torznab: Some(2040),
                ..Default::default()
            }
            .label()
            .as_deref(),
            Some("Movies")
        );
        assert_eq!(
            TorrentCategory {
                torznab: Some(4050),
                ..Default::default()
            }
            .label()
            .as_deref(),
            Some("Games")
        );
        assert_eq!(TorrentCategory::default().label(), None);
    }

    #[test]
    fn update_semantics() {
        let cur =
            TorrentCategory::from_parts(Some("A"), Some("nyaa"), Some("1_2"), Some(5070)).unwrap();
        let u: CategoryUpdate = serde_json::from_str(r#"{"category": "B"}"#).unwrap();
        let n = u.apply(&cur).unwrap();
        assert_eq!(
            (n.name.as_deref(), n.source.as_deref(), n.torznab),
            (Some("B"), Some("nyaa"), Some(5070))
        );
        let u: CategoryUpdate =
            serde_json::from_str(r#"{"category": null, "torznab_category": null}"#).unwrap();
        let n = u.apply(&cur).unwrap();
        assert_eq!(
            (n.name, n.torznab, n.id.as_deref()),
            (None, None, Some("1_2"))
        );
        let u: CategoryUpdate = serde_json::from_str(r#"{}"#).unwrap();
        assert!(u.is_noop());
        let u: CategoryUpdate =
            serde_json::from_str(r#"{"category_source": "bad source"}"#).unwrap();
        assert!(u.apply(&cur).is_err());
        assert!(
            serde_json::from_str::<CategoryUpdate>(r#"{"torznab_category": "Anime"}"#).is_err()
        );
    }

    #[test]
    fn view_serialization() {
        let c = TorrentCategory::from_parts(None, Some("nyaa"), Some("1_2"), None).unwrap();
        let v = serde_json::to_value(CategoryView::from(&c)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"category_source": "nyaa", "category_id": "1_2", "category_label": "Anime - English-translated"})
        );
        let v = serde_json::to_value(CategoryView::from(&TorrentCategory::default())).unwrap();
        assert_eq!(v, serde_json::json!({}));
    }
}
