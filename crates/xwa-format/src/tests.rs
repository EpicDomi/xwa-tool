//! Whole-archive tests: write, read back, verify, damage.

use std::io::Cursor;

use crate::content::{
    Article, Block, Cell, Formula, Infobox, InfoboxRow, Inline, ListKind, MathItem, Style, Table,
};
use crate::reader::Archive;
use crate::records::{image_encoding, image_kind};
use crate::title_index::FLAG_REDIRECT;
use crate::writer::{ArchiveWriter, ImageInfo, Summary, WriterOptions};
use crate::{Error, Profile};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

const WORDS: [&str; 16] = [
    "the", "theorem", "of", "prime", "numbers", "states", "that", "every", "integer", "greater",
    "than", "one", "Gödel", "proved", "in", "1931",
];

fn words(rng: &mut Rng, n: u32) -> String {
    (0..n)
        .map(|_| WORDS[rng.below(16) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

const GLYPHS: u32 = 16;

fn formula(rng: &mut Rng, display: bool) -> Formula {
    let mut items = Vec::new();
    for _ in 0..1 + rng.below(6) {
        items.push(match rng.below(3) {
            0 => MathItem::Glyph {
                glyph: rng.below(GLYPHS),
                scale: rng.below(3) as u8,
                dx: 500,
                dy: -(rng.below(300) as i32),
            },
            1 => MathItem::GlyphXY {
                glyph: rng.below(GLYPHS),
                sx: 256,
                sy: 300 + rng.below(500) as i16,
                dx: 20,
                dy: 0,
            },
            _ => MathItem::Rule {
                dx: -900,
                dy: 250,
                w: 900,
                h: 40,
            },
        });
    }
    Formula {
        display,
        width: 2000,
        ascent: 750,
        descent: 250,
        items,
        fallback_image: None,
    }
}

const ARTICLES: u32 = 240;
const IMAGES: u32 = 12;

fn article(id: u32) -> Article {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (id as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D));
    let title = format!("Article {id} {}", WORDS[(id % 16) as usize]);
    // Every 9th article is large enough to span several 32 KB chunks; every
    // 13th is empty.
    let paragraphs = match id {
        _ if id % 13 == 5 => 0,
        _ if id.is_multiple_of(9) => 600,
        _ => 1 + id % 7,
    };
    let mut body = Vec::new();
    let mut tables = Vec::new();
    for p in 0..paragraphs {
        if p % 6 == 0 {
            body.push(Block::Heading {
                level: 2 + (p % 3) as u8,
                content: vec![Inline::Text(format!("Section {p}"))],
            });
        }
        let mut run = vec![
            Inline::Text(words(&mut rng, 30)),
            Inline::Link {
                article: rng.below(ARTICLES),
                section: 0,
                content: vec![Inline::Styled(
                    Style::Italic,
                    vec![Inline::Text(words(&mut rng, 2))],
                )],
            },
            Inline::Text(format!(" {}.", words(&mut rng, 12))),
        ];
        if p % 3 == 1 {
            run.push(Inline::Math(formula(&mut rng, false)));
        }
        if p % 4 == 2 {
            run.push(Inline::Icon(rng.below(IMAGES)));
        }
        body.push(Block::Para(run));
        match p % 17 {
            3 => body.push(Block::MathBlock(formula(&mut rng, true))),
            5 => body.push(Block::Figure {
                image: rng.below(IMAGES),
                caption: vec![Inline::Text(words(&mut rng, 6))],
            }),
            7 => body.push(Block::List {
                kind: ListKind::Bullet,
                items: (0..3)
                    .map(|_| vec![Block::Para(vec![Inline::Text(words(&mut rng, 5))])])
                    .collect(),
            }),
            11 => {
                body.push(Block::TableRef(tables.len() as u32));
                tables.push(Table {
                    rows: 2,
                    cols: 2,
                    flags: 0,
                    cells: (0..4)
                        .map(|i| Cell {
                            header: i < 2,
                            rowspan: 1,
                            colspan: 1,
                            content: vec![Inline::Text(words(&mut rng, 2))],
                        })
                        .collect(),
                });
            }
            _ => {}
        }
    }
    let infobox = (id % 5 == 1).then(|| {
        body.insert(0, Block::InfoboxRef);
        Infobox {
            rows: vec![
                InfoboxRow::Header(vec![Inline::Text(title.clone())]),
                InfoboxRow::Image {
                    image: id % IMAGES,
                    caption: vec![],
                },
                InfoboxRow::Field {
                    label: vec![Inline::Text("Born".into())],
                    value: vec![Inline::Text("1906".into())],
                },
            ],
        }
    });
    Article {
        title,
        body,
        tables,
        infobox,
    }
}

fn la2(width: u16, height: u16, seed: u8) -> Vec<u8> {
    let stride = (width as usize * 2).div_ceil(8);
    let pixels: Vec<u8> = (0..stride * height as usize)
        .map(|i| (i as u8).wrapping_mul(seed) & 0xF0)
        .collect();
    let mut c = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(6).unwrap());
    let mut packed = vec![0; c.deflate_compress_bound(pixels.len())];
    let n = c.deflate_compress(&pixels, &mut packed).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&[2, 0]);
    out.extend_from_slice(&packed[..n]);
    out
}

