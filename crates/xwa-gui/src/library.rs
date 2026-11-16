//! What the Kiwix library browser shows: which wiki, and how its files are
//! filtered and sorted. Every wiki here is made by the same scraper
//! (mwoffliner) from the same MediaWiki pages, so the converter reads them
//! all.

use xwa_fetch::Book;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wiki {
    #[default]
    Wikipedia,
    Wiktionary,
    Wikivoyage,
    Wikibooks,
    Wikiquote,
    Wikisource,
    Wikiversity,
    Vikidia,
}

impl Wiki {
    pub const ALL: [Wiki; 8] = [
        Wiki::Wikipedia,
        Wiki::Wiktionary,
        Wiki::Wikivoyage,
        Wiki::Wikibooks,
        Wiki::Wikiquote,
        Wiki::Wikisource,
        Wiki::Wikiversity,
        Wiki::Vikidia,
    ];

    /// Its category in the Kiwix library, also its key in the preferences.
    pub fn category(self) -> &'static str {
        match self {
            Wiki::Wikipedia => "wikipedia",
            Wiki::Wiktionary => "wiktionary",
            Wiki::Wikivoyage => "wikivoyage",
            Wiki::Wikibooks => "wikibooks",
            Wiki::Wikiquote => "wikiquote",
            Wiki::Wikisource => "wikisource",
            Wiki::Wikiversity => "wikiversity",
            Wiki::Vikidia => "vikidia",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Wiki::Wikipedia => "Wikipedia",
            Wiki::Wiktionary => "Wiktionary",
            Wiki::Wikivoyage => "Wikivoyage",
            Wiki::Wikibooks => "Wikibooks",
            Wiki::Wikiquote => "Wikiquote",
            Wiki::Wikisource => "Wikisource",
            Wiki::Wikiversity => "Wikiversity",
            Wiki::Vikidia => "Vikidia",
        }
    }

    /// What it is, in a few words.
    pub fn about(self) -> &'static str {
        match self {
            Wiki::Wikipedia => "The encyclopedia",
            Wiki::Wiktionary => "The dictionary",
            Wiki::Wikivoyage => "The travel guide",
            Wiki::Wikibooks => "Textbooks and manuals",
            Wiki::Wikiquote => "Quotations",
            Wiki::Wikisource => "Texts in the public domain",
            Wiki::Wikiversity => "Courses and learning",
            Wiki::Vikidia => "The encyclopedia for children",
        }
    }

    pub fn from_key(key: &str) -> Option<Wiki> {
        Wiki::ALL.into_iter().find(|w| w.category() == key)
    }
}

/// One of a filter's or the sort's choices, for the choosers.
pub trait Choice: Copy + PartialEq + Sized + 'static {
    const ALL: &'static [Self];
    fn name(self) -> &'static str;
    fn key(self) -> &'static str;

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }
}

macro_rules! choice {
    ($(#[$meta:meta])* $ty:ident { $($variant:ident => $key:literal, $name:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub enum $ty {
            #[default]
            $($variant),+
        }

        impl Choice for $ty {
            const ALL: &'static [$ty] = &[$($ty::$variant),+];

            fn name(self) -> &'static str {
                match self {
                    $($ty::$variant => $name),+
                }
            }

            fn key(self) -> &'static str {
                match self {
                    $($ty::$variant => $key),+
                }
            }
        }
    };
}

choice!(
    /// Whether files have pictures.
    Pictures {
        Any => "any", "Any",
        With => "with", "With pictures",
        Without => "without", "Without pictures",
    }
);

choice!(
    /// The whole wiki, or a selection of it on one topic.
    Selection {
        Any => "any", "Any",
        Whole => "whole", "The whole wiki",
        Topics => "topics", "Topic selections",
    }
);

choice!(
    /// Complete articles, or their introductions only.
    Articles {
        Any => "any", "Any",
        Complete => "complete", "Complete articles",
        Introductions => "intros", "Introductions only",
    }
);

choice!(
    Sort {
        Recommended => "recommended", "Recommended",
        Title => "title", "Title",
        Largest => "largest", "Largest first",
        Smallest => "smallest", "Smallest first",
        Newest => "newest", "Newest first",
        MostArticles => "articles", "Most articles",
    }
);

/// How the list is filtered and sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct View {
    pub pictures: Pictures,
    pub selection: Selection,
    pub articles: Articles,
    pub sort: Sort,
}

impl View {
    pub fn is_filtered(&self) -> bool {
        self.pictures != Pictures::Any
            || self.selection != Selection::Any
            || self.articles != Articles::Any
    }

