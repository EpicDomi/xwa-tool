//! What each screen shows, and the device drawn round it.

use std::path::Path;
use std::time::Duration;

use iced::border::Border;
use iced::widget::{
    Space, button, canvas, column, container, image, mouse_area, opaque, pin, row, scrollable,
    space, stack, tooltip,
};
use iced::{Alignment, Background, Element, Length, Padding, Shadow, mouse};
use xwa_fetch::size_text;

use crate::app::{
    App, ArchiveFont, Choice, FirmwareChoice, Job, Message, Popup, SEARCH_ID, Screen, ZOOMS,
    ZimInfo, conversion_steps, group,
};
use crate::device::{Body, Finish, Glass, Key, Model, margin};
use crate::downloads::{Download, Fetch};
use crate::eink::{
    self, BODY, DARK, Face, INK, Kind, LIGHT, PAD, PAPER, RADIUS, Row, SIDE, SMALL, TITLE, fit,
    label, paragraph,
};
use crate::graphics::Graphics;
use crate::icons;
use crate::library::{self, Choice as _, View, Wiki};

/// The key held for shortcuts.
const MODIFIER: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};

/// What a screen holds: its header, the room it keeps above and below its
/// list, and the list.
pub struct Page {
    pub title: String,
    pub subtitle: Option<String>,
    pub back: bool,
    pub top: f32,
    pub bottom: f32,
    pub rows: Vec<Row<Message>>,
    /// Said under the list, when there is nothing (or not yet anything) in it.
    pub note: Option<String>,
    /// Labels for the four front keys.
    pub hints: [&'static str; 4],
}

const LIST_HINTS: [&str; 4] = ["« Back", "Select", "Up", "Down"];

/// The italic screen's pictures, from the firmware's simulator, and what
/// each shows.
pub const ITALIC_PICTURES: [(&[u8], &str); 3] = [
    (include_bytes!("../assets/italic-enlarged.png"), "Enlarged"),
    (include_bytes!("../assets/italic-real.png"), "Real"),
    (include_bytes!("../assets/italic-slanted.png"), "Slanted"),
];

/// The italic screen's pictures, decoded once.
fn italic_picture(i: usize) -> image::Handle {
    static PICTURES: std::sync::OnceLock<Vec<image::Handle>> = std::sync::OnceLock::new();
    PICTURES.get_or_init(|| {
        ITALIC_PICTURES
            .iter()
            .map(|(png, _)| {
                let img = ::image::load_from_memory(png)
                    .expect("bundled picture")
                    .to_rgba8();
                image::Handle::from_rgba(img.width(), img.height(), img.into_raw())
            })
            .collect()
    })[i]
        .clone()
}
const HOME_TOP: f32 = 192.0;
const TILE_TOP: f32 = 150.0;
const FIELD_TOP: f32 = 58.0;
const SUMMARY_TOP: f32 = 112.0;
const COPY_TOP: f32 = 96.0;

/// What rare characters give and cost, from the catalogue of their fonts.
fn rare_tip() -> String {
    let r = xwa_convert::fallback::rare_summary();
    let others = r.characters - r.ideographs;
    format!(
        "Adds the shapes of characters no Noto font has, which the e-reader shows as boxes: \
         {} rare Chinese characters (Unicode's extensions B to J) and {} letters and symbols \
         of scripts new to Unicode, from Plangothic, NewGardiner and GNU Unifont. Few wikis \
         use them. The \
         archive grows only by the ones its text uses: nothing for most wikis, up to about \
         {} for one that uses them all, as English Wikipedia's articles on Unicode do. Their \
         fonts ({}) are downloaded the first time they are needed. Converted without it, the \
         summary says what it would add, and Copy to SD card can add it then.",
        group(r.ideographs as u64),
        group(others as u64),
        size_text(r.bytes),
        size_text(r.download),
    )
}

/// "1 character", "2,048 characters".
pub(crate) fn characters(n: usize) -> String {
    match n {
        1 => "1 character".into(),
        n => format!("{} characters", group(n as u64)),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).floor())
}

fn time_left(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60);
    match minutes {
        0..=1 => "About a minute left".into(),
        2..=59 => format!("About {minutes} min left"),
        _ => format!("About {} h {} min left", minutes / 60, minutes % 60),
    }
}

impl App {
    pub fn page(&self) -> Page {
        match self.screen {
            Screen::Home => self.home_page(),
            Screen::Library => self.library_page(),
            Screen::Languages => self.languages_page(),
            Screen::Book => self.book_page(),
            Screen::Convert => self.convert_page(),
            Screen::Card => self.card_page(),
            Screen::Settings => self.settings_page(),
            Screen::Filter => self.filter_page(),
            Screen::Downloads => self.downloads_page(),
            Screen::Italic => self.italic_page(),
            Screen::Firmware => self.firmware_page(),
            Screen::Read => self.read_page(),
            Screen::Article => self.article_page(),
            Screen::Contents => self.contents_page(),
            Screen::Table => self.table_page(),
            Screen::Sidebar => self.sidebar_page(),
        }
    }

    fn read_page(&self) -> Page {
        let Some(wiki) = &self.wiki else {
            return Page {
                title: "Read".into(),
                subtitle: None,
                back: true,
                top: 0.0,
                bottom: 0.0,
                rows: vec![
                    Row::new(Kind::Item, "Open an archive")
                        .icon(&icons::FOLDER)
                        .opens()
                        .on_press(Message::ChooseWiki),
                ],
                note: Some(match &self.wiki_error {
                    Some(e) => format!("That archive would not open: {e}"),
                    None => {
                        "Open an archive (an .xwa file) to read it as the e-reader shows it.".into()
                    }
                }),
                hints: LIST_HINTS,
            };
        };
        let searching = !self.wiki_search.trim().is_empty();
        let rows: Vec<Row<Message>> = if searching {
            self.wiki_hits
                .iter()
                .map(|h| {
                    let place = match h.anchor {
                        Some(a) => xwa_format::content::Place::Anchor(a),
                        None => xwa_format::content::Place::TOP,
                    };
                    Row::new(Kind::Item, h.display.clone()).on_press(Message::WikiLink(
                        crate::read::layout::Link {
                            article: h.article,
                            place,
                        },
                    ))
                })
                .collect()
        } else {
            let mut rows = Vec::new();
            if let Some(r) = &self.reading {
                rows.push(
                    Row::new(Kind::Item, "Carry on reading")
                        .subtitle(r.title.clone())
                        .on_press(Message::Open(Screen::Article)),
                );
            }
            if let Some(id) = wiki.main_article() {
                rows.push(
                    Row::new(Kind::Item, "Main page")
                        .subtitle(wiki.article_title(id))
                        .on_press(Message::WikiOpen(id, 0)),
                );
            }
            let today = chrono::Local::now();
            use chrono::Datelike;
            if let Some(id) = wiki.today(today.month(), today.day()) {
                rows.push(
                    Row::new(Kind::Item, "On this day")
                        .subtitle(wiki.article_title(id))
                        .on_press(Message::WikiOpen(id, 0)),
                );
            }
            rows.push(Row::new(Kind::Item, "Random article").on_press(Message::WikiRandom));
            rows.push(
                Row::new(Kind::Item, "Open another archive")
                    .icon(&icons::FOLDER)
                    .opens()
                    .on_press(Message::ChooseWiki),
            );
            rows
        };
        let note = match (&self.wiki_error, searching && rows.is_empty()) {
            (Some(e), _) => Some(format!("That article would not open: {e}")),
            (None, true) => Some("No titles start with that.".into()),
            (None, false) => None,
        };
        Page {
            title: wiki.title.clone(),
            subtitle: Some(format!("{} articles", group(wiki.articles as u64))),
            back: true,
            top: FIELD_TOP,
            bottom: 0.0,
            rows,
            note,
            hints: LIST_HINTS,
        }
    }

    fn article_page(&self) -> Page {
        let (title, subtitle) = match &self.reading {
            Some(r) => (
                r.title.clone(),
                Some(format!("{} of {}", r.page + 1, r.laid.pages.len().max(1))),
            ),
            None => ("Read".into(), None),
        };
        let (_, h) = self.reading_size();
        Page {
            title,
            subtitle,
            back: true,
            top: h + 20.0,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: ["« Back", "Contents", "Prev", "Next"],
        }
    }

    fn table_page(&self) -> Page {
        let (_, h) = self.reading_size();
        Page {
            title: "Table".into(),
            subtitle: self.reading.as_ref().map(|r| r.title.clone()),
            back: true,
            top: h + 20.0,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: ["« Back", "Across", "Up", "Down"],
        }
    }

    fn contents_page(&self) -> Page {
        let Some(r) = &self.reading else {
            return Page {
                title: "Contents".into(),
                subtitle: None,
                back: true,
                top: 0.0,
                bottom: 0.0,
                rows: Vec::new(),
                note: None,
                hints: LIST_HINTS,
            };
        };
        // The article's sidebars first: few, and easily missed below a long
        // list of sections.
        let mut rows: Vec<Row<Message>> = r
            .sidebars
            .iter()
            .enumerate()
            .map(|(i, sb)| {
                let links: usize = sb.groups.iter().map(|g| g.links.len()).sum();
                Row::new(Kind::Item, sb.title.clone())
                    .subtitle(format!("{links} articles"))
                    .opens()
                    .on_press(Message::WikiSidebar(i))
            })
            .collect();
        rows.push(Row::new(Kind::Item, "Top").on_press(Message::WikiSection(0)));
        for (i, (level, text)) in r.toc.iter().enumerate() {
            let indent = "    ".repeat(level.saturating_sub(2) as usize);
            rows.push(
                Row::new(Kind::Item, format!("{indent}{text}"))
                    .on_press(Message::WikiSection(i + 1)),
            );
        }
        Page {
            title: "Contents".into(),
            subtitle: Some(r.title.clone()),
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows,
            note: None,
            hints: LIST_HINTS,
        }
    }

