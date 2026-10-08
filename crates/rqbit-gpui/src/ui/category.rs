//! Torrent categories in the list (pure, unit tested): the category filter
//! and the "Set category…" form. Mirrors the web UI's `helper/category.ts`.

use serde_json::{Map, Value};

use crate::api::TorrentListItem;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum CategoryFilter {
    #[default]
    All,
    /// Torrents without a category.
    None,
    Label(String),
}

impl CategoryFilter {
    pub fn label(&self) -> String {
        match self {
            CategoryFilter::All => "All".into(),
            CategoryFilter::None => "None".into(),
            CategoryFilter::Label(l) => l.clone(),
        }
    }

    pub fn matches(&self, t: &TorrentListItem) -> bool {
        match self {
            CategoryFilter::All => true,
            CategoryFilter::None => t.category_text().is_empty(),
            CategoryFilter::Label(l) => t.category_text() == *l,
        }
    }
}

/// Distinct category labels, sorted (case-insensitive).
pub fn category_options(torrents: &[TorrentListItem]) -> Vec<String> {
    let mut v: Vec<String> = torrents
        .iter()
        .map(|t| t.category_text())
        .filter(|l| !l.is_empty())
        .collect();
    v.sort_by_key(|l| l.to_lowercase());
    v.dedup();
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CategoryField {
    Name,
    Source,
    Id,
    Torznab,
}

impl CategoryField {
    pub const ALL: [CategoryField; 4] = [
        CategoryField::Name,
        CategoryField::Source,
        CategoryField::Id,
        CategoryField::Torznab,
    ];

    pub fn key(self) -> &'static str {
        match self {
            CategoryField::Name => "category",
            CategoryField::Source => "category_source",
            CategoryField::Id => "category_id",
            CategoryField::Torznab => "torznab_category",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CategoryField::Name => "Name",
            CategoryField::Source => "Source",
            CategoryField::Id => "Source id",
            CategoryField::Torznab => "Torznab number",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            CategoryField::Name => "e.g. Anime - English-translated",
            CategoryField::Source => "e.g. nyaa",
            CategoryField::Id => "e.g. 1_2",
            CategoryField::Torznab => "e.g. 5070",
        }
    }

    fn value(self, t: &TorrentListItem) -> String {
        match self {
            CategoryField::Name => t.category.clone().unwrap_or_default(),
            CategoryField::Source => t.category_source.clone().unwrap_or_default(),
            CategoryField::Id => t.category_id.clone().unwrap_or_default(),
            CategoryField::Torznab => t
                .torznab_category
                .map(|n| n.to_string())
                .unwrap_or_default(),
        }
    }

    /// Same rules as the server (which checks again).
    pub fn validate(self, raw: &str) -> Result<(), String> {
        let v = raw.trim();
        if v.is_empty() {
            return Ok(());
        }
        let ok = match self {
            CategoryField::Name => {
                if v.chars().count() > 100 {
                    return Err("Category is longer than 100 characters".into());
                }
                if v.chars().any(char::is_control) {
                    return Err("Category contains control characters".into());
                }
                true
            }
            CategoryField::Source => {
                v.len() <= 32
                    && v.to_ascii_lowercase().bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
            }
            CategoryField::Id => {
                v.len() <= 32
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            }
            CategoryField::Torznab => v.len() <= 9 && v.bytes().all(|b| b.is_ascii_digit()),
        };
        if ok {
            Ok(())
        } else {
            Err(match self {
                CategoryField::Source => "Source: 1-32 characters of a-z, 0-9, _ or -",
                CategoryField::Id => "Source id: 1-32 characters of A-Z, a-z, 0-9, _, . or -",
                _ => "Torznab category must be a number",
            }
            .into())
        }
    }
}

/// Starting value of one field for the selected torrents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInit {
    /// Prefilled text ("" when mixed or unset).
    pub value: String,
    /// The torrents have different values (left alone unless typed into).
    pub mixed: bool,
}

pub fn initial_form(torrents: &[&TorrentListItem]) -> [FieldInit; 4] {
    CategoryField::ALL.map(|f| {
        let mut values: Vec<String> = torrents.iter().map(|t| f.value(t)).collect();
        values.sort();
        values.dedup();
        let mixed = values.len() > 1;
        FieldInit {
            value: if mixed {
                String::new()
            } else {
                values.pop().unwrap_or_default()
            },
            mixed,
        }
    })
}

/// The `POST /torrents/{id}/category` body: fields whose text differs from the
/// starting value. An emptied field clears it (null); a mixed field left empty
/// stays unchanged.
pub fn build_update(init: &[FieldInit; 4], current: &[String; 4]) -> Result<Value, String> {
    let mut m = Map::new();
    for (i, f) in CategoryField::ALL.iter().enumerate() {
        let cur = current[i].trim();
        if cur == init[i].value.trim() {
            continue;
        }
        f.validate(cur)?;
        let v = if cur.is_empty() {
            Value::Null
        } else {
            match f {
                CategoryField::Torznab => {
                    Value::from(cur.parse::<u32>().map_err(|e| e.to_string())?)
                }
                CategoryField::Source => Value::from(cur.to_ascii_lowercase()),
                _ => Value::from(cur),
            }
        };
        m.insert(f.key().into(), v);
    }
    Ok(Value::Object(m))
}

