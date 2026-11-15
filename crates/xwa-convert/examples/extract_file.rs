//! Extracts one saved HTML file and prints it as text: a debugging aid.
//! Every link "resolves", to an id that names its target path.

use std::sync::Mutex;

struct AllPaths(Mutex<Vec<String>>);

impl xwa_convert::html::Resolver for AllPaths {
    fn article(&self, path: &str) -> Option<u32> {
        let mut v = self.0.lock().unwrap();
        v.push(path.to_string());
        Some(v.len() as u32 - 1)
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: extract_file <file.html>");
    let html = std::fs::read_to_string(&path).unwrap();
    let paths = AllPaths(Mutex::new(vec!["(self)".into()]));
    let t = std::time::Instant::now();
    let x = xwa_convert::html::extract(&html, "Article", "Article", 0, &paths);
    let took = t.elapsed();
    let names = paths.0.lock().unwrap().clone();
    let text = xwa_format::text::article_text(&x.article, &|id| names.get(id as usize).cloned());
    println!("{text}");
    let stream = xwa_format::content::encode_article(&x.article);
    eprintln!(
        "{:?}\nhtml {} bytes -> stream {} bytes, {} tables, infobox {}, in {:.1} ms",
        x.stats,
        html.len(),
        stream.len(),
        x.article.tables.len(),
        x.article.infobox.is_some(),
        took.as_secs_f64() * 1000.0
    );
}