    fn sidebar_page(&self) -> Page {
        let sidebar = self
            .reading
            .as_ref()
            .and_then(|r| Some((r, r.sidebars.get(self.reading_sidebar)?)));
        let Some((r, sb)) = sidebar else {
            return Page {
                title: "Series".into(),
                subtitle: None,
                back: true,
                top: 0.0,
                bottom: 0.0,
                rows: Vec::new(),
                note: None,
                hints: LIST_HINTS,
            };
        };
        let mut rows = Vec::new();
        for g in &sb.groups {
            if !g.heading.is_empty() {
                rows.push(Row::new(Kind::Heading, g.heading.clone()));
            }
            for l in &g.links {
                let link = crate::read::layout::Link {
                    article: l.article,
                    place: l.place,
                };
                rows.push(
                    Row::new(Kind::Item, l.label.clone()).on_press(Message::WikiSidebarLink(link)),
                );
            }
        }
        // "Part of a series on Biology" is too long for the title bar; the
        // series' name says it.
        let title = ["Part of a series on ", "Part of a series about "]
            .iter()
            .find_map(|p| sb.title.strip_prefix(p))
            .map_or(sb.title.as_str(), |rest| {
                rest.strip_prefix("the ").unwrap_or(rest)
            });
        let mut title = title.to_string();
        if let Some(c) = title.get(..1) {
            title = c.to_uppercase() + &title[1..];
        }
        Page {
            title,
            subtitle: Some(r.title.clone()),
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows,
            note: None,
            hints: LIST_HINTS,
        }
    }

    fn home_page(&self) -> Page {
        let short = self.prefs.model.short_name();
        let convert = match (self.conversion.fraction(), &self.zim) {
            (Some(f), _) => format!("Converting… {}", percent(f)),
            (None, Some(zim)) => file_name(zim),
            (None, None) => format!("Make a ZIM file readable on the {short}"),
        };
        let copy = match (self.copy.fraction(), &self.archive) {
            (Some(f), _) => format!("Copying… {}", percent(f)),
            (None, Some(a)) => file_name(a),
            (None, None) => "Put an archive on your e-reader's card".into(),
        };
        let settings = format!(
            "{short} · {} case · {:.0}% size",
            self.prefs.finish().name(),
            self.zoom * 100.0
        );
        Page {
            title: "xwa-tool".into(),
            subtitle: Some(self.prefs.model.name().into()),
            back: false,
            top: HOME_TOP,
            bottom: 0.0,
            rows: [
                Some(
                    Row::new(Kind::Menu, "Get Wikis")
                        .icon(&icons::LIBRARY)
                        .subtitle("Wikipedia and more, from Kiwix")
                        .tip(
                            "Find Wikipedia, Wiktionary and other wikis in your language and \
                             download them. Kiwix packs each one in a single ZIM file, which \
                             Convert then turns into an archive for the e-reader.",
                        )
                        .on_press(Message::Open(Screen::Library)),
                ),
                // Only while there are downloads to see.
                self.downloads.summary().map(|s| {
                    Row::new(Kind::Menu, "Downloads")
                        .icon(&icons::DOWNLOAD)
                        .subtitle(s)
                        .on_press(Message::Open(Screen::Downloads))
                }),
                Some(
                    Row::new(Kind::Menu, "Convert")
                        .icon(&icons::CONVERT)
                        .subtitle(convert)
                        .tip(
                            "Turn a ZIM file into an archive (an .xwa file) that the e-reader \
                             can open. A large wiki takes from minutes to a few hours.",
                        )
                        .on_press(Message::Open(Screen::Convert)),
                ),
                Some(
                    Row::new(Kind::Menu, "Copy to SD card")
                        .icon(&icons::TRANSFER)
                        .subtitle(copy)
                        .tip(
                            "Put an archive on the e-reader's SD card, in the folder the \
                             e-reader looks in. Put the card in this computer, or connect the \
                             e-reader to it, first.",
                        )
                        .on_press(Message::Open(Screen::Card)),
                ),
                Some(
                    Row::new(Kind::Menu, "Read")
                        .icon(&icons::BOOK)
                        .subtitle(match &self.wiki {
                            Some(w) => w.title.clone(),
                            None => "See an archive as the e-reader shows it".into(),
                        })
                        .tip(
                            "Open an archive and read it here, laid out in pages as the \
                             e-reader shows it: search, follow links, and see its pictures, \
                             tables and formulas before copying it to the card.",
                        )
                        .on_press(match (&self.wiki, &self.archive) {
                            // The archive just made or copied, unless one is open.
                            (None, Some(a)) => Message::WikiChosen(Some(a.clone())),
                            _ => Message::Open(Screen::Read),
                        }),
                ),
                Some(
                    Row::new(Kind::Menu, "Firmware")
                        .icon(&icons::USB)
                        .subtitle(if self.install.running() {
                            format!("Installing… {}", self.install.label())
                        } else {
                            "Install or update the e-reader's firmware".into()
                        })
                        .tip(
                            "Installs the latest release of the e-reader's firmware over a USB \
                             cable (or another release you choose), keeping its books, \
                             settings and SD card.",
                        )
                        .on_press(Message::Open(Screen::Firmware)),
                ),
                Some(
                    Row::new(Kind::Menu, "Settings")
                        .icon(&icons::SETTINGS)
                        .subtitle(settings)
                        .on_press(Message::Open(Screen::Settings)),
                ),
            ]
            .into_iter()
            .flatten()
            .collect(),
            note: None,
            hints: ["", "Select", "Up", "Down"],
        }
    }

