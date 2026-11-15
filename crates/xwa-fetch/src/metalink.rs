//! Metalink 4 files (RFC 5854), which the Kiwix download server returns for
//! each ZIM: the file's size, its checksums, and mirrors in priority order.

use crate::Error;
use crate::xml;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metalink {
    pub name: String,
    pub size: u64,
    /// Lowercase hex SHA-256, if the file lists one.
    pub sha256: Option<String>,
    /// Download URLs, best first.
    pub urls: Vec<String>,
}

pub fn parse(doc: &str) -> Result<Metalink, Error> {
    let root = xml::parse(doc)?;
    let file = root
        .child("file")
        .ok_or_else(|| Error::Format("Metalink has no <file>".into()))?;
    let name = file.attr("name").unwrap_or_default().to_string();
    // A name with a path in it could write outside the download folder.
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(Error::Format(format!(
            "unusable file name in Metalink: {name:?}"
        )));
    }
    let size = file
        .child_text("size")
        .parse()
        .map_err(|_| Error::Format("Metalink has no size".into()))?;
    let sha256 = file
        .children_named("hash")
        .find(|h| h.attr("type") == Some("sha-256"))
        .map(|h| h.text.trim().to_ascii_lowercase())
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let mut urls: Vec<(u32, String)> = file
        .children_named("url")
        .map(|u| {
            (
                u.attr("priority")
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(u32::MAX),
                u.text.trim().to_string(),
            )
        })
        .filter(|(_, u)| u.starts_with("https://") || u.starts_with("http://"))
        .collect();
    urls.sort_by_key(|(p, _)| *p);
    if urls.is_empty() {
        return Err(Error::Format("Metalink lists no mirrors".into()));
    }
    Ok(Metalink {
        name,
        size,
        sha256,
        urls: urls.into_iter().map(|(_, u)| u).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_size_hash_and_mirrors() {
        let m = parse(include_str!("../tests/data/ray-charles.meta4")).unwrap();
        assert_eq!(m.name, "wikipedia_en_ray-charles_maxi_2026-10.zim");
        assert_eq!(m.size, 29_887_083);
        assert_eq!(
            m.sha256.as_deref(),
            Some("a0804b170cac5215edd673559fca430e07ef2ab4c4fe73a060fd37eab1b0b95e")
        );
        assert_eq!(m.urls.len(), 6);
        assert!(m.urls[0].starts_with("https://wi.mirror.driftle.ss/"));
        assert!(m.urls[5].starts_with("https://mirrors.dotsrc.org/"));
    }

    #[test]
    fn rejects_paths_in_names() {
        let doc = r#"<metalink><file name="../evil.zim"><size>1</size><url>https://x/y</url></file></metalink>"#;
        assert!(parse(doc).is_err());
    }
}
