//! The code points no font draws: those Unicode has not assigned, private
//! use, surrogates and controls (general categories Cn, Co, Cs and Cc),
//! from the Unicode Character Database, as `verify` uses them to say why
//! the fallback font lacks a character.
//!
//! One range per line, in hexadecimal (`0378-0379`, or `E000-F8FF`), after
//! a line naming the Unicode version. It follows the latest version of
//! Unicode; the weekly check (`.github/workflows/watch.yml`) says when a new
//! one is out.

use std::path::Path;

/// The latest Unicode Character Database.
pub(crate) const UCD: &str = "https://www.unicode.org/Public/UCD/latest/ucd";

/// A file of the latest Unicode Character Database.
pub(crate) fn ucd_file(name: &str) -> Result<String, String> {
    let url = format!("{UCD}/{name}");
    crate::fonts::agent()
        .get(&url)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))
}

/// The Unicode version of a UCD file, from its first line
/// (`# DerivedGeneralCategory-18.0.0.txt`).
pub(crate) fn version_of(text: &str) -> Result<String, String> {
    text.lines()
        .next()
        .and_then(|l| l.rsplit('-').next())
        .and_then(|v| v.strip_suffix(".txt"))
        .map(str::to_string)
        .ok_or_else(|| "a UCD file without its version".to_string())
}

pub fn write(root: &Path) -> Result<(), String> {
    let text = ucd_file("extracted/DerivedGeneralCategory.txt")?;
    let version = version_of(&text)?;
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some((points, category)) = line.split_once(';') else {
            continue;
        };
        if !matches!(category.trim(), "Cn" | "Co" | "Cs" | "Cc") {
            continue;
        }
        let points = points.trim();
        let (a, b) = points.split_once("..").unwrap_or((points, points));
        let parse = |s: &str| u32::from_str_radix(s, 16).map_err(|e| format!("{s}: {e}"));
        ranges.push((parse(a)?, parse(b)?));
    }
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (a, b) in ranges {
        match merged.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut out = format!("# Unicode {version}: general categories Cn, Co, Cs and Cc\n");
    for (a, b) in &merged {
        if a == b {
            out.push_str(&format!("{a:04X}\n"));
        } else {
            out.push_str(&format!("{a:04X}-{b:04X}\n"));
        }
    }
    let path = root.join("crates/xwa-convert/data/no-character.txt");
    std::fs::write(&path, &out).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!(
        "wrote {} ranges to {} ({} bytes)",
        merged.len(),
        path.display(),
        out.len()
    );
    Ok(())
}