/// Body that clears every part.
pub fn clear_update() -> Value {
    let mut m = Map::new();
    for f in CategoryField::ALL {
        m.insert(f.key().into(), Value::Null);
    }
    Value::Object(m)
}

/// Details row text, e.g. "Anime - English-translated (nyaa 1_2 · Torznab 5070)".
pub fn details_text(t: &TorrentListItem) -> String {
    let label = t.category_text();
    if label.is_empty() {
        return "None".into();
    }
    let mut parts = Vec::new();
    if let Some(s) = t.category_source.as_deref().filter(|s| !s.is_empty()) {
        parts.push(match t.category_id.as_deref() {
            Some(i) if !i.is_empty() => format!("{s} {i}"),
            _ => s.to_owned(),
        });
    }
    if let Some(n) = t.torznab_category {
        parts.push(format!("Torznab {n}"));
    }
    if parts.is_empty() {
        label
    } else {
        format!("{label} ({})", parts.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(id: usize, v: Value) -> TorrentListItem {
        let mut o = json!({"id": id, "info_hash": "", "output_folder": "", "total_pieces": 0});
        o.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
        serde_json::from_value(o).unwrap()
    }

    #[test]
    fn deserializes_and_labels() {
        let a = t(
            1,
            json!({"category_source": "nyaa", "category_id": "1_2", "category_label": "Anime - English-translated"}),
        );
        assert_eq!(a.category_text(), "Anime - English-translated");
        assert_eq!(
            t(2, json!({"category": "Custom"})).category_text(),
            "Custom"
        );
        assert_eq!(
            t(3, json!({"category_source": "other", "category_id": "7"})).category_text(),
            "other 7"
        );
        assert_eq!(t(4, json!({})).category_text(), "");
        // Older servers: no category fields at all.
        assert_eq!(t(5, json!({})).torznab_category, None);
    }

    #[test]
    fn filter_and_options() {
        let list = vec![
            t(1, json!({"category_label": "Anime"})),
            t(2, json!({"category_label": "Real Life - Videos"})),
            t(3, json!({})),
            t(4, json!({"category_label": "anime x"})),
            t(5, json!({"category_label": "Anime"})),
        ];
        assert_eq!(
            category_options(&list),
            vec!["Anime", "anime x", "Real Life - Videos"]
        );
        let n = |f: CategoryFilter| list.iter().filter(|t| f.matches(t)).count();
        assert_eq!(n(CategoryFilter::All), 5);
        assert_eq!(n(CategoryFilter::None), 1);
        assert_eq!(n(CategoryFilter::Label("Anime".into())), 2);
        assert_eq!(n(CategoryFilter::Label("Nope".into())), 0);
    }

    #[test]
    fn form_and_update() {
        let a = t(1, json!({"category_source": "nyaa", "category_id": "1_2"}));
        let b = t(2, json!({"category_source": "nyaa", "category_id": "1_4"}));
        let init = initial_form(&[&a, &b]);
        assert_eq!(
            init[1],
            FieldInit {
                value: "nyaa".into(),
                mixed: false
            }
        );
        assert_eq!(
            init[2],
            FieldInit {
                value: "".into(),
                mixed: true
            }
        );
        let cur = |v: [&str; 4]| v.map(str::to_owned);
        assert_eq!(
            build_update(&init, &cur(["", "nyaa", "", ""])).unwrap(),
            json!({})
        );
        assert_eq!(
            build_update(&init, &cur([" Anime - Raw ", "nyaa", "", "5070"])).unwrap(),
            json!({"category": "Anime - Raw", "torznab_category": 5070})
        );
        assert_eq!(
            build_update(&init, &cur(["", "", "", ""])).unwrap(),
            json!({"category_source": null})
        );
        assert_eq!(
            build_update(&init, &cur(["", "NYAA2", "", ""])).unwrap(),
            json!({"category_source": "nyaa2"})
        );
        assert!(build_update(&init, &cur(["", "bad source", "", ""])).is_err());
        assert!(build_update(&init, &cur(["", "nyaa", "1/2", ""])).is_err());
        assert!(build_update(&init, &cur(["", "nyaa", "", "Anime"])).is_err());
        assert!(build_update(&init, &cur([&"x".repeat(101), "nyaa", "", ""])).is_err());
        assert_eq!(
            clear_update(),
            json!({"category": null, "category_source": null, "category_id": null, "torznab_category": null})
        );
    }

    #[test]
    fn details_row() {
        let a = t(
            1,
            json!({"category_source": "sukebei", "category_id": "2_2", "torznab_category": 6000, "category_label": "Real Life - Videos"}),
        );
        assert_eq!(
            details_text(&a),
            "Real Life - Videos (sukebei 2_2 · Torznab 6000)"
        );
        assert_eq!(details_text(&t(2, json!({"category": "Custom"}))), "Custom");
        assert_eq!(details_text(&t(3, json!({}))), "None");
    }
}