    fn library_page(&self) -> Page {
        let wiki = self.prefs.wiki;
        let view = self.prefs.view;
        let mut rows = vec![
            Row::new(Kind::Setting, "Wiki")
                .value(wiki.name())
                .tip(
                    "Which wiki to list: Wikipedia (the encyclopedia), Wiktionary (a \
                     dictionary), Wikivoyage (travel guides), Wikibooks (textbooks), \
                     Wikiquote (quotations), Wikisource (texts), Wikiversity (courses) or \
                     Vikidia (an encyclopedia for children).",
                )
                .on_press(Message::Choose(Choice::Wiki)),
            Row::new(Kind::Setting, "Language")
                .value(
                    self.language
                        .as_ref()
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "…".into()),
                )
                .opens()
                .on_press_maybe(
                    (!self.languages.is_empty()).then_some(Message::Open(Screen::Languages)),
                ),
            Row::new(Kind::Item, "Filter and sort")
                .subtitle(view.summary())
                .opens()
                .on_press(Message::Open(Screen::Filter)),
        ];
        debug_assert_eq!(rows.len(), LIBRARY_ROWS);
        let shown = view.arrange(&self.books, &self.search);
        for b in &shown {
            // A book on the downloads list shows how it is getting on.
            let value = match self.downloads.get(&b.id) {
                Some(d) => d.badge(),
                None => size_text(b.size),
            };
            rows.push(
                Row::new(Kind::Item, b.title.clone())
                    .subtitle(format!("{} · {}", b.flavour_description(), b.date))
                    .value(value)
                    .on_press(Message::OpenBook((*b).clone())),
            );
        }
        let mut note = self.library_status.clone();
        if note.is_none() && !self.books.is_empty() && shown.is_empty() {
            note = Some(if self.search.is_empty() {
                "Nothing here matches the filters.".into()
            } else {
                "Nothing matches the search.".into()
            });
        }
        if self.library_failed {
            rows.push(Row::new(Kind::Action, "Try again").on_press(Message::LoadLibrary));
        }
        Page {
            title: "Get Wikis".into(),
            subtitle: self.language.as_ref().map(|l| l.name.clone()),
            back: true,
            top: FIELD_TOP,
            bottom: 0.0,
            rows,
            note,
            hints: ["« Back", "Open", "Up", "Down"],
        }
    }

    fn filter_page(&self) -> Page {
        let view = self.prefs.view;
        let shown = view.arrange(&self.books, "").len();
        Page {
            title: "Filter and sort".into(),
            subtitle: Some(self.prefs.wiki.name().into()),
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows: vec![
                Row::new(Kind::Setting, "Pictures")
                    .value(view.pictures.name())
                    .tip(
                        "Files without pictures are many times smaller and keep all the \
                         text, tables and formulas.",
                    )
                    .on_press(Message::Choose(Choice::Pictures)),
                Row::new(Kind::Setting, "Selection")
                    .value(view.selection.name())
                    .tip(
                        "Topic selections are small collections from the whole wiki, such \
                         as chemistry, medicine or football.",
                    )
                    .on_press(Message::Choose(Choice::Selection)),
                Row::new(Kind::Setting, "Articles")
                    .value(view.articles.name())
                    .tip(
                        "\"Introductions only\" keeps the start of each article, before its \
                         first heading: a fraction of the size, and enough to look things up.",
                    )
                    .on_press(Message::Choose(Choice::Articles)),
                Row::new(Kind::Setting, "Sort by")
                    .value(view.sort.name())
                    .on_press(Message::Choose(Choice::Sort)),
                Row::new(Kind::Item, "Show everything").on_press_maybe(view.is_filtered().then(
                    || {
                        Message::SetView(View {
                            sort: view.sort,
                            ..View::default()
                        })
                    },
                )),
            ],
            note: (!self.books.is_empty())
                .then(|| format!("{shown} of the {} files are shown.", self.books.len())),
            hints: ["« Back", "Change", "Up", "Down"],
        }
    }

    fn languages_page(&self) -> Page {
        let needle = self.language_search.to_lowercase();
        let rows: Vec<_> = self
            .languages
            .iter()
            .filter(|l| {
                needle.is_empty()
                    || l.name.to_lowercase().contains(&needle)
                    || l.code.starts_with(&needle)
            })
            .map(|l| {
                let row = Row::new(Kind::Item, l.name.clone())
                    .on_press(Message::LanguageChosen(l.clone()));
                if self.language.as_ref() == Some(l) {
                    row.value(SELECTED)
                } else {
                    row
                }
            })
            .collect();
        let note = rows
            .is_empty()
            .then(|| "No language matches the search.".to_string());
        Page {
            title: "Language".into(),
            subtitle: Some(format!("{} languages", self.languages.len())),
            back: true,
            top: FIELD_TOP,
            bottom: 0.0,
            rows,
            note,
            hints: LIST_HINTS,
        }
    }

    fn book_page(&self) -> Page {
        let Some(b) = &self.book else {
            return self.home_page();
        };
        let state = self.downloads.get(&b.id).map(|d| &d.state);
        let id = || b.id.clone();
        let remove = |what: &str| {
            Row::new(Kind::Item, "Remove")
                .subtitle(what.to_string())
                .on_press(Message::RemoveDownload(id()))
        };
        let downloads = Row::new(Kind::Item, "Downloads")
            .icon(&icons::DOWNLOAD)
            .subtitle(self.downloads.summary().unwrap_or_default())
            .opens()
            .on_press(Message::Open(Screen::Downloads));
        let mut page = Page {
            title: b.title.clone(),
            subtitle: (!b.flavour.is_empty()).then(|| b.flavour.clone()),
            back: true,
            top: SUMMARY_TOP,
            bottom: if self.book_note().is_some() {
                96.0
            } else {
                0.0
            },
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        if let Some(Fetch::Running { .. }) = state {
            page.top += progress_height(None);
            page.rows = vec![
                Row::new(Kind::Item, "Pause")
                    .subtitle("Carry on later from where it stopped")
                    .on_press(Message::PauseDownload(id())),
                remove("Stop, and delete what was downloaded"),
                downloads,
            ];
            return page;
        }
        page.rows = vec![
            Row::new(Kind::Info, "Size").value(size_text(b.size)),
            Row::new(Kind::Info, "Articles").value(group(b.article_count)),
            Row::new(Kind::Info, "Pictures").value(if b.has_pictures() {
                group(b.media_count)
            } else {
                "None".into()
            }),
            Row::new(Kind::Info, "Published").value(b.date.clone()),
        ];
        let more = match state {
            None => vec![
                Row::new(Kind::Item, "Save in")
                    .icon(&icons::FOLDER)
                    .subtitle(self.download_dir().display().to_string())
                    .tip(
                        "Where the ZIM file is downloaded. Once it is converted it can be \
                         deleted, unless you want to convert it again.",
                    )
                    .opens()
                    .on_press(Message::ChooseDownloadDir),
                Row::new(Kind::Action, "Download").on_press(Message::Download),
            ],
            Some(Fetch::Waiting) => vec![downloads, remove("Take it off the list")],
            Some(Fetch::Paused { .. }) => vec![
                Row::new(Kind::Action, "Resume").on_press(Message::ResumeDownload(id())),
                remove("Delete what was downloaded"),
                downloads,
            ],
            Some(Fetch::Done(path)) => vec![
                Row::new(Kind::Action, "Convert").on_press_maybe(
                    (!self.conversion.running()).then(|| Message::ZimChosen(Some(path.clone()))),
                ),
                downloads,
            ],
            Some(Fetch::Failed(_)) => vec![
                Row::new(Kind::Action, "Try again").on_press(Message::ResumeDownload(id())),
                remove("Delete what was downloaded"),
            ],
            Some(Fetch::Running { .. }) => Vec::new(),
        };
        page.rows.extend(more);
        page
    }

    /// What the book screen says under its list about the book's download.
    fn book_note(&self) -> Option<String> {
        let b = self.book.as_ref()?;
        match self.downloads.get(&b.id).map(|d| &d.state) {
            None if self.downloads.is_running() => {
                Some("Another file is downloading. This one will wait its turn.".into())
            }
            None | Some(Fetch::Running { .. }) => None,
            Some(Fetch::Waiting) => {
                Some("Waiting its turn: it starts when the files before it are done.".into())
            }
            Some(Fetch::Paused { fraction }) => Some(format!(
                "Paused at {}. Resuming carries on from there.",
                percent(*fraction)
            )),
            Some(Fetch::Done(path)) => Some(format!("Downloaded to {}", path.display())),
            Some(Fetch::Failed(e)) => Some(format!("The download stopped: {e}")),
        }
    }

    fn downloads_page(&self) -> Page {
        let rows = self
            .downloads
            .items
            .iter()
            .enumerate()
            .map(|(i, d)| {
                Row::new(Kind::Item, d.book.title.clone())
                    .subtitle(d.line())
                    .value(d.badge())
                    .on_press(Message::DownloadMenu(i))
            })
            .collect();
        Page {
            title: "Downloads".into(),
            subtitle: None,
            back: true,
            top: if self.downloads.is_running() {
                COPY_TOP + progress_height(None)
            } else {
                0.0
            },
            bottom: 0.0,
            rows,
            note: self.downloads.items.is_empty().then(|| {
                "Nothing is downloading. Files chosen in Get Wikis wait here for their turn."
                    .to_string()
            }),
            hints: ["« Back", "Choose", "Up", "Down"],
        }
    }

    fn convert_page(&self) -> Page {
        let short = self.prefs.model.short_name();
        let mut page = Page {
            title: "Convert".into(),
            subtitle: Some(format!("for the {short}")),
            back: true,
            top: TILE_TOP,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        let Some(zim) = &self.zim else {
            page.top = 190.0;
            page.rows = vec![
                Row::new(Kind::Action, "Choose a ZIM file").on_press(Message::ChooseZim),
                Row::new(Kind::Item, "Get Wikis")
                    .subtitle("Download one from the Kiwix library")
                    .opens()
                    .on_press(Message::Open(Screen::Library)),
            ];
            return page;
        };
        if self.conversion.running() {
            let steps = conversion_steps(self.converting_images, self.converting_sharper);
            page.top += progress_height(Some(steps.len()));
            page.rows = vec![stop_row(
                &self.conversion,
                "Stop converting",
                Message::AskStopConversion,
            )];
            return page;
        }
        if let Job::Finished(Ok(_)) = &self.conversion {
            page.bottom = 150.0;
            page.rows = vec![
                Row::new(Kind::Action, "Copy to SD card").on_press(Message::Open(Screen::Card)),
                Row::new(Kind::Item, "Convert again")
                    .subtitle("With other settings, or for another device")
                    .on_press(Message::ConvertAgain),
                Row::new(Kind::Item, "Convert another file")
                    .opens()
                    .on_press(Message::ChooseZim),
            ];
            return page;
        }
        let readable = matches!(self.zim_info, Some(Ok(_)));
        page.rows = vec![
            Row::new(Kind::Item, "File")
                .subtitle(file_name(zim))
                .tip(format!(
                    "The ZIM file to convert: {}. Choose this to pick another.",
                    zim.display()
                ))
                .opens()
                .on_press(Message::ChooseZim),
            self.save_row(),
            Row::new(Kind::Setting, "Made for")
                .value(short)
                .tip(
                    "The e-reader the archive is for. The X4 Pro's archives use stronger \
                     compression, which the X4 and X3 cannot read, so convert again for \
                     another model.",
                )
                .on_press(Message::Choose(Choice::Device)),
            if self.zim_has_pictures() {
                Row::new(Kind::Setting, "Pictures")
                    .value(on_off(self.images))
                    .tip(
                        "Off leaves the pictures out: a far smaller archive, with all the text, \
                         tables and formulas. Useful when the SD card is short of space.",
                    )
                    .on_press(Message::ToggleImages)
            } else {
                Row::new(Kind::Setting, "Pictures")
                    .value("None in this ZIM")
                    .tip(
                        "This ZIM was made without pictures (Kiwix's \"no pictures\" or \
                         \"mini\" version), so the archive has none either. Get the full \
                         version of the wiki for its pictures.",
                    )
            },
            Row::new(Kind::Setting, "Sharper diagrams")
                .subtitle("From Wikipedia, over the internet")
                .tip(
                    "Replaces the ZIM's small pictures of diagrams, maps and charts with \
                     sharper ones from Wikipedia, so their labels can be read. Needs the \
                     internet and takes longer the first time; they are kept, so converting \
                     again downloads nothing.",
                )
                .value(on_off(self.pictures() && self.sharper))
                .on_press_maybe(self.pictures().then_some(Message::ToggleSharper)),
            Row::new(Kind::Setting, "Real italic")
                .subtitle("For letters the e-reader's fonts lack")
                .tip(
                    "For Greek, Cyrillic, symbols and other letters the e-reader's own fonts \
                     do not have, the archive carries their real italic shapes rather than \
                     slanted upright ones. It adds very little to the archive.",
                )
                .value(on_off(self.real_italic))
                .on_press(Message::ToggleItalic),
            Row::new(Kind::Item, "See the difference")
                .tip("Pictures of the e-reader's screen with real and slanted italic.")
                .opens()
                .on_press(Message::Open(Screen::Italic)),
            Row::new(Kind::Setting, "Rare characters")
                .subtitle("Rare Chinese characters, newest scripts")
                .tip(rare_tip())
                .value(on_off(self.rare_characters))
                .on_press(Message::ToggleRare),
            Row::new(
                Kind::Action,
                if matches!(self.conversion, Job::Finished(Err(_))) {
                    "Try again"
                } else {
                    "Convert"
                },
            )
            .on_press_maybe(readable.then_some(Message::Convert)),
        ];
        if matches!(self.conversion, Job::Finished(Err(_))) {
            page.bottom = 96.0;
        }
        page
    }

    /// Convert: where the archive is saved, and whether it likely fits.
    fn save_row(&self) -> Row<Message> {
        let row = Row::new(Kind::Item, "Save to")
            .opens()
            .on_press(Message::ChooseSaveDir);
        let Some(dir) = self.save_dir() else {
            return row.subtitle("Choose a folder");
        };
        let free = self
            .save_free
            .as_ref()
            .filter(|(d, _)| *d == dir)
            .map(|(_, f)| *f);
        let estimate = self.archive_estimate();
        let mut tip = format!("The archive is saved in {}", dir.display());
        match (estimate, free) {
            (Some(e), Some(f)) => {
                tip += &format!(
                    ". It will be up to about {} ({} is free there).",
                    size_text(e),
                    size_text(f)
                )
            }
            (Some(e), None) => tip += &format!(". It will be up to about {}.", size_text(e)),
            (None, Some(f)) => tip += &format!(" ({} is free there).", size_text(f)),
            (None, None) => tip += ".",
        }
        tip += " Choose this to save it somewhere else.";
        let row = row.tip(tip);
        match (self.archive_fits(), estimate, free) {
            (Some(false), Some(e), Some(f)) => row.subtitle(format!(
                "Needs up to about {} · {} free",
                size_text(e),
                size_text(f)
            )),
            (_, _, Some(f)) => row
                .subtitle(format!("{} · {} free", dir.display(), size_text(f)))
                .keep_end(),
            _ => row.subtitle(dir.display().to_string()).keep_end(),
        }
    }

    fn firmware_page(&self) -> Page {
        let mut page = Page {
            title: "Firmware".into(),
            subtitle: None,
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        if self.install.running() {
            page.top = COPY_TOP + progress_height(None);
            // Connecting and downloading can stop; writing cannot.
            page.rows = vec![if self.install.step().unwrap_or(0) <= 1 {
                stop_row(&self.install, "Stop", Message::CancelInstall)
            } else {
                Row::new(Kind::Info, "Keep the reader plugged in")
                    .subtitle("Writing takes a minute or two")
            }];
            return page;
        }
        let reader = Row::new(Kind::Item, "Reader")
            .tip(
                "Connect the e-reader to this computer with a USB cable that carries data \
                 (some carry only power), and switch it on. It shows here by itself.",
            )
            .on_press(Message::LookForReaders);
        let reader = match self.readers.as_slice() {
            [] => reader.subtitle("Connect it by USB and switch it on"),
            [one] => reader.subtitle(format!("Connected on {}", one.name)),
            more => reader.subtitle(format!("{} connected: connect only one", more.len())),
        };
        let firmware = Row::new(Kind::Item, "Firmware").tip(
            "Which firmware to install: the latest release unless you choose another. With \
             \"Allow other firmware\" on in Settings, a firmware file can be chosen too.",
        );
        let can_choose =
            matches!(&self.releases, Some(Ok(r)) if !r.is_empty()) || self.prefs.any_firmware;
        let firmware = match (&self.firmware_choice, &self.releases, self.chosen_release()) {
            (FirmwareChoice::File(path), _, _) => firmware.subtitle(file_name(path)),
            (_, _, Some(r)) => firmware.subtitle(format!(
                "{} · released {}{}",
                r.tag,
                r.date(),
                if r.prerelease { " · pre-release" } else { "" }
            )),
            (_, None, _) => firmware.subtitle("Looking for releases…"),
            (_, Some(Ok(_)), None) => firmware.subtitle("No releases yet"),
            (_, Some(Err(e)), None) => firmware
                .subtitle("Could not get the releases; choose to try again")
                .tip(e.clone()),
        };
        let firmware = match (&self.releases, can_choose) {
            (_, true) => firmware.opens().on_press(Message::Choose(Choice::Firmware)),
            (Some(Err(_)), false) => firmware.on_press(Message::LoadReleases),
            _ => firmware,
        };
        page.rows = vec![
            reader,
            firmware,
            Row::new(Kind::Action, "Install")
                .on_press_maybe(self.can_install().then_some(Message::AskInstallFirmware)),
        ];
        page.bottom = 150.0;
        page
    }

    fn italic_page(&self) -> Page {
        let spec = self.spec();
        let hints = if spec.hints.is_some() {
            eink::HINTS
        } else {
            0.0
        };
        let i = self.italic_picture;
        Page {
            title: "Italic".into(),
            subtitle: Some(format!(
                "{} · {} of {}",
                ITALIC_PICTURES[i].1,
                i + 1,
                ITALIC_PICTURES.len()
            )),
            back: true,
            top: spec.screen.height - eink::HEADER - hints,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: ["« Back", "Next", "Previous", "Next"],
        }
    }

    fn card_page(&self) -> Page {
        let mut page = Page {
            title: "Copy to SD card".into(),
            subtitle: None,
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        if self.copy.running() {
            page.top = COPY_TOP + progress_height(None);
            page.rows = vec![stop_row(&self.copy, "Stop copying", Message::CancelCopy)];
            return page;
        }
        if self.font_work.running() {
            page.top = COPY_TOP + progress_height(None);
            page.rows = vec![stop_row(&self.font_work, "Stop", Message::CancelFontWork)];
            return page;
        }
        page.rows.push(
            Row::new(Kind::Item, "Archive")
                .subtitle(
                    self.archive
                        .as_deref()
                        .map(file_name)
                        .unwrap_or_else(|| "Choose an .xwa file".into()),
                )
                .tip("The converted archive (.xwa file) to put on the card.")
                .opens()
                .on_press(Message::ChooseArchive),
        );
        let rare = self.archive_font.rare();
        match &self.archive_font {
            ArchiveFont::Missing { glyphs, .. } if self.archive.is_some() => page.rows.push(
                Row::new(Kind::Item, "Complete its fallback font")
                    .subtitle(format!(
                        "{} glyphs missing · downloads Noto fonts",
                        group(*glyphs as u64)
                    ))
                    .tip(
                        "Some letters in this archive have no shape to draw, because their \
                         fonts could not be downloaded when it was converted (offline, say). \
                         This downloads them and adds them to the archive, without \
                         converting again.",
                    )
                    .on_press(Message::CompleteFont),
            ),
            ArchiveFont::Unchecked if self.archive.is_some() => page.rows.push(
                Row::new(Kind::Item, "Check its fallback font")
                    .subtitle("Reads its text to find missing glyphs")
                    .tip(
                        "Archives made by older versions do not list the letters they use. \
                         This reads the archive's text once to find letters it has no shape \
                         for, which can then be added.",
                    )
                    .on_press(Message::CheckFont),
            ),
            _ => {}
        }
        if rare.chars > 0 && self.archive.is_some() {
            page.rows.push(
                Row::new(Kind::Item, "Add rare characters")
                    .subtitle(format!(
                        "{} · about {} more",
                        characters(rare.chars),
                        size_text(rare.bytes)
                    ))
                    .tip(format!(
                        "This archive's text has {} no Noto font has, such as rare Chinese \
                         characters or letters of scripts new to Unicode, which the e-reader \
                         shows as boxes. This adds their shapes, from Plangothic and GNU \
                         Unifont, without converting again: about {} more on the card. The \
                         fonts are downloaded the first time.",
                        characters(rare.chars),
                        size_text(rare.bytes)
                    ))
                    .on_press(Message::AddRareCharacters),
            );
        }
        for d in &self.drives {
            let name = if d.name.is_empty() {
                "Removable drive".to_string()
            } else {
                d.name.clone()
            };
            let row = Row::new(Kind::Item, name)
                .icon(&icons::CARD)
                .subtitle(drive_line(d))
                .on_press(Message::DriveChosen(d.clone()));
            page.rows.push(if self.drive.as_ref() == Some(d) {
                row.value(SELECTED)
            } else {
                row
            });
        }
        page.rows.push(
            Row::new(Kind::Item, "Look for cards again")
                .tip(
                    "Put the e-reader's SD card in this computer, or connect the e-reader, then \
                 choose this if the card is not listed.",
                )
                .on_press(Message::LookForCards),
        );
        page.rows
            .push(Row::new(Kind::Action, "Copy").on_press_maybe(
                (self.archive.is_some() && self.drive.is_some()).then_some(Message::CopyToCard),
            ));
        if self.drives.is_empty()
            || matches!(self.copy, Job::Finished(_))
            || matches!(self.font_work, Job::Finished(_))
        {
            page.bottom = 120.0;
        }
        page
    }

    fn settings_page(&self) -> Page {
        let converting = self.conversion.running();
        Page {
            title: "Settings".into(),
            subtitle: None,
            back: true,
            top: 0.0,
            bottom: 170.0,
            rows: vec![
                Row::new(Kind::Setting, "Device")
                    .value(self.prefs.model.short_name())
                    .tip("The e-reader this app shows, and converts for unless you choose another.")
                    .on_press_maybe((!converting).then_some(Message::Choose(Choice::Device))),
                Row::new(Kind::Setting, "Case")
                    .value(self.prefs.finish().name())
                    .on_press(Message::Choose(Choice::Case)),
                Row::new(Kind::Setting, "Display size")
                    .value(format!("{:.0}%", self.zoom * 100.0))
                    .tip("How large the e-reader is drawn. Dragging the window's edges does the same.")
                    .on_press(Message::Choose(Choice::Size)),
                {
                    let row = Row::new(Kind::Setting, "Graphics")
                        .value(match self.prefs.graphics {
                            Graphics::Auto => self.graphics.name().to_string(),
                            g => g.name().to_string(),
                        })
                        .tip(
                            "How the app draws itself. Leave it on Automatic; if the window is \
                             blank, black or flickers, try another. It takes effect the next \
                             time the app starts.",
                        )
                        .on_press(Message::Choose(Choice::Graphics));
                    if self.prefs.graphics != Graphics::Auto && self.prefs.graphics != self.graphics
                    {
                        row.subtitle("From the next start")
                    } else {
                        row
                    }
                },
                Row::new(Kind::Setting, "Refresh flash")
                    .value(on_off(self.prefs.refresh_flash))
                    .tip(
                        "Flashes the screen black and white when it changes, as e-ink screens \
                         do. Turn it off if you find it distracting.",
                    )
                    .on_press(Message::ToggleFlash),
                self.fonts_row(),
                Row::new(Kind::Setting, "Allow other firmware")
                    .value(on_off(self.prefs.any_firmware))
                    .tip(
                        "Off, only the firmware's own releases can be installed. On, any \
                         firmware file for the reader's chip can be too, such as one you \
                         built or another project's. Firmware that cannot read XWA archives \
                         will not open them.",
                    )
                    .on_press(Message::ToggleAnyFirmware),
                Row::new(Kind::Item, "Download folder")
                    .icon(&icons::FOLDER)
                    .tip("Where downloaded ZIM files are saved.")
                    .subtitle(self.download_dir().display().to_string())
                    .opens()
                    .on_press(Message::ChooseDownloadDir),
                Row::new(Kind::Info, "Version").value(env!("CARGO_PKG_VERSION")),
                Row::new(Kind::Info, "Archive format").tip(
                    "The version of the archive format this app writes. The e-reader's \
                     firmware must support it to open the archives.",
                ).value(format!(
                    "XWA {}.{}",
                    xwa_format::VERSION_MAJOR,
                    xwa_format::VERSION_MINOR
                )),
            ],
            note: None,
            hints: ["« Back", "Change", "Up", "Down"],
        }
    }

    /// Settings: keep every Noto font the fallback font could need, for
    /// converting offline.
    fn fonts_row(&self) -> Row<Message> {
        let row = Row::new(Kind::Item, "Fonts for converting offline").tip(
            "Downloads every font (about 120 MB) a conversion might need for letters the \
             e-reader's own fonts lack, so that converting without the internet still gives \
             archives that can show every letter.",
        );
        if self.font_download.running() {
            return row
                .subtitle(self.font_download.label().to_string())
                .value(percent(self.font_download.fraction().unwrap_or(0.0)))
                .on_press(Message::DownloadFonts);
        }
        if let Job::Finished(Err(e)) = &self.font_download {
            return row
                .subtitle(format!("Stopped: {e}"))
                .on_press(Message::DownloadFonts);
        }
        match self.fonts_kept {
            Some(k) if k.all() => row.subtitle(format!("All {} Noto font files kept", k.total)),
            Some(k) => row
                .subtitle(format!(
                    "{} of {} kept · download the rest ({})",
                    k.kept,
                    k.total,
                    size_text(k.missing_bytes)
                ))
                .on_press(Message::DownloadFonts),
            None => row.subtitle("No folder to keep them in"),
        }
    }

    // -----------------------------------------------------------------------
    // The space above and below the lists

    fn top_view(&self) -> Element<'_, Message> {
        let w = self.spec().screen.width;
        match self.screen {
            Screen::Home => self.home_tile(w),
            Screen::Library => search(eink::field(
                "Search titles",
                &self.search,
                Message::Search,
                Some(Message::Submit),
                SEARCH_ID,
            )),
            Screen::Read if self.wiki.is_some() => search(eink::field(
                "Search titles",
                &self.wiki_search,
                Message::WikiSearch,
                Some(Message::Submit),
                SEARCH_ID,
            )),
            Screen::Article => match (&self.wiki, &self.reading) {
                (Some(wiki), Some(r)) => {
                    let (rw, rh) = self.reading_size();
                    container(
                        canvas(crate::read::draw::PageView {
                            wiki,
                            laid: &r.laid,
                            page: r.page,
                            on_link: Message::WikiTarget,
                        })
                        .width(rw)
                        .height(rh),
                    )
                    .padding(Padding {
                        top: 12.0,
                        bottom: 8.0,
                        left: SIDE,
                        right: SIDE,
                    })
                    .into()
                }
                _ => Space::new().into(),
            },
            Screen::Table => match (&self.wiki, &self.reading_table) {
                (Some(wiki), Some(t)) => {
                    let (rw, rh) = self.reading_size();
                    let title = paragraph(t.title.clone(), Face::UiBold, BODY);
                    let table = canvas(crate::read::draw::PageView {
                        wiki,
                        laid: &t.laid,
                        page: 0,
                        on_link: Message::WikiTarget,
                    })
                    .width(t.width)
                    .height(t.height);
                    container(
                        column![
                            title,
                            scrollable(table)
                                .id(crate::app::TABLE_ID)
                                .direction(scrollable::Direction::Both {
                                    vertical: scrollable::Scrollbar::new().width(4).scroller_width(4),
                                    horizontal: scrollable::Scrollbar::new().width(4).scroller_width(4),
                                })
                                .width(rw)
                                .height(rh - 34.0),
                        ]
                        .spacing(8),
                    )
                    .padding(Padding {
                        top: 10.0,
                        bottom: 8.0,
                        left: SIDE,
                        right: SIDE,
                    })
                    .into()
                }
                _ => Space::new().into(),
            },
            Screen::Languages => search(eink::field(
                "Search languages",
                &self.language_search,
                Message::LanguageSearch,
                Some(Message::Submit),
                SEARCH_ID,
            )),
            Screen::Book => {
                let summary = self.book.as_ref().map(|b| b.summary.as_str()).unwrap_or("");
                let summary = container(paragraph(clip_words(summary, 180), Face::Ui, BODY))
                    .padding([10.0, SIDE + PAD])
                    .height(SUMMARY_TOP);
                match self
                    .book
                    .as_ref()
                    .and_then(|b| self.downloads.get(&b.id))
                    .and_then(Download::job)
                {
                    Some(job) => column![summary, progress_view(job, None)].into(),
                    None => summary.into(),
                }
            }
            Screen::Downloads => match self.downloads.running() {
                Some(d) => column![
                    heading(&d.book.title, d.book.flavour_description(), w),
                    d.job().map_or_else(|| Space::new().into(), |j| progress_view(j, None)),
                ]
                .into(),
                None => Space::new().into(),
            },
            Screen::Convert => match &self.zim {
                Some(zim) if self.conversion.running() => {
                    let steps = conversion_steps(self.converting_images, self.converting_sharper);
                    column![
                        container(zim_tile(zim, self.zim_info.as_ref(), w)).height(TILE_TOP),
                        progress_view(&self.conversion, Some(&steps)),
                    ]
                    .into()
                }
                Some(zim) => zim_tile(zim, self.zim_info.as_ref(), w),
                None => container(
                    column![
                        image(eink::wikipedia_cover()).width(96).height(96),
                        paragraph(
                            "Choose a Kiwix ZIM file to make an archive the e-reader can open, or drop one onto the device.",
                            Face::Ui,
                            BODY,
                        ),
                    ]
                    .spacing(16)
                    .align_x(Alignment::Center),
                )
                .padding([18.0, SIDE + PAD])
                .center_x(Length::Fill)
                .into(),
            },
            Screen::Italic => mouse_area(
                container(
                    image(italic_picture(self.italic_picture))
                        .content_fit(iced::ContentFit::Contain)
                        .filter_method(image::FilterMethod::Linear)
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .padding(Padding {
                    top: 6.0,
                    bottom: 12.0,
                    left: SIDE,
                    right: SIDE,
                }),
            )
            .on_press(Message::ItalicPicture(1))
            .on_scroll(Message::Scroll)
            .interaction(mouse::Interaction::Pointer)
            .into(),
            Screen::Firmware if self.install.running() => column![
                heading(&self.firmware_name(), "onto the e-reader", w),
                progress_view(&self.install, None),
            ]
            .into(),
            Screen::Card if self.font_work.running() => {
                let what = self.archive.as_deref().map(file_name).unwrap_or_default();
                column![
                    heading(&what, "Its fallback font", w),
                    progress_view(&self.font_work, None),
                ]
                .into()
            }
            Screen::Card if self.copy.running() => {
                let what = self.archive.as_deref().map(file_name).unwrap_or_default();
                let to = self.drive.as_ref().map(drive_line).unwrap_or_default();
                column![
                    heading(&what, &format!("to {to}"), w),
                    progress_view(&self.copy, None),
                ]
                .into()
            }
            _ => Space::new().into(),
        }
    }

    fn bottom_view(&self) -> Element<'_, Message> {
        match self.screen {
            Screen::Convert => match &self.conversion {
                Job::Finished(Ok(s)) => note(&format!("Done. {s}")),
                Job::Finished(Err(e)) => note(&format!("It did not work: {e}")),
                _ => Space::new().into(),
            },
            Screen::Card => match (&self.copy, &self.font_work) {
                (_, Job::Finished(Ok(s))) => note(s),
                (_, Job::Finished(Err(e))) => note(e),
                (Job::Finished(Ok(s)), _) => note(s),
                (Job::Finished(Err(e)), _) => note(&format!("The copy did not work: {e}")),
                (Job::Idle, _) if self.drives.is_empty() => note(
                    "No card found. Put the card in a card reader, or connect your e-reader by USB, then look again.",
                ),
                _ => Space::new().into(),
            },
            Screen::Book => match self.book_note() {
                Some(s) => note(&s),
                None => Space::new().into(),
            },
            Screen::Firmware => match &self.install {
                Job::Finished(Ok(s)) => note(s),
                Job::Finished(Err(e)) => note(&format!("It did not work: {e}")),
                _ => note(
                    "The firmware goes into the reader's spare slot and is checked before the \
                     reader is set to start from it. Books, settings and the SD card are kept.",
                ),
            },
            Screen::Settings => note(&format!(
                "On the keyboard: the arrows move, Enter chooses, Esc goes back, Page Up and Page Down turn pages. {MODIFIER} with + and − changes the size, {MODIFIER} 0 fits it to the screen."
            )),
            _ => Space::new().into(),
        }
    }

    fn home_tile(&self, w: f32) -> Element<'_, Message> {
        let wide = w - 2.0 * SIDE;
        let zim_cover = || match &self.zim_info {
            Some(Ok(ZimInfo { cover: Some(c), .. })) => image_cover(c.clone()),
            _ => image_cover(eink::wikipedia_cover()),
        };
        let zim_title = || match &self.zim_info {
            Some(Ok(info)) => info.title.clone(),
            _ => self.zim.as_deref().map(file_name).unwrap_or_default(),
        };
        let (cover, title, lines, open): (
            Element<'_, Message>,
            String,
            Vec<Element<'_, Message>>,
            Screen,
        ) = if let Job::Running { step, .. } = &self.conversion {
            let steps = conversion_steps(self.converting_images, self.converting_sharper);
            (
                zim_cover(),
                zim_title(),
                job_lines(
                    &self.conversion,
                    steps.get(*step).copied().unwrap_or("Converting"),
                ),
                Screen::Convert,
            )
        } else if let Some((d, job)) = self.downloads.running().and_then(|d| Some((d, d.job()?))) {
            (
                image_cover(eink::wikipedia_cover()),
                d.book.title.clone(),
                job_lines(job, d.book.flavour_description()),
                Screen::Downloads,
            )
        } else if self.copy.running() {
            (
                icon_cover(&icons::BOOK),
                self.archive.as_deref().map(file_name).unwrap_or_default(),
                job_lines(&self.copy, "Copying to the card"),
                Screen::Card,
            )
        } else if let Some(a) = &self.archive {
            (
                icon_cover(&icons::BOOK),
                file_name(a),
                vec![small_line(match &self.copy {
                    Job::Finished(Ok(_)) => "Copied to the card",
                    _ => "Ready to copy to the card",
                })],
                Screen::Card,
            )
        } else if self.zim.is_some() {
            (
                zim_cover(),
                zim_title(),
                vec![small_line("Ready to convert")],
                Screen::Convert,
            )
        } else {
            (
                    image_cover(eink::wikipedia_cover()),
                    "Wikis, offline".into(),
                    vec![
                        paragraph(
                            "Download Wikipedia or another wiki from Kiwix, convert it here, then copy it to your e-reader's card.",
                            Face::Small,
                            SMALL,
                        )
                        .into(),
                    ],
                    Screen::Library,
                )
        };
        let words = column![paragraph(title, Face::UiBold, TITLE)]
            .extend(lines)
            .spacing(10)
            .width(Length::Fill);
        let tile = button(
            container(row![cover, words].spacing(16).align_y(Alignment::Center))
                .padding([0.0, 16.0])
                .center_y(Length::Fill),
        )
        .width(wide)
        .height(HOME_TOP - 24.0)
        .padding(0)
        .on_press(Message::Open(open))
        .style(|_, status| button::Style {
            background: match status {
                button::Status::Pressed => Some(Background::Color(LIGHT)),
                _ => None,
            },
            text_color: INK,
            border: Border {
                color: INK,
                width: 1.0,
                radius: RADIUS.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        });
        container(tile).padding([12.0, SIDE]).into()
    }

    // -----------------------------------------------------------------------
    // The whole

    pub fn view(&self) -> Element<'_, Message> {
        let spec = self.spec();
        let look = self.prefs.model.look(self.prefs.finish());
        let window = spec.window();
        let screen = spec.screen_in_window();
        let mut layers: Vec<Element<'_, Message>> = vec![
            canvas(Body {
                spec,
                look,
                hovered: self.hovered_key,
                pressed: self.shown_pressed(),
            })
            .width(window.width)
            .height(window.height)
            .into(),
            pin(
                mouse_area(Space::new().width(spec.body.width).height(spec.body.height))
                    .on_press(Message::Drag)
                    .on_right_press(Message::SystemMenu),
            )
            .x(margin())
            .y(margin())
            .into(),
            pin(opaque(self.screen_view()))
                .x(screen.x)
                .y(screen.y)
                .into(),
        ];
        // Handles on the edges and corners resize the window; macOS does
        // that itself at the edges of a borderless window.
        if !cfg!(target_os = "macos") {
            for (r, direction, cursor) in resize_handles(window) {
                layers.push(
                    pin(mouse_area(Space::new().width(r.width).height(r.height))
                        .on_press(Message::ResizeFrom(direction))
                        .interaction(cursor))
                    .x(r.x)
                    .y(r.y)
                    .into(),
                );
            }
        }
        for k in spec.keys {
            let hit = k.hit(spec);
            let area = mouse_area(Space::new().width(hit.width).height(hit.height))
                .on_enter(Message::KeyHover(Some(k.key)))
                .on_exit(Message::KeyHover(None))
                .on_press(Message::KeyDown(k.key))
                .on_release(Message::KeyUp(k.key))
                .interaction(mouse::Interaction::Pointer);
            layers.push(
                pin(tooltip(area, tip(k.key.tooltip()), k.tip_position()).gap(4))
                    .x(hit.x)
                    .y(hit.y)
                    .into(),
            );
        }
        layers.push(
            canvas(Glass {
                spec,
                look,
                flash: self.flash_colour(),
            })
            .width(window.width)
            .height(window.height)
            .into(),
        );
        container(stack(layers).width(window.width).height(window.height))
            .center(Length::Fill)
            .into()
    }

    fn screen_view(&self) -> Element<'_, Message> {
        let spec = self.spec();
        let (w, h) = (spec.screen.width, spec.screen.height);
        let page = self.page();
        let room = self.list_room(&page);
        let pages = eink::paginate(&page.rows, room);
        let current = pages
            .iter()
            .position(|p| p.contains(&self.focus))
            .unwrap_or(0);
        let list_w = w - 2.0 * SIDE;
        let selecting = self.popup.is_none();
        let mut list = column![];
        if let Some(range) = pages.get(current) {
            for i in range.clone() {
                list = list.push(eink::row_view(
                    &page.rows[i],
                    selecting && i == self.focus,
                    list_w,
                    Message::Focus(i),
                ));
            }
        }
        if let Some(n) = &page.note {
            list = list.push(container(paragraph(n.clone(), Face::Ui, BODY)).padding([12.0, PAD]));
        }
        let multi = pages.len() > 1;
        let mut list_area: Element<'_, Message> = container(list)
            .padding([0.0, SIDE])
            .width(w)
            .height(if multi {
                Length::Fixed(room)
            } else {
                Length::Shrink
            })
            .into();
        if multi {
            list_area = stack![
                list_area,
                pin(canvas(eink::ScrollBar {
                    pages: pages.len(),
                    page: current,
                    on_page: Message::ShowPage,
                })
                .width(16)
                .height(room - 8.0))
                .x(w - 20.0)
                .y(4.0),
            ]
            .into();
        }
        let list_area = mouse_area(list_area).on_scroll(Message::Scroll);

        let status = self.status();
        let mut content = column![eink::header(
            &page.title,
            page.subtitle.as_deref(),
            page.back.then_some(Message::Back),
            status.as_deref(),
            &self.clock,
            [Message::Sleep, Message::CloseRequested],
            w,
        )];
        if page.top > 0.0 {
            content = content.push(container(self.top_view()).width(w).height(page.top));
        }
        content = content.push(list_area);
        if page.bottom > 0.0 {
            content = content.push(container(self.bottom_view()).width(w).height(page.bottom));
        }
        content = content.push(space().height(Length::Fill));
        if let Some(xs) = spec.hints {
            content = content.push(eink::hints(
                xs,
                page.hints,
                [Key::Back, Key::Confirm, Key::Left, Key::Right].map(|k| Some(Message::Press(k))),
            ));
        }

        let mut layers: Vec<Element<'_, Message>> = vec![
            image(eink::grain(w as u32, h as u32))
                .width(w)
                .height(h)
                .filter_method(image::FilterMethod::Nearest)
                .into(),
            content.width(w).height(h).into(),
        ];
        if let Some(p) = self.popup {
            layers.push(opaque(
                mouse_area(Space::new().width(w).height(h)).on_press(Message::ClosePopup),
            ));
            layers.push(
                pin(self.popup_view(p, w - 2.0 * SIDE))
                    .x(SIDE - 2.0)
                    .y((h * 0.165).round())
                    .into(),
            );
        }
        container(stack(layers).width(w).height(h))
            .style(|_| container::Style {
                background: Some(Background::Color(PAPER)),
                text_color: Some(INK),
                ..container::Style::default()
            })
            .into()
    }

    fn popup_view(&self, p: Popup, width: f32) -> Element<'_, Message> {
        let inner = width - 2.0 * 16.0;
        let (title, words) = match p {
            Popup::Download(i) => match self.downloads.items.get(i) {
                Some(d) => (
                    fit(&d.book.title, Face::UiBold, TITLE, inner).into_owned(),
                    Some(d.line()),
                ),
                None => ("Download".into(), None),
            },
            p => {
                let (title, words) = self.popup_words(p);
                (title.to_string(), words)
            }
        };
        let mut content = column![label(title, Face::UiBold, TITLE)].spacing(8);
        if let Some(w) = words {
            content = content.push(paragraph(w, Face::Ui, BODY));
        }
        let rows = self.popup_rows(p);
        let mut list = column![];
        for (i, r) in rows.iter().enumerate() {
            list = list.push(eink::row_view(
                r,
                i == self.popup_focus,
                inner,
                Message::PopupFocus(i),
            ));
        }
        eink::popup(content.push(list).into(), width)
    }

    /// A popup's title, and what it says above its choices.
    fn popup_words(&self, p: Popup) -> (&'static str, Option<String>) {
        match p {
            Popup::Power => ("Power", None),
            Popup::TurnOffBusy => (
                "Still working",
                Some(self.status().unwrap_or_default() + ". Turning off now stops it."),
            ),
            Popup::StopConversion => (
                "Stop converting?",
                Some(
                    "The archive will not be finished. Converting again starts from the beginning."
                        .to_string(),
                ),
            ),
            Popup::ReplaceArchive => (
                "Replace the archive?",
                Some(format!(
                    "{} is already in {}. Converting replaces it.",
                    self.output_path()
                        .as_deref()
                        .map(file_name)
                        .unwrap_or_default(),
                    self.save_dir()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default(),
                )),
            ),
            Popup::LowSpace { needs, has } => (
                "Not enough space?",
                Some(format!(
                    "The archive may need up to about {}, and {} is free where it is saved. \
                     The estimate errs on the large side.",
                    size_text(needs),
                    size_text(has)
                )),
            ),
            Popup::InstallFirmware => (
                "Install the firmware?",
                Some(format!(
                    "{} goes into the reader's spare firmware slot, and the reader starts \
                     from it. Its books, settings and SD card are kept, and the firmware it \
                     has now stays in the other slot. Keep the cable plugged in until it is \
                     done.",
                    self.firmware_name()
                )),
            ),
            Popup::Choose(Choice::Firmware) => ("Firmware", None),
            Popup::Choose(Choice::Device) => ("Device", None),
            Popup::Choose(Choice::Case) => ("Case", None),
            Popup::Choose(Choice::Size) => ("Display size", None),
            Popup::Choose(Choice::Wiki) => ("Wiki", None),
            Popup::Choose(Choice::Pictures) => ("Pictures", None),
            Popup::Choose(Choice::Selection) => ("Selection", None),
            Popup::Choose(Choice::Articles) => ("Articles", None),
            Popup::Choose(Choice::Sort) => ("Sort by", None),
            Popup::Choose(Choice::Graphics) => (
                "Graphics",
                Some(
                    "How the window is drawn. A change takes effect when xwa-tool next starts."
                        .to_string(),
                ),
            ),
            Popup::Download(_) => ("Download", None),
        }
    }
}