    /// The filters and sort in a line, for the row that opens them.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.pictures != Pictures::Any {
            parts.push(self.pictures.name().to_string());
        }
        if self.selection != Selection::Any {
            parts.push(self.selection.name().to_string());
        }
        if self.articles != Articles::Any {
            parts.push(self.articles.name().to_string());
        }
        if parts.is_empty() {
            parts.push("Everything".into());
        }
        if self.sort != Sort::Recommended {
            parts.push(format!("by {}", self.sort.name().to_lowercase()));
        }
        parts.join(" · ")
    }

    pub fn keeps(&self, b: &Book) -> bool {
        let pictures = match self.pictures {
            Pictures::Any => true,
            Pictures::With => b.has_pictures(),
            Pictures::Without => !b.has_pictures(),
        };
        let whole = b.name.ends_with("_all");
        let selection = match self.selection {
            Selection::Any => true,
            Selection::Whole => whole,
            Selection::Topics => !whole,
        };
        let articles = match self.articles {
            Articles::Any => true,
            Articles::Complete => b.flavour != "mini",
            Articles::Introductions => b.flavour == "mini",
        };
        pictures && selection && articles
    }

    /// The books to list: those the filters and `search` keep, in order.
    pub fn arrange<'a>(&self, books: &'a [Book], search: &str) -> Vec<&'a Book> {
        let needle = search.to_lowercase();
        let mut kept: Vec<&Book> = books
            .iter()
            .filter(|b| self.keeps(b))
            .filter(|b| {
                needle.is_empty()
                    || b.title.to_lowercase().contains(&needle)
                    || b.name.to_lowercase().contains(&needle)
                    || b.flavour.to_lowercase().contains(&needle)
            })
            .collect();
        let flavour_rank = |f: &str| match f {
            "maxi" => 0,
            "nopic" => 1,
            "mini" => 2,
            _ => 3,
        };
        let title = |b: &Book| b.title.to_lowercase();
        kept.sort_by(|a, b| {
            let order = match self.sort {
                // The whole wiki first, then selections by title, the most
                // complete first within a title.
                Sort::Recommended => (
                    !a.name.ends_with("_all"),
                    title(a),
                    flavour_rank(&a.flavour),
                )
                    .cmp(&(
                        !b.name.ends_with("_all"),
                        title(b),
                        flavour_rank(&b.flavour),
                    )),
                Sort::Title => {
                    (title(a), flavour_rank(&a.flavour)).cmp(&(title(b), flavour_rank(&b.flavour)))
                }
                Sort::Largest => b.size.cmp(&a.size),
                Sort::Smallest => a.size.cmp(&b.size),
                Sort::Newest => b.date.cmp(&a.date),
                Sort::MostArticles => b.article_count.cmp(&a.article_count),
            };
            order.then_with(|| b.date.cmp(&a.date))
        });
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(name: &str, title: &str, flavour: &str, pictures: bool, gb: f64, date: &str) -> Book {
        Book {
            id: format!("{name}_{flavour}_{date}"),
            name: name.into(),
            title: title.into(),
            summary: String::new(),
            language: "eng".into(),
            flavour: flavour.into(),
            category: "wikipedia".into(),
            tags: if pictures {
                "_pictures:yes".into()
            } else {
                "_pictures:no".into()
            },
            date: date.into(),
            article_count: (gb * 1000.0) as u64,
            media_count: 0,
            size: (gb * 1e9) as u64,
            url: String::new(),
        }
    }

    fn shelf() -> Vec<Book> {
        vec![
            book(
                "wikipedia_en_chemistry",
                "Chemistry",
                "maxi",
                true,
                0.4,
                "2026-09",
            ),
            book(
                "wikipedia_en_all",
                "Wikipedia",
                "nopic",
                false,
                48.0,
                "2026-10",
            ),
            book(
                "wikipedia_en_all",
                "Wikipedia",
                "maxi",
                true,
                111.0,
                "2026-10",
            ),
            book(
                "wikipedia_en_all",
                "Wikipedia",
                "mini",
                false,
                12.0,
                "2026-10",
            ),
            book(
                "wikipedia_en_astronomy",
                "Astronomy",
                "nopic",
                false,
                0.3,
                "2026-08",
            ),
        ]
    }

    fn titles(books: Vec<&Book>) -> Vec<String> {
        books
            .into_iter()
            .map(|b| format!("{} {}", b.title, b.flavour))
            .collect()
    }

    #[test]
    fn the_recommended_order_puts_the_whole_wiki_first() {
        let shelf = shelf();
        assert_eq!(
            titles(View::default().arrange(&shelf, "")),
            [
                "Wikipedia maxi",
                "Wikipedia nopic",
                "Wikipedia mini",
                "Astronomy nopic",
                "Chemistry maxi"
            ]
        );
    }

    #[test]
    fn filters_combine_and_sorts_reorder() {
        let shelf = shelf();
        let view = View {
            pictures: Pictures::Without,
            articles: Articles::Complete,
            sort: Sort::Smallest,
            ..View::default()
        };
        assert_eq!(
            titles(view.arrange(&shelf, "")),
            ["Astronomy nopic", "Wikipedia nopic"]
        );
        let topics = View {
            selection: Selection::Topics,
            sort: Sort::Newest,
            ..View::default()
        };
        assert_eq!(
            titles(topics.arrange(&shelf, "")),
            ["Chemistry maxi", "Astronomy nopic"]
        );
        assert_eq!(
            titles(View::default().arrange(&shelf, "chem")),
            ["Chemistry maxi"]
        );
        assert_eq!(
            view.summary(),
            "Without pictures · Complete articles · by smallest first"
        );
        assert_eq!(View::default().summary(), "Everything");
    }

    #[test]
    fn choices_round_trip_through_their_keys() {
        for p in Pictures::ALL {
            assert_eq!(Pictures::from_key(p.key()), Some(*p));
        }
        for s in Sort::ALL {
            assert_eq!(Sort::from_key(s.key()), Some(*s));
        }
        for w in Wiki::ALL {
            assert_eq!(Wiki::from_key(w.category()), Some(w));
        }
    }
}
