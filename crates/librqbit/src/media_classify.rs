//! Heuristic media-type classification for auto-organize.
//!
//! Classification is best-effort from torrent name + file names/extensions.
//! It can be wrong — treat results as a convenience, not ground truth.

use serde::{Deserialize, Serialize};

/// Coarse media categories used by auto-organize.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Anime,
    Tv,
    Movie,
    Game,
    Porn,
    Music,
    Book,
    Software,
    Other,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anime => "anime",
            Self::Tv => "tv",
            Self::Movie => "movie",
            Self::Game => "game",
            Self::Porn => "porn",
            Self::Music => "music",
            Self::Book => "book",
            Self::Software => "software",
            Self::Other => "other",
        }
    }
}

/// Subfolder names for each media type (configurable via preferences).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoOrganizeFolders {
    #[serde(default = "default_anime")]
    pub anime: String,
    #[serde(default = "default_tv")]
    pub tv: String,
    #[serde(default = "default_movie")]
    pub movie: String,
    #[serde(default = "default_game")]
    pub game: String,
    #[serde(default = "default_porn")]
    pub porn: String,
    #[serde(default = "default_music")]
    pub music: String,
    #[serde(default = "default_book")]
    pub book: String,
    #[serde(default = "default_software")]
    pub software: String,
    #[serde(default = "default_other")]
    pub other: String,
}

fn default_anime() -> String {
    "Anime".into()
}
fn default_tv() -> String {
    "TV".into()
}
fn default_movie() -> String {
    "Movies".into()
}
fn default_game() -> String {
    "Games".into()
}
fn default_porn() -> String {
    "Adult".into()
}
fn default_music() -> String {
    "Music".into()
}
fn default_book() -> String {
    "Books".into()
}
fn default_software() -> String {
    "Software".into()
}
fn default_other() -> String {
    "Other".into()
}

impl Default for AutoOrganizeFolders {
    fn default() -> Self {
        Self {
            anime: default_anime(),
            tv: default_tv(),
            movie: default_movie(),
            game: default_game(),
            porn: default_porn(),
            music: default_music(),
            book: default_book(),
            software: default_software(),
            other: default_other(),
        }
    }
}

impl AutoOrganizeFolders {
    pub fn folder_for(&self, t: MediaType) -> &str {
        match t {
            MediaType::Anime => &self.anime,
            MediaType::Tv => &self.tv,
            MediaType::Movie => &self.movie,
            MediaType::Game => &self.game,
            MediaType::Porn => &self.porn,
            MediaType::Music => &self.music,
            MediaType::Book => &self.book,
            MediaType::Software => &self.software,
            MediaType::Other => &self.other,
        }
    }
}

/// Map a Newznab/Torznab category id to a media type.
///
/// Uses the thousands bucket (1000 Games, 2000 Movies, …). Category 5070
/// (Anime) wins over generic TV (5000). Unknown / 8000+ returns `None` so
/// callers can fall back to name/path heuristics.
pub fn media_type_from_torznab_category(category: u32) -> Option<MediaType> {
    // Anime is a TV subcategory — check before the 5000 bucket.
    if category == 5070 || (5070..5080).contains(&category) {
        return Some(MediaType::Anime);
    }
    match category / 1000 {
        1 => Some(MediaType::Game),
        2 => Some(MediaType::Movie),
        3 => Some(MediaType::Music),
        4 => Some(MediaType::Software),
        5 => Some(MediaType::Tv),
        6 => Some(MediaType::Porn),
        7 => Some(MediaType::Book),
        // 8xxx Other / unknown → heuristics
        _ => None,
    }
}

