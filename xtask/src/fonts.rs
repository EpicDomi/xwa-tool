//! The fallback font catalogue: for each Noto family the converter may
//! draw fallback glyphs from (spec 10), in the order it looks in them, the
//! regular style's file on Google Fonts, its size and SHA-256, and the code
//! points it covers.
//!
//! One line per font: family, URL, SHA-256, size in bytes, then its
//! coverage as comma-separated hexadecimal ranges (`4E00-9FFF`), all
//! tab-separated; compressed with zstd. Converters download a font only
//! when an archive needs it, and check it against this.

use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Where to look for a glyph first: Noto Sans, then the scripts Wikipedia
/// uses most. Every other Noto family on Google Fonts follows (see
/// [`families`]), and then [`LAST`].
const FIRST: &[&str] = &[
    "Noto Sans",
    // Each script's own font before the symbol fonts, which have some
    // letters too (Noto Sans Math has Arabic's), and those before the CJK
    // fonts, which have many symbols in a CJK style.
    "Noto Sans Arabic",
    "Noto Sans Hebrew",
    "Noto Sans Armenian",
    "Noto Sans Georgian",
    "Noto Sans Devanagari",
    "Noto Sans Bengali",
    "Noto Sans Gurmukhi",
    "Noto Sans Gujarati",
    "Noto Sans Oriya",
    "Noto Sans Tamil",
    "Noto Sans Telugu",
    "Noto Sans Kannada",
    "Noto Sans Malayalam",
    "Noto Sans Sinhala",
    "Noto Sans Thai",
    "Noto Sans Lao",
    "Noto Sans Khmer",
    "Noto Sans Myanmar",
    "Noto Serif Tibetan",
    "Noto Sans Ethiopic",
    "Noto Sans Cherokee",
    "Noto Sans Syriac",
    "Noto Sans Thaana",
    "Noto Sans Mongolian",
    "Noto Sans Canadian Aboriginal",
    "Noto Sans Tifinagh",
    "Noto Sans NKo",
    "Noto Sans Adlam",
    "Noto Sans Vai",
    "Noto Sans Ol Chiki",
    "Noto Sans Javanese",
    "Noto Sans Balinese",
    "Noto Sans Sundanese",
    "Noto Sans Tai Viet",
    "Noto Sans Lisu",
    "Noto Sans Yi",
    "Noto Sans Coptic",
    "Noto Sans Gothic",
    "Noto Sans Runic",
    "Noto Sans Ogham",
    "Noto Sans Old Italic",
    "Noto Sans Phoenician",
    "Noto Sans Samaritan",
    "Noto Sans Glagolitic",
    "Noto Sans Linear B",
    "Noto Sans Cuneiform",
    "Noto Sans Egyptian Hieroglyphs",
];

/// Where to look last: the symbol fonts, which have some letters too (Noto
/// Sans Math has Arabic's), then the CJK fonts, which have many symbols in
/// a CJK style, as a group the converter reorders by the archive's
/// language, then emoji.
const LAST: &[&str] = &[
    "Noto Sans Math",
    "Noto Sans Symbols",
    "Noto Sans Symbols 2",
    "Noto Music",
    "Noto Znamenny Musical Notation",
    "Noto Sans SC",
    "Noto Sans TC",
    "Noto Sans HK",
    "Noto Sans JP",
    "Noto Sans KR",
    "Noto Emoji",
];

/// Families on Google Fonts that are not for this: other designs of a
/// script whose Sans is there, and fonts without outlines.
fn unwanted(family: &str) -> bool {
    let variant = [
        "Mono",
        "Display",
        "Unjoined",
        "Looped",
        "Eastern",
        "Western",
        "Supplement",
        "UI",
    ];
    family == "Noto Color Emoji"
        || family == "Noto Serif"
        || family.split(' ').any(|w| variant.contains(&w))
}