fn image(id: u32) -> (ImageInfo, Vec<u8>) {
    if id.is_multiple_of(2) {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend((0..100 + id * 30).map(|i| (i * 7) as u8));
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        let info = ImageInfo {
            encoding: image_encoding::JPEG,
            width: 320,
            height: 200,
            kind: image_kind::PHOTO,
            flags: 0,
        };
        (info, jpeg)
    } else {
        let (w, h) = (37 + id as u16, 20);
        let info = ImageInfo {
            encoding: image_encoding::LINE_ART,
            width: w,
            height: h,
            kind: image_kind::LINE_ART,
            flags: 1,
        };
        (info, la2(w, h, id as u8))
    }
}

fn math_glyphs() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&1000u16.to_le_bytes());
    b.extend_from_slice(&(GLYPHS as u16).to_le_bytes());
    b
}

fn options(profile: Profile) -> WriterOptions {
    WriterOptions {
        // Debug builds compile the C codecs without optimisation.
        deflate_level: 6,
        zstd_level: 3,
        dict_sample_bytes: 256 << 10,
        dict_size: 16 << 10,
        threads: 3,
        ..WriterOptions::new(profile)
    }
}

fn finish(w: ArchiveWriter<Cursor<Vec<u8>>>) -> (Vec<u8>, Summary) {
    let (cursor, summary) = w.finish().unwrap();
    (cursor.into_inner(), summary)
}

fn check_archive(bytes: Vec<u8>, summary: &Summary, profile: Profile) {
    assert_eq!(bytes.len() as u64, summary.file_size);
    let a = Archive::from_source(bytes).unwrap();
    assert_eq!(a.superblock().profile, profile);
    assert_eq!(a.article_count(), ARTICLES);
    assert_eq!(a.image_count(), IMAGES);
    assert!(a.chunk_count() > 10, "large articles should span chunks");
    assert_eq!(a.meta_value("title"), Some("Test wiki"));
    for id in 0..ARTICLES {
        assert_eq!(a.article(id).unwrap(), article(id), "article {id}");
        assert_eq!(a.title(id).unwrap(), article(id).title);
    }
    for id in 0..IMAGES {
        let (rec, data) = a.image(id).unwrap();
        let (info, expected) = image(id);
        assert_eq!(
            (rec.encoding, rec.width, rec.height, rec.kind, rec.flags),
            (
                info.encoding,
                info.width,
                info.height,
                info.kind,
                info.flags
            )
        );
        assert_eq!(data, expected);
    }
    assert_eq!(a.article_record(7).unwrap().popularity, 200);

    let hits = a.search("ARTICLE 12", 50).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.article == 12 && h.flags & FLAG_REDIRECT == 0)
    );
    let hits = a.search("alias of 12", 5).unwrap();
    assert_eq!(
        (hits[0].display.as_str(), hits[0].article, hits[0].flags),
        ("Alias of 12", 12, FLAG_REDIRECT)
    );
    assert!(a.search("zz no such title", 5).unwrap().is_empty());

    let report = a.verify(&mut |_, _, _| {}).unwrap();
    assert!(report.ok(), "{:#?}", report.problems);
    assert_eq!(report.redirects, (0..ARTICLES).step_by(3).count() as u64);
    assert_eq!(report.formulas, summary.formulas);
    assert!(report.formulas > 0 && report.links > 0);
    assert_eq!(report.title_entries, ARTICLES as u64 + report.redirects);
    // Articles are read in order, so each chunk is decompressed once.
    assert_eq!(report.chunk_reads, a.chunk_count() as u64);
}

/// Writes the test archive, adding images between articles the way a
/// converter would.
fn archive(opts: WriterOptions) -> (Vec<u8>, Summary) {
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new()), opts).unwrap();
    w.set_meta("title", "Test wiki");
    w.set_math_glyphs(math_glyphs());
    let mut next_image = 0;
    let mut add_image = |w: &mut ArchiveWriter<_>| {
        let (info, data) = image(next_image);
        w.add_image(next_image, info, data).unwrap();
        next_image += 1;
    };
    for id in 0..ARTICLES {
        w.add_article(id, &article(id), 0).unwrap();
        if id % 20 == 0 && w.image_count() < IMAGES {
            add_image(&mut w);
        }
        if id % 3 == 0 {
            w.add_redirect(&format!("Alias of {id}"), id);
            // Same key as the article's own title: dropped by the index.
            w.add_redirect(&format!("article {id} {}", WORDS[(id % 16) as usize]), id);
        }
    }
    while w.image_count() < IMAGES {
        add_image(&mut w);
    }
    w.set_popularity(7, 200).unwrap();
    finish(w)
}