impl App {
    /// The choices in a popup.
    pub fn popup_rows(&self, p: Popup) -> Vec<Row<Message>> {
        let pick = |title: String, chosen: bool, m: Message| {
            let row = Row::new(Kind::Item, title).on_press(m);
            if chosen { row.value(SELECTED) } else { row }
        };
        match p {
            Popup::Choose(Choice::Device) => Model::ALL
                .into_iter()
                .map(|m| pick(m.name().into(), m == self.prefs.model, Message::SetModel(m)))
                .collect(),
            Popup::Choose(Choice::Case) => Finish::ALL
                .into_iter()
                .map(|f| {
                    pick(
                        f.name().into(),
                        f == self.prefs.finish(),
                        Message::SetFinish(f),
                    )
                })
                .collect(),
            Popup::Choose(Choice::Wiki) => Wiki::ALL
                .into_iter()
                .map(|w| {
                    pick(w.name().into(), w == self.prefs.wiki, Message::SetWiki(w))
                        .subtitle(w.about())
                })
                .collect(),
            Popup::Choose(Choice::Pictures) => {
                let v = self.prefs.view;
                choices(v.pictures, |pictures| View { pictures, ..v })
            }
            Popup::Choose(Choice::Selection) => {
                let v = self.prefs.view;
                choices(v.selection, |selection| View { selection, ..v })
            }
            Popup::Choose(Choice::Articles) => {
                let v = self.prefs.view;
                choices(v.articles, |articles| View { articles, ..v })
            }
            Popup::Choose(Choice::Sort) => {
                let v = self.prefs.view;
                choices(v.sort, |sort| View { sort, ..v })
            }
            Popup::Choose(Choice::Firmware) => {
                let releases = match &self.releases {
                    Some(Ok(r)) => r.as_slice(),
                    _ => &[],
                };
                let chosen = self.chosen_release().map(|r| r.tag.clone());
                let mut rows: Vec<_> = releases
                    .iter()
                    .enumerate()
                    .take(8)
                    .map(|(i, r)| {
                        let pre = if r.prerelease { " · pre-release" } else { "" };
                        pick(
                            r.tag.clone(),
                            chosen.as_deref() == Some(r.tag.as_str())
                                && !matches!(self.firmware_choice, FirmwareChoice::File(_)),
                            Message::ChooseRelease(i),
                        )
                        .subtitle(format!("Released {}{pre}", r.date()))
                    })
                    .collect();
                if self.prefs.any_firmware {
                    rows.push(
                        pick(
                            "A file on this computer".into(),
                            matches!(self.firmware_choice, FirmwareChoice::File(_)),
                            Message::ChooseFirmwareFile,
                        )
                        .subtitle("Any firmware for the reader's chip"),
                    );
                }
                rows
            }
            Popup::Choose(Choice::Graphics) => Graphics::choices()
                .into_iter()
                .map(|g| {
                    pick(
                        g.name().into(),
                        g == self.prefs.graphics,
                        Message::SetGraphics(g),
                    )
                })
                .collect(),
            Popup::Choose(Choice::Size) => {
                let max = self.max_zoom();
                let mut rows: Vec<_> = ZOOMS
                    .into_iter()
                    .filter(|&z| z <= max + 1e-3)
                    .map(|z| {
                        let chosen = self.prefs.zoom.is_some() && (z - self.zoom).abs() < 1e-3;
                        pick(
                            format!("{:.0}%", z * 100.0),
                            chosen,
                            Message::SetZoom(Some(z)),
                        )
                    })
                    .collect();
                rows.push(
                    pick(
                        "Fit to the screen".into(),
                        self.prefs.zoom.is_none(),
                        Message::SetZoom(None),
                    )
                    .subtitle(format!("{:.0}% here", max.min(1.0) * 100.0)),
                );
                rows
            }
            Popup::Download(i) => self.download_actions(i),
            _ => popup_actions(p),
        }
    }

