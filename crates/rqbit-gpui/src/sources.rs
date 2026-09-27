//! Finds magnet links and http(s) .torrent URLs in pasted text, like the web
//! UI's `helper/parseTorrentSources.ts` (`extractTorrentSources`).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Magnet,
    Url,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSource {
    pub value: String,
    pub kind: SourceKind,
}

fn is_stop(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'')
}

/// Strip trailing punctuation picked up from prose (`.,;:)]}>`).
fn clean_token(raw: &str) -> &str {
    raw.trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '>'])
}

/// Byte offsets of case-insensitive (ASCII) occurrences of `needle`.
fn find_all_ci(hay: &str, needle: &str) -> Vec<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return Vec::new();
    }
    (0..=h.len() - n.len())
        .filter(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
        .collect()
}

/// Token starting at byte `start` and running until a stop character.
fn token_at<'a>(text: &'a str, start: usize, extra_stop: &[char]) -> &'a str {
    let rest = &text[start..];
    let end = rest
        .char_indices()
        .find(|(_, c)| is_stop(*c) || extra_stop.contains(c))
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// `https?://[^\s<>"']+\.torrent(\?[^\s<>"']*)?` applied to one token.
fn torrent_url_in_token(tok: &str) -> Option<&str> {
    let lower = tok.to_ascii_lowercase();
    // Greedy: the last ".torrent" that ends the token or is followed by a query.
    let mut best = None;
    for (i, _) in lower.match_indices(".torrent") {
        let after = i + ".torrent".len();
        if after == tok.len() || tok.as_bytes()[after] == b'?' {
            best = Some(tok);
        } else if best.is_none() && i > "https://".len() {
            // e.g. ".torrentX": the regex would still match up to ".torrent".
            best = Some(&tok[..after]);
        }
    }
    best
}

/// Magnets first, then explicit *.torrent URLs; if neither is found, any
/// whitespace-separated `magnet:` / `http(s)://` token. Deduplicated.
pub fn extract_torrent_sources(text: &str) -> Vec<ParsedSource> {
    let mut out: Vec<ParsedSource> = Vec::new();
    fn push(out: &mut Vec<ParsedSource>, raw: &str, kind: SourceKind) {
        let v = clean_token(raw);
        if v.is_empty() || out.iter().any(|s| s.value == v) {
            return;
        }
        out.push(ParsedSource {
            value: v.to_owned(),
            kind,
        });
    }

    for i in find_all_ci(text, "magnet:?") {
        push(&mut out, token_at(text, i, &[']']), SourceKind::Magnet);
    }
    let mut url_starts = find_all_ci(text, "http://");
    url_starts.extend(find_all_ci(text, "https://"));
    url_starts.sort_unstable();
    for i in url_starts {
        if let Some(u) = torrent_url_in_token(token_at(text, i, &[])) {
            push(&mut out, u, SourceKind::Url);
        }
    }

    if out.is_empty() {
        for tok in text.split_whitespace() {
            let t = clean_token(tok.trim());
            let l = t.to_ascii_lowercase();
            if l.starts_with("magnet:") {
                push(&mut out, t, SourceKind::Magnet);
            } else if l.starts_with("http://") || l.starts_with("https://") {
                push(&mut out, t, SourceKind::Url);
            }
        }
    }
    out
}

/// Label for a staged source: the magnet's `dn=` name if present, else the
/// (shortened) text. Mirrors the web UI's `displayNameForSource`.
pub fn display_name_for_source(text: &str) -> String {
    if let Some(q) = text.split_once('?').map(|(_, q)| q) {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            if k.eq_ignore_ascii_case("dn") && !v.is_empty() {
                return v.into_owned();
            }
        }
    }
    shorten(text, 72)
}

pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let s: String = text.chars().take(max.saturating_sub(3)).collect();
        format!("{s}…")
    } else {
        text.to_owned()
    }
}

pub fn is_magnet(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.starts_with("magnet:") || (text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(t: &str) -> Vec<String> {
        extract_torrent_sources(t)
            .into_iter()
            .map(|s| s.value)
            .collect()
    }

    #[test]
    fn finds_magnets_and_torrent_urls_in_prose() {
        let text = "see magnet:?xt=urn:btih:ABC&dn=Foo+Bar, and \
                    <https://ex.com/a/b.torrent?x=1> plus http://h/c.TORRENT.\n\
                    magnet:?xt=urn:btih:ABC&dn=Foo+Bar";
        let s = extract_torrent_sources(text);
        assert_eq!(
            s.iter().map(|x| x.value.as_str()).collect::<Vec<_>>(),
            vec![
                "magnet:?xt=urn:btih:ABC&dn=Foo+Bar",
                "https://ex.com/a/b.torrent?x=1",
                "http://h/c.TORRENT",
            ]
        );
        assert_eq!(s[0].kind, SourceKind::Magnet);
        assert_eq!(s[1].kind, SourceKind::Url);
    }

    #[test]
    fn falls_back_to_plain_urls_one_per_line() {
        assert_eq!(
            values("https://tracker/dl?id=5\nhttp://x/y\nnot a url"),
            vec!["https://tracker/dl?id=5", "http://x/y"]
        );
        assert!(values("nothing here").is_empty());
        // Only non-.torrent URLs are ignored once a magnet was found.
        assert_eq!(values("magnet:?xt=1 https://x/y"), vec!["magnet:?xt=1"]);
    }

    #[test]
    fn magnet_stops_at_bracket_and_quotes() {
        assert_eq!(values("[magnet:?xt=1]"), vec!["magnet:?xt=1"]);
        assert_eq!(values("href=\"magnet:?xt=2\""), vec!["magnet:?xt=2"]);
    }

    #[test]
    fn display_names() {
        assert_eq!(
            display_name_for_source("magnet:?xt=urn:btih:1&dn=Hello%20World+2"),
            "Hello World 2"
        );
        assert_eq!(
            display_name_for_source("https://x/y.torrent"),
            "https://x/y.torrent"
        );
        assert_eq!(shorten(&"a".repeat(80), 72).chars().count(), 70);
        assert!(is_magnet("magnet:?xt=1"));
        assert!(is_magnet(&"a".repeat(40)));
        assert!(!is_magnet("https://x"));
    }
}
