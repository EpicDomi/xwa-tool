//! The fallback font catalogue: for each Noto family the converter may
//! draw fallback glyphs from (spec 10), in the order it looks in them, the
//! files of its styles on Google Fonts (regular, and bold, italic and bold
//! italic where the family has them), their sizes and SHA-256, and the
//! code points each covers.
//!
//! One line per file: family, style (`regular`, `bold`, `italic` or
//! `bold-italic`), URL, SHA-256, size in bytes, then its coverage as
//! comma-separated hexadecimal ranges (`4E00-9FFF`), all tab-separated; a
//! family's regular file comes first. Compressed with zstd. Converters
//! download a file only when an archive needs it, and check it against
//! this.

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
fn families(agent: &ureq::Agent) -> Result<Vec<(String, Vec<&'static str>)>, String> {
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
    let list = json["familyMetadataList"]
        .as_array()
        .ok_or("Google Fonts: no family list")?;
    // The styles besides regular each family has, by Google's names for
    // them ("700", "400i", "700i").
    let styles = |name: &str| -> Vec<&'static str> {
        let fonts = list
            .iter()
            .find(|f| f["family"].as_str() == Some(name))
            .and_then(|f| f["fonts"].as_object());
        STYLES
            .iter()
            .filter(|(_, key)| fonts.is_some_and(|f| f.contains_key(*key)))
            .map(|(style, _)| *style)
            .collect()
    };
    let mut all: Vec<String> = list
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
    Ok(out
        .into_iter()
        .map(|f| {
            let s = styles(&f);
            (f, s)
        })
        .collect())
}

/// The styles besides regular, and Google Fonts' names for them.
const STYLES: &[(&str, &str)] = &[("bold", "700"), ("italic", "400i"), ("bold-italic", "700i")];

/// Wikimedia and Google ask tools to say who they are.
const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

