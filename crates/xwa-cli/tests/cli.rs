//! Runs the `xwa-tool` binary against a small archive built with the library.

use std::path::{Path, PathBuf};
use std::process::Command;

use xwa_format::Profile;
use xwa_format::content::{Article, Block, Inline, Place, Style};
use xwa_format::la2;
use xwa_format::records::{image_encoding, image_kind};
use xwa_format::writer::{ArchiveWriter, ImageInfo, WriterOptions};

fn tool(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_xwa-tool"))
        .args(args)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

fn line_art() -> Vec<u8> {
    // 4x2 pixels, 2 bpp, stored without compression inside a deflate stream.
    let packed = [0b0001_1011, 0b1110_0100];
    let mut c = stored_deflate(&packed);
    let mut out = vec![4, 0, 2, 0, 2, la2::codec::DEFLATE];
    out.append(&mut c);
    out
}

/// A raw deflate "stored" block: valid input for any inflater, no encoder needed.
fn stored_deflate(data: &[u8]) -> Vec<u8> {
    let len = data.len() as u16;
    let mut out = vec![0x01];
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn build(path: &Path) {
    let opts = WriterOptions {
        threads: 2,
        ..WriterOptions::new(Profile::Compact)
    };
    let mut w = ArchiveWriter::create(path, opts).unwrap();
    w.set_meta("title", "CLI test");
    let text = |s: &str| Inline::Text(s.to_string());
    let pythagoras = Article {
        title: "Pythagorean theorem".into(),
        body: vec![
            Block::Para(vec![
                text("A relation in "),
                Inline::Link {
                    article: 1,
                    place: Place::TOP,
                    content: vec![Inline::Styled(Style::Italic, vec![text("geometry")])],
                },
                text("."),
            ]),
            Block::Heading {
                level: 2,
                content: vec![text("Proofs")],
            },
            Block::Figure {
                image: 0,
                caption: vec![text("A diagram")],
            },
        ],
        ..Default::default()
    };
    let geometry = Article {
        title: "Geometry".into(),
        body: vec![Block::Para(vec![text("Shapes.")])],
        ..Default::default()
    };
    w.add_article(0, &pythagoras, 0).unwrap();
    w.add_article(1, &geometry, 0).unwrap();
    w.add_redirect("Pythagoras' theorem", 0, None).unwrap();
    let info = ImageInfo {
        encoding: image_encoding::LINE_ART,
        width: 4,
        height: 2,
        kind: image_kind::LINE_ART,
        flags: 0,
    };
    w.add_image(0, info, line_art()).unwrap();
    w.finish().unwrap();
}

fn temp(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
fn info_verify_and_dump() {
    let path = temp("cli-test.xwa");
    build(&path);
    let file = path.to_str().unwrap();

    let (ok, out) = tool(&["info", file]);
    assert!(ok, "{out}");
    assert!(
        out.contains("XWA 0.4") && out.contains("2 articles") && out.contains("title: CLI test"),
        "{out}"
    );

    let (ok, out) = tool(&["verify", file]);
    assert!(ok, "{out}");
    assert!(out.contains("OK: no problems found"), "{out}");

    let (ok, out) = tool(&["dump", file, "--title", "PYTHAGOREAN theorem"]);
    assert!(ok, "{out}");
    assert!(out.contains("# Pythagorean theorem"), "{out}");
    assert!(out.contains("[*geometry*](#1 Geometry)"), "{out}");
    assert!(
        out.contains("## Proofs") && out.contains("[figure: image 0] A diagram"),
        "{out}"
    );

    let (ok, out) = tool(&["dump", file, "--search", "pyth"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("Pythagoras' theorem -> Pythagorean theorem"),
        "{out}"
    );

    // Each article once, by its first title in index order.
    let (ok, out) = tool(&["dump", file, "--search", "PYTH", "--popular"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("Pythagoras' theorem -> Pythagorean theorem")
            && !out.contains("  Pythagorean theorem"),
        "{out}"
    );

    let pgm = temp("cli-test.pgm");
    let (ok, out) = tool(&["dump", file, "--image", "0", "--out", pgm.to_str().unwrap()]);
    assert!(ok, "{out}");
    assert_eq!(
        std::fs::read(&pgm).unwrap(),
        b"P5\n4 2\n255\n\xff\xaa\x55\x00\x00\x55\xaa\xff"
    );

    let (ok, out) = tool(&["dump", file, "--title", "No such article"]);
    assert!(!ok && out.contains("no article titled"), "{out}");
}

#[test]
fn verify_reports_damage() {
    let path = temp("cli-damaged.xwa");
    build(&path);
    let chunk = xwa_format::reader::Archive::open(&path)
        .unwrap()
        .chunk_entry(0)
        .unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4096 + chunk.offset as usize + 3] ^= 0x40;
    std::fs::write(&path, bytes).unwrap();
    let (ok, out) = tool(&["verify", path.to_str().unwrap()]);
    assert!(!ok, "{out}");
    assert!(out.contains("problem: chunk 0 checksum mismatch"), "{out}");
    assert!(out.contains("problems found"), "{out}");
}

#[test]
fn rejects_other_files() {
    let path = temp("not-an-archive.txt");
    std::fs::write(&path, "hello").unwrap();
    let (ok, out) = tool(&["info", path.to_str().unwrap()]);
    assert!(
        !ok && out.contains("neither a ZIM nor an XWA file"),
        "{out}"
    );
    let (ok, out) = tool(&["verify", path.to_str().unwrap()]);
    assert!(!ok && out.contains("not an XWA archive"), "{out}");
}
