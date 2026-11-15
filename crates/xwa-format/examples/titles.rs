//! Time and memory for an archive with many titles and empty articles, as a
//! stand-in for English Wikipedia's 7 million articles and 11.5 million
//! redirects:
//!
//! ```text
//! cargo run --release -p xwa-format --example titles -- 7000000 11500000 /tmp/titles.xwa
//! ```

use std::time::Instant;

use xwa_format::Profile;
use xwa_format::content::Article;
use xwa_format::writer::{ArchiveWriter, WriterOptions};

/// A Wikipedia-like title of one to four words, some accented, made unique
/// by its number.
fn title(i: u64) -> String {
    const WORDS: [&str; 16] = [
        "List",
        "of",
        "Battle",
        "River",
        "Église",
        "Station",
        "(album)",
        "Ångström",
        "County",
        "Saint",
        "José",
        "Football",
        "Club",
        "North",
        "Müller",
        "Island",
    ];
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut t = String::new();
    for _ in 0..1 + x % 4 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        t.push_str(WORDS[(x % 16) as usize]);
        t.push(' ');
    }
    t.push_str(&i.to_string());
    t
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: titles ARTICLES REDIRECTS [OUT]";
    let articles: u32 = args.get(1).and_then(|a| a.parse().ok()).expect(usage);
    let redirects: u64 = args.get(2).and_then(|a| a.parse().ok()).expect(usage);
    let out = args.get(3).map_or("titles.xwa", String::as_str);
    let started = Instant::now();
    // Fast compression: this measures the tables, not the text.
    let opts = WriterOptions {
        deflate_level: 1,
        ..WriterOptions::new(Profile::Compact)
    };
    let mut w = ArchiveWriter::create(out, opts).unwrap();
    for id in 0..articles {
        let article = Article {
            title: title(id as u64),
            ..Default::default()
        };
        w.add_article(id, &article, 0).unwrap();
    }
    for r in 0..redirects {
        let target = (r % articles as u64) as u32;
        w.add_redirect(&title(articles as u64 + r), target).unwrap();
    }
    let (_, s) = w.finish().unwrap();
    eprintln!(
        "{} articles, {} redirects, title index {} MB, file {} MB, {:.1} s",
        s.articles,
        s.redirects,
        s.title_index_bytes >> 20,
        s.file_size >> 20,
        started.elapsed().as_secs_f64()
    );
}