pub(crate) fn agent() -> ureq::Agent {
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

/// The URL of one style of a family as one TrueType file. Google Fonts
/// serves whole files to clients that are not browsers.
fn font_url(
    agent: &ureq::Agent,
    family: &str,
    style: &str,
    has_italic: bool,
) -> Result<String, String> {
    let axes = match (style, has_italic) {
        ("regular", _) => "",
        ("bold", false) => ":wght@700",
        ("bold", true) => ":ital,wght@0,700",
        ("italic", _) => ":ital,wght@1,400",
        _ => ":ital,wght@1,700",
    };
    let url = format!(
        "https://fonts.googleapis.com/css2?family={}{axes}",
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
    let mut files = 0;
    for (family, styles) in &families {
        let has_italic = styles.iter().any(|s| s.contains("italic"));
        for style in std::iter::once(&"regular").chain(styles) {
            let line = file_line(&agent, family, style, has_italic, &mut total)?;
            table.push_str(&line);
            files += 1;
        }
    }
    let packed = zstd::encode_all(table.as_bytes(), 19).map_err(|e| e.to_string())?;
    let out = root.join("crates/xwa-convert/data/fonts.tsv.zst");
    std::fs::write(&out, &packed).map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!(
        "wrote {} families in {files} files ({} MB in all) to {} ({} bytes)",
        families.len(),
        total >> 20,
        out.display(),
        packed.len()
    );
    Ok(())
}

/// The catalogue line for one style of a family.
fn file_line(
    agent: &ureq::Agent,
    family: &str,
    style: &str,
    has_italic: bool,
    total: &mut u64,
) -> Result<String, String> {
    let url = font_url(agent, family, style, has_italic)?;
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
    let ranges = coverage(&data).map_err(|e| format!("{family} {style}: {e}"))?;
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
        "{family} {style}: {} KB, {} code points",
        data.len() / 1024,
        ranges.iter().map(|(a, b)| b - a + 1).sum::<u32>()
    );
    *total += data.len() as u64;
    Ok(format!(
        "{family}\t{style}\t{url}\t{sha}\t{}\t{}\n",
        data.len(),
        cover.join(",")
    ))
}

/// The catalogue of the fonts for the converter's optional "rare
/// characters", in the order they are looked in after the Noto fonts:
///
/// - the designs of Noto scripts the main catalogue leaves out, for what
///   only they have (Noto Sans Tamil Supplement, for one);
/// - Plangothic, for the CJK ideographs of Unicode's extensions B to J,
///   which Noto has only the commonest of, and many of the newest letters
///   and symbols;
/// - NewGardiner, for the Egyptian hieroglyphs of Extended-A (Unicode 16),
///   which Noto Sans Egyptian Hieroglyphs lacks;
/// - GNU Unifont and Unifont Upper, drawn from pixels, as the last resort.
///
/// A file is listed only where it has a glyph no font before it has. Each
/// line has a seventh column: the mean size in bytes, in FONT's encoding
/// (spec 10), of the glyphs it adds, for estimating what the option costs.
/// Writes `crates/xwa-convert/data/fonts-rare.tsv.zst`. A source that
/// cannot be had is left out, and the task then fails after writing.
pub fn write_rare(root: &Path) -> Result<(), String> {
    let agent = agent();
    // Code points no font draws (unassigned, private use, controls) are
    // never counted as added, nor are those the Noto fonts have.
    let mut covered = vec![false; 0x11_0000];
    let none = root.join("crates/xwa-convert/data/no-character.txt");
    let none = std::fs::read_to_string(&none).map_err(|e| format!("{}: {e}", none.display()))?;
    for line in none.lines().filter(|l| !l.starts_with('#')) {
        for (a, b) in parse_ranges(line) {
            covered[a as usize..=b as usize].fill(true);
        }
    }
    let main = root.join("crates/xwa-convert/data/fonts.tsv.zst");
    let packed = std::fs::read(&main).map_err(|e| format!("{}: {e}", main.display()))?;
    let table = zstd::decode_all(&packed[..]).map_err(|e| e.to_string())?;
    for line in String::from_utf8_lossy(&table).lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 6 && f[1] == "regular" {
            for (a, b) in parse_ranges(f[5]) {
                covered[a as usize..=b as usize].fill(true);
            }
        }
    }

    let mut sources: Vec<(String, Result<String, String>)> = Vec::new();
    for family in noto_variants(&agent)? {
        let url = font_url(&agent, &family, "regular", false);
        sources.push((family, url));
    }
    match plangothic(&agent) {
        Ok(files) => sources.extend(files.into_iter().map(|(n, u)| (n, Ok(u)))),
        Err(e) => sources.push(("Plangothic".into(), Err(e))),
    }
    match newgardiner() {
        Ok(files) => sources.extend(files.into_iter().map(|(n, u)| (n, Ok(u)))),
        Err(e) => sources.push(("NewGardiner".into(), Err(e))),
    }
    match unifont(&agent) {
        Ok(files) => sources.extend(files.into_iter().map(|(n, u)| (n, Ok(u)))),
        Err(e) => sources.push(("Unifont".into(), Err(e))),
    }

    let mut table = String::new();
    let mut failed = Vec::new();
    for (family, url) in sources {
        let got = url.and_then(|url| {
            let data = download(&agent, &url).map_err(|e| format!("{family}: {e}"))?;
            Ok((url, data))
        });
        let (url, data) = match got {
            Ok(g) => g,
            Err(e) => {
                eprintln!("{family}: could not be had: {e}");
                failed.push(e);
                continue;
            }
        };
        let ranges = coverage(&data).map_err(|e| format!("{family}: {e}"))?;
        let face = ttf_parser::Face::parse(&data, 0).map_err(|e| format!("{family}: {e}"))?;
        let mut added = 0u64;
        let mut bytes = 0u64;
        for &(a, b) in &ranges {
            for cp in a..=b {
                if covered[cp as usize] {
                    continue;
                }
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                if let Some(n) = glyph_bytes(&face, c) {
                    added += 1;
                    bytes += n;
                }
            }
        }
        if added == 0 {
            eprintln!("{family}: adds nothing, left out");
            continue;
        }
        for &(a, b) in &ranges {
            covered[a as usize..=b as usize].fill(true);
        }
        let mean = bytes / added;
        eprintln!(
            "{family}: {} KB, adds {added} code points, {mean} bytes a glyph",
            data.len() / 1024
        );
        let sha: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        table.push_str(&format!(
            "{family}\tregular\t{url}\t{sha}\t{}\t{}\t{mean}\n",
            data.len(),
            format_ranges(&ranges)
        ));
    }
    let packed = zstd::encode_all(table.as_bytes(), 19).map_err(|e| e.to_string())?;
    let out = root.join("crates/xwa-convert/data/fonts-rare.tsv.zst");
    std::fs::write(&out, &packed).map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!("wrote {} ({} bytes)", out.display(), packed.len());
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the catalogue was written without {} source(s): {}",
            failed.len(),
            failed.join("; ")
        ))
    }
}

/// The Noto families on Google Fonts that the main catalogue leaves out as
/// other designs of a script ([`unwanted`]), except the colour emoji, which
/// has no outlines.
fn noto_variants(agent: &ureq::Agent) -> Result<Vec<String>, String> {
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
    let mut out: Vec<String> = json["familyMetadataList"]
        .as_array()
        .ok_or("Google Fonts: no family list")?
        .iter()
        .filter_map(|f| f["family"].as_str())
        .filter(|f| f.starts_with("Noto ") && unwanted(f) && *f != "Noto Color Emoji")
        .map(str::to_string)
        .collect();
    out.sort();
    Ok(out)
}

