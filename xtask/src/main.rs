//! Repository chores, run as `cargo xtask <task>`.
//!
//! `cargo xtask licenses [--target TRIPLE]... [--out FILE]` writes the
//! third-party notice that ships with releases: every crate compiled into
//! the xwa-tool and xwa-tool-gui programs for the given targets (all release
//! targets by default), then their licence texts. It puts no limits on which
//! crates may be used: a crate whatever its licence is listed with the texts
//! it ships, one that ships none gets the standard text of its licence when
//! `xtask/licenses/` has one, and anything missing is a warning, never an
//! error.
//!
//! `cargo xtask days` writes `crates/xwa-convert/data/days.tsv.zst`, the
//! title of each Wikipedia's article for each day of the year, from
//! Wikidata, which the converter uses for the DAYS section.

mod days;
mod spdx;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::Value;

/// The packages whose programs ship.
const SHIPPED: &[&str] = &["xwa-cli", "xwa-gui"];

/// The targets release builds are made for.
const RELEASE_TARGETS: &[&str] = &[
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

/// The order standard texts are picked in, for a crate that ships no text
/// and offers a choice of licences.
const PREFERENCE: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "Zlib",
    "BSL-1.0",
    "MPL-2.0",
    "CC0-1.0",
    "Unlicense",
    "0BSD",
];

/// Statements some licences require in a program's documentation.
const ACKNOWLEDGEMENTS: &[(&str, &str)] = &[(
    "IJG",
    "This software is based in part on the work of the Independent JPEG Group.",
)];