    /// What can be done with one of the downloads.
    fn download_actions(&self, i: usize) -> Vec<Row<Message>> {
        let Some(d) = self.downloads.items.get(i) else {
            return Vec::new();
        };
        let id = || d.id().to_string();
        let remove = |what: &str| {
            Row::new(Kind::Item, "Remove")
                .subtitle(what.to_string())
                .on_press(Message::RemoveDownload(id()))
        };
        let mut rows = match &d.state {
            Fetch::Running { .. } => vec![
                Row::new(Kind::Item, "Pause")
                    .subtitle("Carry on later from where it stopped")
                    .on_press(Message::PauseDownload(id())),
                remove("Stop, and delete what was downloaded"),
            ],
            Fetch::Waiting => vec![remove("Take it off the list")],
            Fetch::Paused { .. } => vec![
                Row::new(Kind::Item, "Resume")
                    .subtitle("Carry on from where it stopped")
                    .on_press(Message::ResumeDownload(id())),
                remove("Delete what was downloaded"),
            ],
            Fetch::Done(path) => vec![
                Row::new(Kind::Item, "Convert")
                    .subtitle(format!("for the {}", self.prefs.model.short_name()))
                    .on_press_maybe(
                        (!self.conversion.running())
                            .then(|| Message::ZimChosen(Some(path.clone()))),
                    ),
                Row::new(Kind::Item, "Remove from the list")
                    .subtitle("The file stays in its folder")
                    .on_press(Message::RemoveDownload(id())),
            ],
            Fetch::Failed(_) => vec![
                Row::new(Kind::Item, "Try again").on_press(Message::ResumeDownload(id())),
                remove("Delete what was downloaded"),
            ],
        };
        rows.push(Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup));
        rows
    }
}