/// Plangothic's latest release on GitHub: its TrueType files, in order
/// (the font is split in parts, P1, P2 and so on, as one file can hold at
/// most 65,535 glyphs).
fn plangothic(agent: &ureq::Agent) -> Result<Vec<(String, String)>, String> {
    let release = agent
        .get("https://api.github.com/repos/Fitzgerald-Porthmouth-Koenigsegg/Plangothic-Project/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Plangothic's releases: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Plangothic's releases: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&release).map_err(|e| e.to_string())?;
    let assets = json["assets"].as_array().ok_or("Plangothic: no assets")?;
    let mut parts: Vec<(u32, String, String)> = assets
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?;
            let part: u32 = name
                .strip_prefix("PlangothicP")?
                .strip_suffix("-Regular.ttf")?
                .parse()
                .ok()?;
            let url = a["browser_download_url"].as_str()?;
            Some((part, format!("Plangothic P{part}"), url.to_string()))
        })
        .collect();
    parts.sort();
    if parts.is_empty() {
        return Err("Plangothic: no PlangothicP*-Regular.ttf in the latest release".into());
    }
    Ok(parts.into_iter().map(|(_, n, u)| (n, u)).collect())
}

/// NewGardiner's fonts at the newest commit of its repository, which is
/// where it is published: the core signs, then the few non-core ones.
fn newgardiner() -> Result<Vec<(String, String)>, String> {
    const REPO: &str = "https://github.com/nederhof/newgardiner";
    let out = std::process::Command::new("git")
        .args(["ls-remote", REPO, "refs/heads/main"])
        .output()
        .map_err(|e| format!("NewGardiner: git: {e}"))?;
    let commit = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .filter(|c| c.len() == 40)
        .map(str::to_string)
        .ok_or("NewGardiner: no main branch")?;
    let raw = format!("https://raw.githubusercontent.com/nederhof/newgardiner/{commit}/fonts");
    Ok(vec![
        ("NewGardiner".into(), format!("{raw}/NewGardiner.ttf")),
        (
            "NewGardiner Non-Core".into(),
            format!("{raw}/NewGardinerNonCore.ttf"),
        ),
    ])
}

/// GNU Unifont's latest version: its fonts for the Basic Multilingual Plane
/// and for the planes above it, as OpenType outlines of its pixels.
fn unifont(agent: &ureq::Agent) -> Result<Vec<(String, String)>, String> {
    const BASE: &str = "https://ftp.gnu.org/gnu/unifont/";
    let index = agent
        .get(BASE)
        .call()
        .map_err(|e| format!("Unifont: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Unifont: {e}"))?;
    let version = index
        .split("href=\"unifont-")
        .skip(1)
        .filter_map(|s| s.split('/').next())
        .filter_map(|v| {
            let parts: Option<Vec<u32>> = v.split('.').map(|p| p.parse().ok()).collect();
            parts.map(|p| (p, v.to_string()))
        })
        .max()
        .map(|(_, v)| v)
        .ok_or("Unifont: no version listed")?;
    Ok(vec![
        (
            "Unifont".into(),
            format!("{BASE}unifont-{version}/unifont-{version}.otf"),
        ),
        (
            "Unifont Upper".into(),
            format!("{BASE}unifont-{version}/unifont_upper-{version}.otf"),
        ),
    ])
}

fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    agent
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// The size of `c`'s glyph in FONT's encoding (spec 10): its bounding box,
/// each command (move and line 5 bytes, quadratic 9, cubic 13, close 1)
/// and END, and its advance and offset in the tables.
fn glyph_bytes(face: &ttf_parser::Face, c: char) -> Option<u64> {
    struct Count(u64);
    impl ttf_parser::OutlineBuilder for Count {
        fn move_to(&mut self, _: f32, _: f32) {
            self.0 += 5;
        }
        fn line_to(&mut self, _: f32, _: f32) {
            self.0 += 5;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 9;
        }
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 13;
        }
        fn close(&mut self) {
            self.0 += 1;
        }
    }
    let id = face.glyph_index(c).filter(|g| g.0 != 0)?;
    let mut n = Count(8 + 1 + 6);
    face.outline_glyph(id, &mut n);
    Some(n.0)
}

fn parse_ranges(s: &str) -> Vec<(u32, u32)> {
    s.split(',')
        .filter_map(|r| {
            let (a, b) = r.split_once('-').unwrap_or((r, r));
            Some((
                u32::from_str_radix(a, 16).ok()?,
                u32::from_str_radix(b, 16).ok()?,
            ))
        })
        .collect()
}

fn format_ranges(ranges: &[(u32, u32)]) -> String {
    ranges
        .iter()
        .map(|&(a, b)| {
            if a == b {
                format!("{a:X}")
            } else {
                format!("{a:X}-{b:X}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