#[test]
fn compact_round_trip() {
    let (bytes, summary) = archive(options(Profile::Compact));
    check_archive(bytes, &summary, Profile::Compact);
}

#[test]
fn full_round_trip_with_trained_dictionary() {
    let (bytes, summary) = archive(options(Profile::Full));
    assert!(summary.dictionary_bytes > 0);
    let a = Archive::from_source(bytes.clone()).unwrap();
    assert!(a.has_dictionary());
    check_archive(bytes, &summary, Profile::Full);
}

#[test]
fn small_full_archive_has_no_dictionary() {
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new()), options(Profile::Full)).unwrap();
    w.add_article(
        0,
        &Article {
            title: "Only".into(),
            body: vec![Block::Para(vec![Inline::Text("Hi".into())])],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let (bytes, summary) = finish(w);
    assert_eq!(summary.dictionary_bytes, 0);
    let a = Archive::from_source(bytes).unwrap();
    assert!(!a.has_dictionary());
    assert!(a.verify(&mut |_, _, _| {}).unwrap().ok());
    assert_eq!(a.search("on", 5).unwrap()[0].article, 0);
}

#[test]
fn empty_archive_is_valid() {
    let w = ArchiveWriter::new(Cursor::new(Vec::new()), options(Profile::Compact)).unwrap();
    let (bytes, _) = finish(w);
    let a = Archive::from_source(bytes).unwrap();
    assert_eq!(
        (a.article_count(), a.image_count(), a.chunk_count()),
        (0, 0, 0)
    );
    assert!(a.verify(&mut |_, _, _| {}).unwrap().ok());
}

#[test]
fn output_is_deterministic() {
    let (a, _) = archive(options(Profile::Full));
    let (b, _) = archive(WriterOptions {
        threads: 1,
        ..options(Profile::Full)
    });
    assert!(a == b, "archives differ");
}

#[test]
fn damage_is_detected() {
    let (bytes, _) = archive(options(Profile::Compact));
    // A flipped bit in the first chunk.
    let mut damaged = bytes.clone();
    damaged[4096 + 10] ^= 0x10;
    let a = Archive::from_source(damaged).unwrap();
    let report = a.verify(&mut |_, _, _| {}).unwrap();
    assert!(!report.ok());
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains("BLOBS section checksum")),
        "{:?}",
        report.problems
    );
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains("chunk 0 checksum")),
        "{:?}",
        report.problems
    );
    assert!(matches!(a.article(0), Err(Error::Corrupt(_))));

    // An incomplete copy.
    let short = bytes[..bytes.len() - 1].to_vec();
    assert!(
        matches!(Archive::from_source(short), Err(Error::Corrupt(m)) if m.contains("incomplete"))
    );
    // Not an archive.
    assert!(matches!(
        Archive::from_source(vec![0u8; 8192]),
        Err(Error::BadMagic)
    ));
    // A damaged superblock.
    let mut sb = bytes.clone();
    sb[50] ^= 1;
    assert!(matches!(Archive::from_source(sb), Err(Error::Corrupt(_))));
}

#[test]
fn writer_rejects_bad_input() {
    let new = || ArchiveWriter::new(Cursor::new(Vec::new()), options(Profile::Compact)).unwrap();
    let mut w = new();
    assert!(w.add_article(1, &article(1), 0).is_err());
    let (info, data) = image(0);
    assert!(w.add_image(3, info, data).is_err());
    assert!(
        w.add_image(
            0,
            ImageInfo {
                encoding: 9,
                ..info
            },
            vec![1]
        )
        .is_err()
    );

    let mut w = new();
    w.add_article(0, &article(0), 0).unwrap();
    w.add_redirect("Elsewhere", 5);
    assert!(matches!(w.finish(), Err(Error::Invalid(m)) if m.contains("missing article")));

    let mut w = new();
    let with_math = Article {
        title: "M".into(),
        body: vec![Block::MathBlock(Formula::default())],
        ..Default::default()
    };
    w.add_article(0, &with_math, 0).unwrap();
    assert!(matches!(w.finish(), Err(Error::Invalid(m)) if m.contains("MATH_GLYPHS")));

    assert!(
        ArchiveWriter::new(
            Cursor::new(Vec::new()),
            WriterOptions {
                chunk_size: 40_000,
                ..options(Profile::Compact)
            }
        )
        .is_err()
    );
    assert!(
        ArchiveWriter::new(
            Cursor::new(Vec::new()),
            WriterOptions {
                deflate_level: 13,
                ..options(Profile::Compact)
            }
        )
        .is_err()
    );
}