/// Classify a torrent from its display name and relative file paths.
///
/// Order of checks (first match wins for strong signals):
/// 1. Adult / porn keywords
/// 2. Music extensions / cues
/// 3. Book extensions
/// 4. Game package cues
/// 5. Software installers
/// 6. Anime release-group / fansub cues
/// 7. TV episode patterns (`S01E01`, `1x02`, `Season`)
/// 8. Movie cues (year + video ext, BluRay/WEB-DL without episode tokens)
/// 9. Fallback: dominant file extension, else Other
///
/// This is intentionally imperfect. Prefer disabling auto-organize when accuracy matters.
pub fn classify_media(torrent_name: &str, file_paths: &[impl AsRef<str>]) -> MediaType {
    let mut hay = torrent_name.to_lowercase();
    hay.push(' ');
    for p in file_paths {
        hay.push_str(&p.as_ref().to_lowercase());
        hay.push(' ');
    }

    if contains_any(
        &hay,
        &[
            "xxx",
            "porn",
            "onlyfans",
            "adults-only",
            "adult only",
            "jav ",
            " jav",
            "hentai",
            "nsfw",
            "brazzers",
            "reality kings",
            "blacked",
            "vixen",
        ],
    ) {
        return MediaType::Porn;
    }

    let exts = collect_extensions(file_paths);
    if dominant_is(
        &exts,
        &["mp3", "flac", "m4a", "aac", "ogg", "wav", "alac", "ape", "wma"],
    ) || contains_any(&hay, &["discography", "vinyl", " ost", "soundtrack", "flac "])
    {
        return MediaType::Music;
    }

    if dominant_is(
        &exts,
        &["epub", "mobi", "azw", "azw3", "pdf", "cbz", "cbr", "djvu"],
    ) || contains_any(&hay, &["ebook", "audiobook", "epub"])
    {
        return MediaType::Book;
    }

    if contains_any(
        &hay,
        &[
            "fitgirl",
            "dodge",
            "gog.com",
            "steamrip",
            "online-fix",
            ".nsp",
            ".xci",
            ".vpk",
            "nintendo switch",
            "ps4",
            "ps5",
            "xbox",
            "roms",
        ],
    ) || dominant_is(&exts, &["nsp", "xci", "vpk", "cia", "3ds", "nds", "iso"])
        && contains_any(&hay, &["game", "repack", "gog", "steam"])
    {
        return MediaType::Game;
    }

    if dominant_is(
        &exts,
        &["exe", "msi", "dmg", "pkg", "appimage", "deb", "rpm"],
    ) || contains_any(&hay, &["setup.exe", "installer", "x64-setup", "win64"])
    {
        return MediaType::Software;
    }

    // Anime before TV: fansub brackets + episode often look like TV otherwise.
    if looks_like_anime(&hay) {
        return MediaType::Anime;
    }

    if looks_like_tv(&hay) {
        return MediaType::Tv;
    }

    if looks_like_movie(&hay, &exts) {
        return MediaType::Movie;
    }

    if dominant_is(
        &exts,
        &["mkv", "mp4", "avi", "m4v", "mov", "wmv", "ts", "m2ts"],
    ) {
        // Video without clear TV/movie markers — lean movie for single-file, TV for many.
        if file_paths.len() <= 2 {
            return MediaType::Movie;
        }
        return MediaType::Tv;
    }

    MediaType::Other
}

fn looks_like_anime(hay: &str) -> bool {
    const GROUPS: &[&str] = &[
        "[subsplease]",
        "[erai-raws]",
        "[horriblesubs]",
        "[judas]",
        "[ember]",
        "[asw]",
        "[nep_blanc]",
        "[toonsouth]",
        "fansub",
        "dual audio",
        "dual-audio",
        "bdrip",
        "anime",
    ];
    if contains_any(hay, GROUPS) {
        return true;
    }
    // Common anime episode token: ` - 01 [` or ` - 12 (`
    let bytes = hay.as_bytes();
    for i in 0..bytes.len().saturating_sub(6) {
        if bytes[i] == b'-'
            && bytes[i + 1] == b' '
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && (bytes[i + 4] == b' ' || bytes[i + 4] == b'[')
        {
            return true;
        }
    }
    false
}

fn looks_like_tv(hay: &str) -> bool {
    if contains_any(
        hay,
        &[
            "season ",
            "season.",
            "complete series",
            "episode ",
            "hdtv",
            "webrip",
            "web-dl",
        ],
    ) {
        // WEB-DL alone is also movies — require episode-ish token below or Season.
        if hay.contains("season") || hay.contains("episode") || hay.contains("complete series") {
            return true;
        }
    }
    // S01E01 / s01e02
    if has_season_episode_token(hay) {
        return true;
    }
    // 1x02 style
    let b = hay.as_bytes();
    for i in 0..b.len().saturating_sub(3) {
        if b[i].is_ascii_digit() && b[i + 1] == b'x' && b[i + 2].is_ascii_digit() {
            return true;
        }
    }
    false
}