/// Rows for a filter's or the sort's choices, the one in use marked.
fn choices<C: library::Choice>(current: C, view: impl Fn(C) -> View) -> Vec<Row<Message>> {
    C::ALL
        .iter()
        .map(|&c| {
            let row = Row::new(Kind::Item, c.name()).on_press(Message::SetView(view(c)));
            if c == current {
                row.value(SELECTED)
            } else {
                row
            }
        })
        .collect()
}

/// The rows above the books on the library screen.
pub const LIBRARY_ROWS: usize = 3;

/// The value shown beside the choice in use, as the firmware marks it.
pub const SELECTED: &str = "Selected";

/// Where the window can be grabbed to resize it: its edges and corners,
/// in the room round the case where there is some, else the case's rim.
fn resize_handles(
    window: iced::Size,
) -> [(iced::Rectangle, iced::window::Direction, mouse::Interaction); 8] {
    use iced::window::Direction::*;
    use mouse::Interaction::*;
    let t = if crate::device::shaped() {
        margin() + 4.0
    } else {
        6.0
    };
    let c = 22.0;
    let (w, h) = (window.width, window.height);
    let r = |x: f32, y: f32, width: f32, height: f32| iced::Rectangle {
        x,
        y,
        width,
        height,
    };
    [
        (r(c, 0.0, w - 2.0 * c, t), North, ResizingVertically),
        (r(c, h - t, w - 2.0 * c, t), South, ResizingVertically),
        (r(0.0, c, t, h - 2.0 * c), West, ResizingHorizontally),
        (r(w - t, c, t, h - 2.0 * c), East, ResizingHorizontally),
        (r(0.0, 0.0, c, c), NorthWest, ResizingDiagonallyDown),
        (r(w - c, h - c, c, c), SouthEast, ResizingDiagonallyDown),
        (r(w - c, 0.0, c, c), NorthEast, ResizingDiagonallyUp),
        (r(0.0, h - c, c, c), SouthWest, ResizingDiagonallyUp),
    ]
}

