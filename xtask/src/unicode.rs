//! The code points no font draws: those Unicode has not assigned, private
//! use, surrogates and controls (general categories Cn, Co, Cs and Cc),
//! from the Unicode Character Database, as `verify` uses them to say why
//! the fallback font lacks a character.
//!
//! One range per line, in hexadecimal (`0378-0379`, or `E000-F8FF`), after
//! a line naming the Unicode version.

use std::path::Path;

/// The Unicode version the table follows.
const VERSION: &str = "18.0.0";

pub fn write(root: &Path) -> Result<(), String> {
    let url = format!(
        "https://www.unicode.org/Public/{VERSION}/ucd/extracted/DerivedGeneralCategory.txt"
    );
    let text = crate::fonts::agent()
        .get(&url)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
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
    let mut out = format!("# Unicode {VERSION}: general categories Cn, Co, Cs and Cc\n");
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
