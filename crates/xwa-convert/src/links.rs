//! Resolving the links in an article's HTML to article ids.
//!
//! mwoffliner writes links as URLs relative to the article's own path
//! (`Mathematics`, `../Foo`, `Bar#History`). Every path in the archive is
//! kept as a 64-bit hash next to the article id it leads to, after following
//! redirects: 12 bytes per title, about 220 MB for all of English Wikipedia.

use percent_encoding::percent_decode_str;
use xxhash_rust::xxh3::xxh3_64;

/// Sorted (path hash, article id) pairs.
#[derive(Debug, Default)]
pub struct PathTable {
    entries: Vec<(u64, u32)>,
}

impl PathTable {
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, u32)>) -> Self {
        let mut entries: Vec<(u64, u32)> = pairs
            .into_iter()
            .map(|(p, id)| (xxh3_64(p.as_bytes()), id))
            .collect();
        entries.sort_unstable();
        entries.dedup_by_key(|e| e.0);
        Self { entries }
    }

    /// From pairs whose paths are already hashed with [`path_hash`].
    pub fn from_hashed(mut entries: Vec<(u64, u32)>) -> Self {
        entries.sort_unstable();
        entries.dedup_by_key(|e| e.0);
        Self { entries }
    }

    pub fn into_entries(self) -> Vec<(u64, u32)> {
        self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Article id for a decoded archive path.
    pub fn get(&self, path: &str) -> Option<u32> {
        let h = xxh3_64(path.as_bytes());
        self.entries
            .binary_search_by_key(&h, |e| e.0)
            .ok()
            .map(|i| self.entries[i].1)
    }
}

/// The hash [`PathTable`] keys paths by.
pub fn path_hash(path: &str) -> u64 {
    xxh3_64(path.as_bytes())
}

/// The target of an HTML redirect page (`<meta http-equiv="refresh"
/// content="0;URL='./Frog#Call'">`), which mwoffliner writes for redirects
/// to a section. Only small pages are considered.
pub fn refresh_target(html: &[u8]) -> Option<String> {
    if html.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(html).ok()?;
    let lower = text.to_ascii_lowercase();
    let meta = lower
        .find("http-equiv=\"refresh\"")
        .or_else(|| lower.find("http-equiv='refresh'"))?;
    let url_at = meta + lower[meta..].find("url=")? + 4;
    let rest = &text[url_at..];
    let url = match rest.chars().next()? {
        q @ ('\'' | '"') => rest[1..].split(q).next()?,
        _ => rest.split(['"', '\'', '>', ' ']).next()?,
    };
    let url = url
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    (!url.is_empty()).then_some(url)
}

/// The MediaWiki namespace a page says it is in (`"wgNamespaceNumber":14`
/// for a category, 0 for an article), from the script near the top of the
/// page. `None` for pages without one, such as mwoffliner's own main page.
pub fn page_namespace(html: &[u8]) -> Option<i32> {
    const KEY: &[u8] = b"\"wgNamespaceNumber\":";
    let head = &html[..html.len().min(64 << 10)];
    let at = head.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let rest = &head[at..];
    let end = rest
        .iter()
        .position(|b| !(b.is_ascii_digit() || *b == b'-'))
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()?.parse().ok()
}

/// A link target inside the archive: the path and an optional fragment,
/// both percent-decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Href {
    pub path: String,
    pub fragment: Option<String>,
}

