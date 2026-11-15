//! The Kiwix library catalog (OPDS feeds).

use crate::Error;
use crate::xml::{self, Node};

/// One downloadable ZIM file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub id: String,
    /// The book's name without flavour or date, e.g. `wikipedia_en_all`.
    pub name: String,
    pub title: String,
    pub summary: String,
    /// ISO 639-3 code(s), comma-separated.
    pub language: String,
    /// `maxi`, `nopic`, `mini`, or empty.
    pub flavour: String,
    pub category: String,
    pub tags: String,
    /// Publication date, `YYYY-MM-DD`.
    pub date: String,
    pub article_count: u64,
    pub media_count: u64,
    /// Size of the ZIM file in bytes.
    pub size: u64,
    /// Metalink (`.meta4`) URL listing mirrors and checksums.
    pub url: String,
}

impl Book {
    pub fn has_pictures(&self) -> bool {
        self.tags.split(';').any(|t| t == "_pictures:yes")
    }

    /// The ZIM file's name, taken from its download link.
    pub fn file_name(&self) -> String {
        let last = self.url.rsplit('/').next().unwrap_or(&self.url);
        last.strip_suffix(".meta4").unwrap_or(last).to_string()
    }

    /// What the flavour means, in plain words.
    pub fn flavour_description(&self) -> &'static str {
        match self.flavour.as_str() {
            "maxi" => "complete articles with pictures",
            "nopic" => "complete articles, no pictures",
            "mini" => "introductions only, no pictures",
            _ if self.has_pictures() => "with pictures",
            _ => "without pictures",
        }
    }
}

/// A language the library has books in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// ISO 639-3 code, e.g. `eng`.
    pub code: String,
    /// English name, e.g. `English`.
    pub name: String,
    /// How many books the library has in it.
    pub books: u64,
}

/// One page of an entries feed.
#[derive(Debug, Default)]
pub struct Page {
    pub books: Vec<Book>,
    pub total: u64,
}

pub fn parse_books(feed: &str) -> Result<Page, Error> {
    let root = xml::parse(feed)?;
    if root.name != "feed" {
        return Err(Error::Format(format!(
            "expected an OPDS feed, got <{}>",
            root.name
        )));
    }
    let number = |n: &Node, name: &str| n.child_text(name).parse::<u64>().unwrap_or(0);
    let mut page = Page {
        total: number(&root, "totalResults"),
        books: Vec::new(),
    };
    for e in root.children_named("entry") {
        let acquisition = e.children_named("link").find(|l| {
            l.attr("rel")
                .is_some_and(|r| r.starts_with("http://opds-spec.org/acquisition"))
        });
        let Some(link) = acquisition else { continue };
        let issued = e.child_text("issued");
        let date = if issued.is_empty() {
            e.child_text("updated")
        } else {
            issued
        };
        page.books.push(Book {
            id: e
                .child_text("id")
                .trim_start_matches("urn:uuid:")
                .to_string(),
            name: e.child_text("name").to_string(),
            title: e.child_text("title").to_string(),
            summary: e.child_text("summary").to_string(),
            language: e.child_text("language").to_string(),
            flavour: e.child_text("flavour").to_string(),
            category: e.child_text("category").to_string(),
            tags: e.child_text("tags").to_string(),
            date: date.get(..10).unwrap_or(date).to_string(),
            article_count: number(e, "articleCount"),
            media_count: number(e, "mediaCount"),
            size: link
                .attr("length")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0),
            url: link.attr("href").unwrap_or("").to_string(),
        });
    }
    Ok(page)
}

pub fn parse_languages(feed: &str) -> Result<Vec<Language>, Error> {
    let root = xml::parse(feed)?;
    Ok(root
        .children_named("entry")
        .map(|e| Language {
            code: e.child_text("language").to_string(),
            name: e.child_text("title").to_string(),
            books: e.child_text("count").parse().unwrap_or(0),
        })
        .filter(|l| !l.code.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_books() {
        let page = parse_books(include_str!("../tests/data/entries.xml")).unwrap();
        assert_eq!(page.total, 65);
        assert_eq!(page.books.len(), 3);
        let b = &page.books[0];
        assert_eq!(
            (b.name.as_str(), b.flavour.as_str(), b.language.as_str()),
            ("wikipedia_en_all", "nopic", "eng")
        );
        assert_eq!(b.date, "2026-06-17");
        assert_eq!(b.size, 52_690_707_456);
        assert_eq!(b.article_count, 19_191_219);
        assert!(!b.has_pictures());
        assert_eq!(b.file_name(), "wikipedia_en_all_nopic_2026-06.zim");
        assert_eq!(b.flavour_description(), "complete articles, no pictures");
        assert!(page.books[1].has_pictures());
    }

    #[test]
    fn reads_languages() {
        let langs = parse_languages(include_str!("../tests/data/languages.xml")).unwrap();
        let eng = langs.iter().find(|l| l.code == "eng").unwrap();
        assert_eq!(eng.name, "English");
        assert!(eng.books > 100);
        assert_eq!(langs.len(), 3);
    }

    #[test]
    fn rejects_other_documents() {
        assert!(parse_books("<html><body/></html>").is_err());
        assert!(parse_books("not xml <").is_err());
    }
}