fn has_season_episode_token(hay: &str) -> bool {
    let b = hay.as_bytes();
    for i in 0..b.len().saturating_sub(5) {
        if (b[i] == b's' || b[i] == b'S')
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
            && (b[i + 3] == b'e' || b[i + 3] == b'E')
            && b[i + 4].is_ascii_digit()
        {
            return true;
        }
    }
    false
}

fn looks_like_movie(hay: &str, exts: &[(String, usize)]) -> bool {
    if has_season_episode_token(hay) {
        return false;
    }
    let has_video = dominant_is(
        exts,
        &["mkv", "mp4", "avi", "m4v", "mov", "wmv", "ts", "m2ts", "iso"],
    ) || contains_any(hay, &[".mkv", ".mp4", ".avi"]);
    if !has_video && !contains_any(hay, &["bluray", "blu-ray", "web-dl", "webrip", "remux", "hdtv"])
    {
        return false;
    }
    // Year token 19xx / 20xx
    if has_year_token(hay) {
        return true;
    }
    contains_any(
        hay,
        &[
            "bluray",
            "blu-ray",
            "remux",
            "2160p",
            "1080p",
            "720p",
            "web-dl",
            "webrip",
        ],
    )
}

fn has_year_token(hay: &str) -> bool {
    let b = hay.as_bytes();
    for i in 0..b.len().saturating_sub(4) {
        let y = &b[i..i + 4];
        if y.iter().all(|c| c.is_ascii_digit()) {
            let year: u32 = (y[0] - b'0') as u32 * 1000
                + (y[1] - b'0') as u32 * 100
                + (y[2] - b'0') as u32 * 10
                + (y[3] - b'0') as u32;
            if (1950..=2099).contains(&year) {
                let before_ok = i == 0 || !b[i - 1].is_ascii_digit();
                let after_ok = i + 4 >= b.len() || !b[i + 4].is_ascii_digit();
                if before_ok && after_ok {
                    return true;
                }
            }
        }
    }
    false
}

fn collect_extensions(file_paths: &[impl AsRef<str>]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut map: HashMap<String, usize> = HashMap::new();
    for p in file_paths {
        let s = p.as_ref().to_lowercase();
        let ext = s.rsplit('.').next().unwrap_or("");
        if ext.is_empty() || ext.len() > 8 || ext.contains('/') {
            continue;
        }
        *map.entry(ext.to_string()).or_insert(0) += 1;
    }
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn dominant_is(exts: &[(String, usize)], want: &[&str]) -> bool {
    let Some((ext, _)) = exts.first() else {
        return false;
    };
    want.iter().any(|w| *w == ext.as_str())
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_tv_episode() {
        assert_eq!(
            classify_media("Show.Name.S02E03.1080p.WEB-DL", &["Show.S02E03.mkv"]),
            MediaType::Tv
        );
    }

    #[test]
    fn classifies_anime_subsplease() {
        assert_eq!(
            classify_media(
                "[SubsPlease] Cool Anime - 12 (1080p) [ABC].mkv",
                &["[SubsPlease] Cool Anime - 12 (1080p) [ABC].mkv"]
            ),
            MediaType::Anime
        );
    }

    #[test]
    fn classifies_movie_year() {
        assert_eq!(
            classify_media("Cool.Movie.2019.1080p.BluRay.x264", &["Cool.Movie.2019.mkv"]),
            MediaType::Movie
        );
    }

    #[test]
    fn classifies_music_flac() {
        assert_eq!(
            classify_media("Artist - Album (2020) [FLAC]", &["01.Track.flac", "02.Track.flac"]),
            MediaType::Music
        );
    }

    #[test]
    fn torznab_5070_beats_tv_bucket() {
        assert_eq!(
            media_type_from_torznab_category(5070),
            Some(MediaType::Anime)
        );
        assert_eq!(media_type_from_torznab_category(5000), Some(MediaType::Tv));
        assert_eq!(media_type_from_torznab_category(2000), Some(MediaType::Movie));
        assert_eq!(media_type_from_torznab_category(8000), None);
    }
}