/// Every family to catalogue, in search order: [`FIRST`], the other Noto
/// Sans families, the Noto Serif ones for scripts without a Sans, the
/// remaining Noto families, then [`LAST`].
fn families(agent: &ureq::Agent) -> Result<Vec<String>, String> {
    let meta = agent
        .get("https://fonts.google.com/metadata/fonts")
        .call()
        .map_err(|e| format!("Google Fonts: {e}"))?
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| format!("Google Fonts: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(meta.trim_start_matches(")]}'")).map_err(|e| e.to_string())?;
    let mut all: Vec<String> = json["familyMetadataList"]
        .as_array()
        .ok_or("Google Fonts: no family list")?
        .iter()
        .filter_map(|f| f["family"].as_str())
        .filter(|f| f.starts_with("Noto ") && !unwanted(f))
        .map(str::to_string)
        .collect();
    all.sort();
    let placed = |f: &str| FIRST.contains(&f) || LAST.contains(&f);
    let script = |f: &str, kind: &str| f.strip_prefix(kind).map(str::to_string);
    let sans: Vec<String> = all.iter().filter_map(|f| script(f, "Noto Sans ")).collect();
    let mut out: Vec<String> = FIRST.iter().map(|f| f.to_string()).collect();
    out.extend(
        all.iter()
            .filter(|f| f.starts_with("Noto Sans ") && !placed(f))
            .cloned(),
    );
    out.extend(
        all.iter()
            .filter(|f| script(f, "Noto Serif ").is_some_and(|s| !sans.contains(&s)) && !placed(f))
            .cloned(),
    );
    out.extend(
        all.iter()
            .filter(|f| !out.contains(f) && !placed(f) && !f.starts_with("Noto Serif "))
            .cloned()
            .collect::<Vec<_>>(),
    );
    out.extend(LAST.iter().map(|f| f.to_string()));
    Ok(out)
}

/// Wikimedia and Google ask tools to say who they are.
const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into()
}

/// The URL of a family's regular style as one TrueType file. Google Fonts
/// serves whole files to clients that are not browsers.
fn font_url(agent: &ureq::Agent, family: &str) -> Result<String, String> {
    let url = format!(
        "https://fonts.googleapis.com/css2?family={}",
        family.replace(' ', "+")
    );
    let css = agent
        .get(&url)
        .call()
        .map_err(|e| format!("{family}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{family}: {e}"))?;
    let start = css
        .find("url(")
        .ok_or_else(|| format!("{family}: no font in {url}"))?
        + 4;
    let end = start + css[start..].find(')').ok_or("bad CSS")?;
    let found = css[start..end].trim_matches(['\'', '"']).to_string();
    if !found.ends_with(".ttf") {
        return Err(format!("{family}: {found} is not a TrueType file"));
    }
    Ok(found)
}

/// The code points a font maps, as ranges.
fn coverage(data: &[u8]) -> Result<Vec<(u32, u32)>, String> {
    let face = ttf_parser::Face::parse(data, 0).map_err(|e| e.to_string())?;
    let mut cps: Vec<u32> = Vec::new();
    for sub in face.tables().cmap.ok_or("no cmap")?.subtables {
        if sub.is_unicode() {
            sub.codepoints(|c| {
                if sub.glyph_index(c).is_some_and(|g| g.0 != 0) {
                    cps.push(c)
                }
            });
        }
    }
    cps.sort_unstable();
    cps.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for c in cps {
        match ranges.last_mut() {
            Some(r) if r.1 + 1 == c => r.1 = c,
            _ => ranges.push((c, c)),
        }
    }
    Ok(ranges)
}

pub fn write(root: &Path) -> Result<(), String> {
    let agent = agent();
    let families = families(&agent)?;
    let mut table = String::new();
    let mut total = 0u64;
    for family in &families {
        let url = font_url(&agent, family)?;
        let data = agent
            .get(&url)
            .call()
            .map_err(|e| format!("{family}: {e}"))?
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()
            .map_err(|e| format!("{family}: {e}"))?;
        let sha: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let ranges = coverage(&data).map_err(|e| format!("{family}: {e}"))?;
        let cover: Vec<String> = ranges
            .iter()
            .map(|&(a, b)| {
                if a == b {
                    format!("{a:X}")
                } else {
                    format!("{a:X}-{b:X}")
                }
            })
            .collect();
        eprintln!(
            "{family}: {} KB, {} code points",
            data.len() / 1024,
            ranges.iter().map(|(a, b)| b - a + 1).sum::<u32>()
        );
        total += data.len() as u64;
        table.push_str(&format!(
            "{family}\t{url}\t{sha}\t{}\t{}\n",
            data.len(),
            cover.join(",")
        ));
    }
    let packed = zstd::encode_all(table.as_bytes(), 19).map_err(|e| e.to_string())?;
    let out = root.join("crates/xwa-convert/data/fonts.tsv.zst");
    std::fs::write(&out, &packed).map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!(
        "wrote {} fonts ({} MB in all) to {} ({} bytes)",
        families.len(),
        total >> 20,
        out.display(),
        packed.len()
    );
    Ok(())
}
