//! What the fallback font can draw of what Unicode assigns, by block: a
//! report in Markdown for the weekly check, which opens an issue with it
//! when Unicode or a font changes, so new characters no font draws are seen
//! (`.github/workflows/watch.yml`).
//!
//! Reads the catalogues in `crates/xwa-convert/data` and the latest Unicode
//! Character Database; characters the e-reader's own fonts draw come from
//! Noto Sans in the catalogue, which has them all.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::unicode::{ucd_file, version_of};

/// A catalogue's families and the code points of their regular styles.
type Coverage = Vec<(String, Vec<(u32, u32)>)>;

fn catalogue(root: &Path, name: &str) -> Result<Coverage, String> {
    let path = root.join("crates/xwa-convert/data").join(name);
    let packed = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let table = zstd::decode_all(&packed[..]).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&table)
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f.len() >= 6 && f[1] == "regular").then(|| {
                let ranges = f[5]
                    .split(',')
                    .filter_map(|r| {
                        let (a, b) = r.split_once('-').unwrap_or((r, r));
                        Some((
                            u32::from_str_radix(a, 16).ok()?,
                            u32::from_str_radix(b, 16).ok()?,
                        ))
                    })
                    .collect();
                (f[0].to_string(), ranges)
            })
        })
        .collect())
}

/// Writes the report to `out`, or prints it.
pub fn write(root: &Path, out: Option<&Path>) -> Result<(), String> {
    let categories = ucd_file("extracted/DerivedGeneralCategory.txt")?;
    let version = version_of(&categories)?;
    let blocks = ucd_file("Blocks.txt")?;
    // 0 unassigned (or private use, surrogate, control), 1 Noto, 2 a font
    // for rare characters, 3 assigned and in no font.
    let mut state = vec![3u8; 0x11_0000];
    for line in categories.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some((points, category)) = line.split_once(';') else {
            continue;
        };
        if matches!(category.trim(), "Cn" | "Co" | "Cs" | "Cc") {
            let points = points.trim();
            let (a, b) = points.split_once("..").unwrap_or((points, points));
            let (a, b) = (
                u32::from_str_radix(a, 16).map_err(|e| e.to_string())?,
                u32::from_str_radix(b, 16).map_err(|e| e.to_string())?,
            );
            state[a as usize..=b as usize].fill(0);
        }
    }
    let mut mark = |name: &str, value: u8| -> Result<(), String> {
        for (_, ranges) in catalogue(root, name)? {
            for (a, b) in ranges {
                for s in &mut state[a as usize..=b as usize] {
                    if *s == 3 {
                        *s = value;
                    }
                }
            }
        }
        Ok(())
    };
    mark("fonts.tsv.zst", 1)?;
    mark("fonts-rare.tsv.zst", 2)?;

    let count = |v: u8| state.iter().filter(|&&s| s == v).count();
    let (noto, rare, none) = (count(1), count(2), count(3));
    let assigned = noto + rare + none;
    let table = std::fs::read_to_string(root.join("crates/xwa-convert/data/no-character.txt"))
        .unwrap_or_default();
    let ours = table
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("# Unicode "))
        .and_then(|l| l.split(':').next())
        .unwrap_or("unknown");

    let mut md = String::new();
    let _ = writeln!(md, "## Character coverage, Unicode {version}\n");
    if ours != version {
        let _ = writeln!(
            md,
            "**Unicode {version} is out** (the converter's table was {ours}).\n"
        );
    }
    let _ = writeln!(
        md,
        "Of {assigned} characters assigned, the Noto fonts draw {noto}, the fonts for rare \
         characters {rare} more, and **{none} have no glyph**.\n"
    );
    // Blocks with characters no font draws.
    let mut missing: BTreeMap<u32, (String, u32, usize)> = BTreeMap::new();
    for line in blocks.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some((points, name)) = line.split_once(';') else {
            continue;
        };
        let Some((a, b)) = points.trim().split_once("..") else {
            continue;
        };
        let (Ok(a), Ok(b)) = (u32::from_str_radix(a, 16), u32::from_str_radix(b, 16)) else {
            continue;
        };
        let n = state[a as usize..=b as usize]
            .iter()
            .filter(|&&s| s == 3)
            .count();
        if n > 0 {
            missing.insert(a, (name.trim().to_string(), b, n));
        }
    }
    if !missing.is_empty() {
        let _ = writeln!(md, "| Block | Range | Without a glyph |\n|---|---|---|");
        for (a, (name, b, n)) in &missing {
            let _ = writeln!(md, "| {name} | U+{a:04X}–U+{b:04X} | {n} |");
        }
    }
    match out {
        Some(path) => std::fs::write(path, &md).map_err(|e| format!("{}: {e}", path.display())),
        None => {
            print!("{md}");
            Ok(())
        }
    }
}