/// The choices in a popup that asks what to do.
fn popup_actions(p: Popup) -> Vec<Row<Message>> {
    match p {
        Popup::Power => vec![
            Row::new(Kind::Item, "Sleep")
                .subtitle("Minimise the window")
                .on_press(Message::Sleep),
            Row::new(Kind::Item, "Turn off")
                .subtitle("Quit xwa-tool")
                .on_press(Message::CloseRequested),
            Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup),
        ],
        Popup::TurnOffBusy => vec![
            Row::new(Kind::Item, "Keep working").on_press(Message::ClosePopup),
            Row::new(Kind::Item, "Turn off anyway").on_press(Message::TurnOff),
        ],
        Popup::ReplaceArchive => vec![
            Row::new(Kind::Item, "Replace it").on_press(Message::ConvertReplacing),
            Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup),
        ],
        Popup::LowSpace { .. } => vec![
            Row::new(Kind::Item, "Convert anyway").on_press(Message::ConvertAnyway),
            Row::new(Kind::Item, "Save somewhere else").on_press(Message::ChooseSaveDirFromPopup),
            Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup),
        ],
        Popup::InstallFirmware => vec![
            Row::new(Kind::Item, "Install").on_press(Message::InstallFirmware),
            Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup),
        ],
        Popup::StopConversion => vec![
            Row::new(Kind::Item, "Keep converting").on_press(Message::ClosePopup),
            Row::new(Kind::Item, "Stop").on_press(Message::CancelConversion),
        ],
        Popup::Choose(_) | Popup::Download(_) => Vec::new(),
    }
}