/// Licence files below a crate's top folder, for code from other projects
/// that the crate compiles in under that project's licence.
const NESTED: &[(&str, &[&str])] = &[
    ("libdeflate-sys", &["libdeflate/COPYING"]),
    ("zstd-sys", &["zstd/LICENSE"]),
    ("ring", &["third_party/fiat/LICENSE"]),
    ("tracing-core", &["src/spin/LICENSE"]),
    ("wayland-protocols", &["protocols/COPYING"]),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("licenses") => licenses(&args[1..]),
        Some("days") if args.len() == 1 => days::write(&workspace_root()),
        _ => Err(
            "usage: cargo xtask licenses [--target TRIPLE]... [--out FILE]\n       cargo xtask days"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn licenses(args: &[String]) -> Result<(), String> {
    let mut targets = Vec::new();
    let mut out = workspace_root().join("target/THIRD-PARTY-LICENSES.txt");
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--target" => targets.push(value()?),
            "--out" => out = PathBuf::from(value()?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if targets.is_empty() {
        targets = RELEASE_TARGETS.iter().map(|t| t.to_string()).collect();
    }

    let mut crates = BTreeMap::new();
    for target in &targets {
        for c in shipped_crates(target)? {
            crates
                .entry((c.name.clone(), c.version.clone()))
                .or_insert(c);
        }
    }
    let notice = notice(&crates.into_values().collect::<Vec<_>>(), &targets);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, &notice.text).map_err(|e| format!("{}: {e}", out.display()))?;
    for warning in &notice.warnings {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "wrote {} ({} crates, {} licence texts)",
        out.display(),
        notice.crates,
        notice.texts
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct Crate {
    name: String,
    version: String,
    /// The licence expression from its Cargo.toml.
    license: String,
    url: String,
    /// Who to name in a copyright line for a standard licence text.
    holders: String,
    dir: PathBuf,
    license_file: Option<String>,
}

/// The crates compiled into the shipped programs for `target`: everything
/// they reach through normal dependencies (not build or dev ones).
fn shipped_crates(target: &str) -> Result<Vec<Crate>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked"])
        .arg("--manifest-path")
        .arg(workspace_root().join("Cargo.toml"))
        .args(["--filter-platform", target])
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed for {target}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let meta: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let str_of = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();

    let packages: BTreeMap<String, Value> = list(&meta["packages"])
        .into_iter()
        .map(|p| (str_of(&p["id"]), p))
        .collect();
    let deps: BTreeMap<String, Vec<String>> = list(&meta["resolve"]["nodes"])
        .into_iter()
        .map(|node| {
            let normal = list(&node["deps"])
                .into_iter()
                .filter(|d| list(&d["dep_kinds"]).iter().any(|k| k["kind"].is_null()))
                .map(|d| str_of(&d["pkg"]))
                .collect();
            (str_of(&node["id"]), normal)
        })
        .collect();
    let members: BTreeSet<String> = list(&meta["workspace_members"])
        .iter()
        .map(str_of)
        .collect();

    let mut todo: Vec<String> = packages
        .values()
        .filter(|p| {
            members.contains(&str_of(&p["id"]))
                && SHIPPED.contains(&p["name"].as_str().unwrap_or_default())
        })
        .map(|p| str_of(&p["id"]))
        .collect();
    if todo.len() != SHIPPED.len() {
        return Err(format!(
            "expected the packages {SHIPPED:?} in the workspace"
        ));
    }
    let mut seen = BTreeSet::new();
    while let Some(id) = todo.pop() {
        if seen.insert(id.clone()) {
            todo.extend(deps.get(&id).into_iter().flatten().cloned());
        }
    }

    let mut crates = Vec::new();
    for id in seen.iter().filter(|id| !members.contains(*id)) {
        let p = &packages[id];
        let name = str_of(&p["name"]);
        let version = str_of(&p["version"]);
        let license = p["license"].as_str().map(str::to_string);
        let license_file = p["license_file"].as_str().map(str::to_string);
        if license.is_none() && license_file.is_none() {
            return Err(format!("{name} {version} states no licence"));
        }
        let manifest = PathBuf::from(str_of(&p["manifest_path"]));
        let holders: Vec<String> = list(&p["authors"])
            .iter()
            .map(|a| {
                let a = str_of(a);
                a.split('<').next().unwrap_or_default().trim().to_string()
            })
            .filter(|a| !a.is_empty())
            .collect();
        crates.push(Crate {
            url: p["repository"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://crates.io/crates/{name}/{version}")),
            holders: if holders.is_empty() {
                format!("the {name} developers")
            } else {
                holders.join(", ")
            },
            license: license.unwrap_or_else(|| "LicenseRef-file".into()),
            dir: manifest.parent().unwrap_or(Path::new(".")).to_path_buf(),
            license_file,
            name,
            version,
        });
    }
    Ok(crates)
}

struct Notice {
    text: String,
    crates: usize,
    texts: usize,
    warnings: Vec<String>,
}

fn rank(id: &str) -> usize {
    PREFERENCE
        .iter()
        .position(|a| *a == id)
        .unwrap_or(usize::MAX)
}

fn notice(crates: &[Crate], targets: &[String]) -> Notice {
    let mut warnings = Vec::new();
    // Licence texts, each with the crates that ship it, in order of first use.
    let mut texts: Vec<(String, Vec<String>)> = Vec::new();
    let mut add_text = |body: String, user: String| {
        let body = normalize(&body);
        if body.is_empty() {
            return;
        }
        match texts.iter_mut().find(|(b, _)| *b == body) {
            Some((_, users)) => users.push(user),
            None => texts.push((body, vec![user])),
        }
    };
    let mut acknowledged = BTreeSet::new();

    for c in crates {
        let label = format!("{} {}", c.name, c.version);
        let (files, trouble) = licence_files(c);
        warnings.extend(trouble);
        let expr = match spdx::parse(&c.license) {
            Ok(e) => Some(e),
            Err(e) => {
                warnings.push(format!("{label}: {e}"));
                None
            }
        };
        for id in expr.iter().flat_map(spdx::ids) {
            if let Some((_, statement)) = ACKNOWLEDGEMENTS.iter().find(|(l, _)| *l == id) {
                acknowledged.insert(*statement);
            }
        }
        let all_text = squash(
            &files
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        // The crate's own texts, plus standard texts for licences they leave
        // out, where there are some.
        let missing: Vec<String> = match &expr {
            Some(expr) => {
                let shown = |id: &str| shows(&all_text, id);
                let usable = |id: &str| shows(&all_text, id) || standard(id).is_some();
                if spdx::choose(expr, &shown, &rank).is_some() {
                    Vec::new()
                } else {
                    spdx::choose(expr, &usable, &rank)
                        .map(|ids| ids.into_iter().filter(|id| !shown(id)).collect())
                        .unwrap_or_default()
                }
            }
            None => Vec::new(),
        };
        let ships_texts = !files.is_empty();
        for (_, body) in files {
            add_text(body, label.clone());
        }
        for id in &missing {
            let text = standard(id).expect("chosen from the standard texts");
            let body = if ["MIT", "Zlib"].contains(&id.as_str()) {
                format!("Copyright (c) {}\n\n{text}", c.holders)
            } else {
                text.to_string()
            };
            if ships_texts {
                warnings.push(format!(
                    "{label}: no {id} text among its licence files, so the standard one is used"
                ));
            }
            add_text(body, label.clone());
        }
        if !ships_texts && missing.is_empty() {
            warnings.push(format!("{label}: no licence text found for {}", c.license));
            add_text(
                format!(
                    "{label} is under {}; its source, which states its terms, is at {}.",
                    c.license, c.url
                ),
                label.clone(),
            );
        }
    }

    let mut out = String::new();
    out.push_str(HEADER);
    for statement in &acknowledged {
        let _ = writeln!(out, "{statement}\n");
    }
    let _ = writeln!(out, "Made for: {}.\n", targets.join(", "));
    let heading = format!("Crates ({})", crates.len());
    let _ = writeln!(out, "{heading}\n{}\n", "-".repeat(heading.len()));
    let width = crates
        .iter()
        .map(|c| c.name.len() + 1 + c.version.len())
        .max()
        .unwrap_or(0)
        .min(40);
    let lic_width = crates
        .iter()
        .map(|c| c.license.len())
        .max()
        .unwrap_or(0)
        .min(40);
    for c in crates {
        let label = format!("{} {}", c.name, c.version);
        let _ = writeln!(out, "{label:<width$}  {:<lic_width$}  {}", c.license, c.url);
    }
    out.push_str("\nLicence texts\n-------------\n");
    for (body, users) in &texts {
        let _ = writeln!(out, "\n{}", "=".repeat(78));
        out.push_str(&used_by(users, 78));
        let _ = writeln!(out, "{}\n", "=".repeat(78));
        out.push_str(body);
        out.push('\n');
    }
    Notice {
        text: out,
        crates: crates.len(),
        texts: texts.len(),
        warnings,
    }
}

const HEADER: &str = "\
Third-party software in xwa-tool
================================

xwa-tool itself is released into the public domain (see LICENSE). Its two
programs, xwa-tool and xwa-tool-gui, are built from the open-source Rust
crates listed below, which keep their own licences; the licence texts follow
the list. Each crate's source code is available from crates.io, at
https://crates.io/crates/NAME/VERSION, and from the repository listed with it.

Both programs contain KaTeX's fonts, to draw formulas; they are under the SIL
Open Font License 1.1 (licenses/KaTeX-fonts-OFL.txt).

";

/// Whether a file name looks like a licence or notice file.
fn is_licence_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = [
        "license",
        "licence",
        "copying",
        "copyright",
        "notice",
        "unlicense",
    ]
    .iter()
    .any(|p| lower.starts_with(p));
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    stem && ![
        "rs", "toml", "py", "sh", "json", "yml", "yaml", "c", "h", "js", "html",
    ]
    .contains(&ext)
}

/// The crate's licence files, as (path in the crate, text): those in its top
/// folder or a `LICENSES` folder there, its `license-file`, and any listed
/// in [`NESTED`]. Files that cannot be read are warnings.
fn licence_files(c: &Crate) -> (Vec<(String, String)>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut paths = BTreeSet::new();
    let entries = match std::fs::read_dir(&c.dir) {
        Ok(entries) => entries.flatten().collect(),
        Err(e) => {
            warnings.push(format!("{}: {e}", c.dir.display()));
            Vec::new()
        }
    };
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_licence_name(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            for inner in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                if inner.path().is_file() {
                    paths.insert(format!("{name}/{}", inner.file_name().to_string_lossy()));
                }
            }
        } else if path.is_file() {
            paths.insert(name);
        }
    }
    if let Some(file) = &c.license_file {
        paths.insert(file.clone());
    }
    for (_, nested) in NESTED.iter().filter(|(n, _)| *n == c.name) {
        for path in *nested {
            if c.dir.join(path).is_file() {
                paths.insert(path.to_string());
            } else {
                warnings.push(format!(
                    "{} {}: {path} is gone; NESTED in xtask/src/main.rs lists it",
                    c.name, c.version
                ));
            }
        }
    }
    let mut files = Vec::new();
    for p in paths {
        match std::fs::read(c.dir.join(&p)) {
            Ok(bytes) => files.push((p, decode(&bytes))),
            Err(e) => warnings.push(format!("{} {}: {p}: {e}", c.name, c.version)),
        }
    }
    (files, warnings)
}

/// UTF-8, or Latin-1 for the odd older file.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Line endings and trailing spaces evened out, so copies of one text match.
fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    lines.join("\n").trim_matches('\n').to_string()
}

/// Lowercase with all runs of white space made one space, for [`shows`].
fn squash(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `text` (squashed) contains the licence `id`.
fn shows(text: &str, id: &str) -> bool {
    let phrases: &[&str] = match id {
        "MIT" => &["permission is hereby granted, free of charge"],
        "Apache-2.0" => &["apache license", "version 2.0"],
        "Apache-2.0 WITH LLVM-exception" => &["apache license", "llvm exceptions"],
        "Zlib" => &["altered source versions must be plainly marked"],
        "BSD-3-Clause" => &[
            "redistribution and use in source and binary forms",
            "neither the name",
        ],
        "BSD-2-Clause" => &["redistribution and use in source and binary forms"],
        "ISC" | "0BSD" => &["permission to use, copy, modify, and"],
        "Unlicense" => &["free and unencumbered software released into the public domain"],
        "CC0-1.0" => &["cc0"],
        "BSL-1.0" => &["boost software license"],
        "Unicode-3.0" => &["unicode"],
        "MPL-2.0" => &["mozilla public license"],
        "IJG" => &["independent jpeg group"],
        "OFL-1.1" => &["open font license"],
        "CDLA-Permissive-2.0" => &["community data license agreement"],
        _ => return false,
    };
    phrases.iter().all(|p| text.contains(p))
}

/// The standard text for a licence, for crates that ship none.
fn standard(id: &str) -> Option<&'static str> {
    Some(match id {
        "MIT" => include_str!("../licenses/MIT.txt"),
        "Apache-2.0" => include_str!("../licenses/Apache-2.0.txt"),
        "Zlib" => include_str!("../licenses/Zlib.txt"),
        "BSL-1.0" => include_str!("../licenses/BSL-1.0.txt"),
        "MPL-2.0" => include_str!("../licenses/MPL-2.0.txt"),
        // These ask for no notice at all.
        "CC0-1.0" | "Unlicense" => "Dedicated to the public domain (no conditions apply).",
        "0BSD" => "Under the BSD Zero Clause License (no conditions apply).",
        _ => return None,
    })
}

/// "Used by" and the names, wrapped to `width` columns without splitting a
/// name from its version.
fn used_by(users: &[String], width: usize) -> String {
    let mut out = String::new();
    let mut line = String::from("Used by");
    for (i, user) in users.iter().enumerate() {
        let item = if i + 1 < users.len() {
            format!("{user},")
        } else {
            user.clone()
        };
        if line.len() + 1 + item.len() > width {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        } else {
            line.push(' ');
        }
        line.push_str(&item);
    }
    out.push_str(&line);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_licence_files() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "license.txt",
            "LICENCE.md",
            "COPYING",
            "NOTICE",
            "UNLICENSE",
            "LICENSES",
        ] {
            assert!(is_licence_name(name), "{name}");
        }
        for name in ["license.rs", "README.md", "Cargo.toml", "copyright.json"] {
            assert!(!is_licence_name(name), "{name}");
        }
    }

    #[test]
    fn recognises_licence_texts() {
        let mit = squash(include_str!("../licenses/MIT.txt"));
        assert!(shows(&mit, "MIT"));
        assert!(!shows(&mit, "Apache-2.0"));
        assert!(shows(
            &squash(include_str!("../licenses/Apache-2.0.txt")),
            "Apache-2.0"
        ));
        assert!(shows(&squash(include_str!("../licenses/Zlib.txt")), "Zlib"));
        assert!(shows(
            &squash(include_str!("../licenses/BSL-1.0.txt")),
            "BSL-1.0"
        ));
        assert!(shows(
            &squash(include_str!("../licenses/MPL-2.0.txt")),
            "MPL-2.0"
        ));
    }

    #[test]
    fn copies_of_a_text_match() {
        assert_eq!(normalize("\u{feff}A  \r\nB\r\n\r\n"), normalize("A\nB"));
        assert_eq!(decode(b"J\xe9r\xf4me"), "Jérôme");
        let users = ["a 1".to_string(), "bb 2".into(), "c 3".into()];
        assert_eq!(used_by(&users, 16), "Used by a 1,\nbb 2, c 3\n");
    }

    /// A crate in a temporary folder holding the given files.
    fn fake(dir: &Path, name: &str, license: &str, files: &[(&str, &str)]) -> Crate {
        let dir = dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        Crate {
            name: name.into(),
            version: "1.0.0".into(),
            license: license.into(),
            url: format!("https://example.org/{name}"),
            holders: "Ada Lovelace".into(),
            dir,
            license_file: None,
        }
    }

    #[test]
    fn writes_texts_for_any_licence_and_never_refuses() {
        let tmp = std::env::temp_dir().join(format!("xtask-notice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let shipped = fake(
            &tmp,
            "shipped",
            "MIT OR Apache-2.0",
            &[(
                "LICENSE-MIT",
                "Copyright (c) Bob\n\nPermission is hereby granted, free of charge, ...",
            )],
        );
        let bare = fake(&tmp, "bare", "Zlib OR Apache-2.0 OR MIT", &[]);
        let twin = fake(
            &tmp,
            "twin",
            "MIT",
            &[(
                "LICENSE",
                "Copyright (c) Bob\r\n\r\nPermission is hereby granted, free of charge, ...  \r\n",
            )],
        );
        let targets = ["x86_64-unknown-linux-gnu".to_string()];
        let n = notice(&[bare.clone(), shipped.clone(), twin], &targets);
        assert!(n.warnings.is_empty(), "{:?}", n.warnings);
        assert_eq!(n.crates, 3);
        assert_eq!(n.texts, 2, "the two copies of Bob's text are one");
        assert!(n.text.contains("Used by shipped 1.0.0, twin 1.0.0\n"));
        // The bare crate gets the standard MIT text with its authors.
        assert!(n.text.contains("Used by bare 1.0.0\n"));
        assert!(
            n.text
                .contains("Copyright (c) Ada Lovelace\n\nPermission is hereby granted")
        );
        assert!(!n.text.contains("Independent JPEG Group"));

        let ijg = fake(
            &tmp,
            "jpeg",
            "MIT AND IJG",
            &[
                (
                    "LICENSE-MIT",
                    "Permission is hereby granted, free of charge",
                ),
                ("LICENSE-IJG", "the Independent JPEG Group's software"),
            ],
        );
        assert!(
            notice(&[ijg], &targets)
                .text
                .contains("based in part on the work of the Independent JPEG Group")
        );

        // Any licence goes in, with its own text and no complaint.
        let gpl = fake(
            &tmp,
            "gpl",
            "GPL-3.0-only",
            &[("COPYING", "GNU GENERAL PUBLIC LICENSE")],
        );
        let n = notice(&[shipped.clone(), gpl], &targets);
        assert!(n.warnings.is_empty(), "{:?}", n.warnings);
        assert!(n.text.contains("Used by gpl 1.0.0\n") && n.text.contains("GNU GENERAL"));
        // With no text to be had, the notice says where the terms are.
        let unicode = fake(&tmp, "uni", "Unicode-3.0", &[]);
        let odd = fake(&tmp, "odd", "MIT OR", &[("LICENSE", "Some terms")]);
        let n = notice(&[unicode, odd], &targets);
        assert_eq!(n.warnings.len(), 2, "{:?}", n.warnings);
        assert!(n.text.contains(
            "uni 1.0.0 is under Unicode-3.0; its source, which states its terms, is at https://example.org/uni."
        ));
        assert!(n.text.contains("Some terms"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