/// Resolves `href` against the path of the article it appears in. Returns
/// `None` for links that leave the archive (absolute URLs, `mailto:` and
/// the like) or climb above its root.
pub fn resolve_href(base: &str, href: &str) -> Option<Href> {
    let href = href.trim();
    let (rest, fragment) = match href.split_once('#') {
        Some((r, f)) => (r, Some(f)),
        None => (href, None),
    };
    let rest = rest.split_once('?').map_or(rest, |(r, _)| r);
    // A link out has a known scheme or "scheme://". Titles may contain a
    // colon themselves ("Star_Trek:_The_Next_Generation").
    if let Some(colon) = rest.find(':')
        && !rest[..colon].contains('/')
    {
        const SCHEMES: &[&str] = &[
            "http",
            "https",
            "ftp",
            "mailto",
            "tel",
            "geo",
            "javascript",
            "data",
            "irc",
            "news",
            "urn",
            "file",
            "about",
        ];
        let scheme = rest[..colon].to_ascii_lowercase();
        if SCHEMES.contains(&scheme.as_str()) || rest[colon + 1..].starts_with("//") {
            return None;
        }
    }
    if rest.starts_with("//") {
        return None;
    }
    let fragment = fragment.filter(|f| !f.is_empty()).map(decode);
    if rest.is_empty() {
        return Some(Href {
            path: base.to_string(),
            fragment,
        });
    }
    let mut segments: Vec<&str> = if rest.starts_with('/') {
        Vec::new()
    } else {
        let mut s: Vec<&str> = base.split('/').collect();
        s.pop(); // the base article's own name
        s
    };
    for seg in rest.trim_start_matches('/').split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                segments.pop()?;
            }
            s => segments.push(s),
        }
    }
    let path = decode(&segments.join("/"));
    Some(Href { path, fragment })
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(base: &str, href: &str) -> Option<(String, Option<String>)> {
        resolve_href(base, href).map(|h| (h.path, h.fragment))
    }

    #[test]
    fn resolves_relative_links() {
        assert_eq!(
            r("Pythagorean_theorem", "Mathematics"),
            Some(("Mathematics".into(), None))
        );
        assert_eq!(
            r("Pythagorean_theorem", "./Euclid%27s_Elements#Book_I"),
            Some(("Euclid's_Elements".into(), Some("Book_I".into())))
        );
        assert_eq!(
            r("AC/DC", "../Rock_music"),
            Some(("Rock_music".into(), None))
        );
        assert_eq!(
            r("AC/DC", "Back_in_Black"),
            Some(("AC/Back_in_Black".into(), None))
        );
        assert_eq!(
            r("A", "#History"),
            Some(("A".into(), Some("History".into())))
        );
        assert_eq!(r("A", "Caf%C3%A9?action=x"), Some(("Café".into(), None)));
        assert_eq!(
            r("A", "Ratio:_golden"),
            Some(("Ratio:_golden".into(), None))
        );
        assert_eq!(
            r("A", "./Star_Trek:_Picard"),
            Some(("Star_Trek:_Picard".into(), None))
        );
    }

    #[test]
    fn rejects_links_out() {
        assert_eq!(r("A", "https://example.org/x"), None);
        assert_eq!(r("A", "mailto:someone@example.org"), None);
        assert_eq!(r("A", "//example.org/x"), None);
        assert_eq!(r("A", "HTTPS://example.org/x"), None);
        assert_eq!(r("A", "gopher://example.org/x"), None);
        assert_eq!(r("A", "../../outside"), None);
    }

    #[test]
    fn finds_refresh_targets() {
        let page = br#"<html><head><title>Frog call</title>
            <meta http-equiv="refresh" content="0;URL='./Frog#Call'" /></head>
            <body><a href="./Frog#Call">Frog call</a></body></html>"#;
        assert_eq!(refresh_target(page).as_deref(), Some("./Frog#Call"));
        assert_eq!(
            refresh_target(br#"<meta HTTP-EQUIV="Refresh" content="0; url=../A&amp;B">"#)
                .as_deref(),
            Some("../A&B")
        );
        assert_eq!(
            refresh_target(b"<html><body><p>An article</p></body></html>"),
            None
        );
    }

    #[test]
    fn reads_page_namespaces() {
        let page = br#"<script>RLCONF = {"wgCanonicalNamespace":"Category","wgNamespaceNumber":14,"wgPageName":"X"};</script>"#;
        assert_eq!(page_namespace(page), Some(14));
        assert_eq!(page_namespace(br#"{"wgNamespaceNumber":0,"#), Some(0));
        assert_eq!(page_namespace(br#"{"wgNamespaceNumber":-1}"#), Some(-1));
        assert_eq!(page_namespace(b"<html><body>Welcome</body></html>"), None);
    }

    #[test]
    fn table_finds_paths() {
        let t = PathTable::from_pairs([("Mathematics", 3), ("Café", 9)]);
        assert_eq!(t.get("Mathematics"), Some(3));
        assert_eq!(t.get("Café"), Some(9));
        assert_eq!(t.get("Physics"), None);
    }
}