fn drive_line(d: &xwa_fetch::drives::Drive) -> String {
    let mut line = format!("{} · {} free", d.mount.display(), size_text(d.free));
    if !d.file_system.is_empty() {
        line += &format!(" · {}", d.file_system);
    }
    line
}

/// At most `max` characters, cut at a word.
fn clip_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) => format!("{}…", cut[..i].trim_end_matches([',', ';', ':', '.'])),
        None => format!("{cut}…"),
    }
}

fn search<'a>(field: Element<'a, Message>) -> Element<'a, Message> {
    container(field).padding([9.0, SIDE]).into()
}

/// What is being copied or downloaded, above its progress.
fn heading<'a>(what: &str, under: &str, w: f32) -> Element<'a, Message> {
    let room = w - 2.0 * (SIDE + PAD);
    container(
        column![
            label(
                fit(what, Face::UiBold, BODY, room).into_owned(),
                Face::UiBold,
                BODY
            ),
            label(
                fit(under, Face::Small, SMALL, room).into_owned(),
                Face::Small,
                SMALL
            ),
        ]
        .spacing(4),
    )
    .padding(Padding {
        top: 20.0,
        right: SIDE + PAD,
        bottom: 0.0,
        left: SIDE + PAD,
    })
    .height(COPY_TOP)
    .into()
}

fn note<'a>(s: &str) -> Element<'a, Message> {
    container(paragraph(s.to_string(), Face::Ui, BODY))
        .padding([10.0, SIDE + PAD])
        .into()
}

fn small_line<'a>(s: &str) -> Element<'a, Message> {
    label(s.to_string(), Face::Small, SMALL).into()
}

fn tip<'a>(s: &str) -> Element<'a, Message> {
    container(label(s.to_string(), Face::Small, SMALL))
        .padding([3.0, 8.0])
        .style(|_| container::Style {
            background: Some(Background::Color(PAPER)),
            text_color: Some(INK),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// A cover in a black frame, the size of the firmware's thumbnails.
fn framed<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .width(100)
        .height(132)
        .center_x(100)
        .center_y(132)
        .style(|_| container::Style {
            background: Some(Background::Color(PAPER)),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 2.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn image_cover<'a>(handle: image::Handle) -> Element<'a, Message> {
    framed(
        image(handle)
            .width(96)
            .height(96)
            .filter_method(image::FilterMethod::Nearest)
            .into(),
    )
}

fn icon_cover<'a>(icon: &'static icons::Icon) -> Element<'a, Message> {
    framed(eink::icon(icon, 64.0))
}

/// What a running job says on the home tile.
fn job_lines<'a>(job: &Job, what: &str) -> Vec<Element<'a, Message>> {
    let fraction = job.fraction().unwrap_or(0.0);
    let mut lines = vec![
        small_line(what),
        row![
            eink::progress(fraction),
            label(percent(fraction), Face::UiBold, SMALL)
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    ];
    if let Some(left) = job.time_left() {
        lines.push(small_line(&time_left(left)));
    }
    lines
}

/// The row that stops a job; once pressed, it says so and cannot be
/// pressed again while the work in hand finishes.
fn stop_row(job: &Job, title: &str, stop: Message) -> Row<Message> {
    if job.stopping() {
        Row::new(Kind::Item, "Stopping…").subtitle("Finishing the work in hand")
    } else {
        Row::new(Kind::Item, title).on_press(stop)
    }
}

fn progress_height(steps: Option<usize>) -> f32 {
    20.0 + steps.map_or(0.0, |n| n as f32 * 30.0 + 8.0) + 22.0 + 10.0 + 28.0 + 20.0
}

/// A running job: its steps (for conversions), a bar, how far along it is
/// and how long is left.
fn progress_view<'a>(job: &Job, steps: Option<&[&str]>) -> Element<'a, Message> {
    let Job::Running {
        label: what,
        fraction,
        step,
        ..
    } = job
    else {
        return Space::new().into();
    };
    let mut col = column![].spacing(0).padding([10.0, SIDE + PAD]);
    if let Some(steps) = steps {
        for (i, name) in steps.iter().enumerate() {
            let (face, colour) = match i.cmp(step) {
                std::cmp::Ordering::Less => (Face::Ui, INK),
                std::cmp::Ordering::Equal => (Face::UiBold, INK),
                std::cmp::Ordering::Greater => (Face::Ui, DARK),
            };
            col = col.push(
                row![
                    step_mark(i.cmp(step)),
                    label(name.to_string(), face, BODY).color(colour)
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .height(30),
            );
        }
        col = col.push(space().height(8));
    }
    col = col.push(
        label(
            fit(what, Face::Small, SMALL, 400.0).into_owned(),
            Face::Small,
            SMALL,
        )
        .height(22),
    );
    col = col.push(space().height(4));
    col = col.push(
        row![
            eink::progress(*fraction),
            label(percent(*fraction), Face::UiBold, BODY)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    );
    let left = if job.stopping() {
        String::new()
    } else {
        job.time_left()
            .map(time_left)
            .unwrap_or_else(|| "Working out how long it will take…".into())
    };
    col = col.push(label(left, Face::Small, SMALL).height(28));
    col.into()
}

/// Done, doing, or still to do.
fn step_mark<'a>(state: std::cmp::Ordering) -> Element<'a, Message> {
    let (fill, border) = match state {
        std::cmp::Ordering::Less => (INK, INK),
        std::cmp::Ordering::Equal => (LIGHT, INK),
        std::cmp::Ordering::Greater => (PAPER, DARK),
    };
    container(Space::new())
        .width(12)
        .height(12)
        .style(move |_| container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: border,
                width: if state == std::cmp::Ordering::Equal {
                    2.0
                } else {
                    1.0
                },
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The ZIM file at the top of the Convert screen.
fn zim_tile<'a>(
    zim: &Path,
    info: Option<&Result<ZimInfo, String>>,
    w: f32,
) -> Element<'a, Message> {
    let room = w - 2.0 * SIDE - 100.0 - 16.0 - PAD;
    let (cover, words): (Element<'a, Message>, Element<'a, Message>) = match info {
        None => (image_cover(eink::wikipedia_cover()), small_line("Reading…")),
        Some(Err(e)) => (
            image_cover(eink::wikipedia_cover()),
            column![
                label(
                    fit(&file_name(zim), Face::UiBold, BODY, room).into_owned(),
                    Face::UiBold,
                    BODY
                ),
                paragraph(format!("Could not read this file: {e}"), Face::Small, SMALL),
            ]
            .spacing(6)
            .into(),
        ),
        Some(Ok(info)) => {
            let cover = match &info.cover {
                Some(c) => image_cover(c.clone()),
                None => image_cover(eink::wikipedia_cover()),
            };
            let mut facts = Vec::new();
            for s in [&info.language, &info.flavour, &info.date] {
                if !s.is_empty() {
                    facts.push(s.clone());
                }
            }
            (
                cover,
                column![
                    paragraph(info.title.clone(), Face::UiBold, TITLE),
                    small_line(&facts.join(" · ")),
                    small_line(&format!(
                        "{} entries · {}",
                        group(u64::from(info.entries)),
                        size_text(info.size)
                    )),
                ]
                .spacing(6)
                .into(),
            )
        }
    };
    container(
        row![cover, container(words).width(Length::Fill)]
            .spacing(16)
            .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 9.0,
        right: SIDE,
        bottom: 9.0,
        left: SIDE + PAD,
    })
    .into()
}
