//! How the app behaves, driven through its messages; and, on request, a
//! picture of every screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use xwa_fetch::drives::Drive;
use xwa_fetch::{Book, Language};

use crate::app::{App, Choice, Event, Job, Message, Popup, Screen};
use crate::device::{Finish, Key, Model};
use crate::downloads::Fetch;
use crate::eink;
use crate::prefs::Prefs;

fn app(model: Model) -> App {
    let prefs = Prefs {
        model,
        ..Prefs::default()
    };
    let (mut app, _) = App::new(prefs, 1.0, None, crate::graphics::Graphics::Auto);
    app.prefs.refresh_flash = false;
    app.clock = "09:41".into();
    app
}

fn press(app: &mut App, key: Key) {
    let _ = app.update(Message::Press(key));
}

fn book(title: &str, flavour: &str, gb: f64, date: &str) -> Book {
    let name = format!(
        "wikipedia_en_{}",
        if title == "Wikipedia" {
            "all".to_string()
        } else {
            title.to_lowercase().replace(' ', "-")
        }
    );
    Book {
        id: format!("{name}_{flavour}"),
        name: name.clone(),
        title: title.into(),
        summary: format!(
            "{title} articles from Wikipedia, the free encyclopedia, for reading offline with Kiwix."
        ),
        language: "eng".into(),
        flavour: flavour.into(),
        category: "wikipedia".into(),
        tags: if flavour == "maxi" {
            "wikipedia;_pictures:yes".into()
        } else {
            "wikipedia;_pictures:no".into()
        },
        date: date.into(),
        article_count: (gb * 70_000.0) as u64,
        media_count: if flavour == "maxi" {
            (gb * 40_000.0) as u64
        } else {
            0
        },
        size: (gb * 1e9) as u64,
        url: format!("https://download.kiwix.org/zim/wikipedia/{name}_{flavour}_{date}.zim.meta4"),
    }
}

fn library() -> Vec<Book> {
    let mut books = vec![
        book("Wikipedia", "maxi", 111.4, "2026-10"),
        book("Wikipedia", "nopic", 47.9, "2026-10"),
        book("Wikipedia", "mini", 12.2, "2026-10"),
    ];
    for (title, gb) in [
        ("Astronomy", 1.4),
        ("Chemistry", 0.42),
        ("Climate change", 0.21),
        ("Computer science", 0.83),
        ("Geography", 2.6),
        ("History", 3.9),
        ("Mathematics", 0.65),
        ("Medicine", 2.2),
        ("Molecular biology", 0.36),
        ("Music", 4.1),
        ("Philosophy", 0.53),
        ("Physics", 0.94),
        ("Ray Charles", 0.006),
        ("Top 100", 0.31),
    ] {
        books.push(book(title, "maxi", gb, "2026-09"));
        books.push(book(title, "nopic", gb / 4.0, "2026-09"));
    }
    books
}

fn languages() -> Vec<Language> {
    [
        ("eng", "English"),
        ("fra", "French"),
        ("deu", "German"),
        ("spa", "Spanish"),
        ("swe", "Swedish"),
    ]
    .into_iter()
    .map(|(code, name)| Language {
        code: code.into(),
        name: name.into(),
        books: 100,
    })
    .collect()
}

fn with_library(app: &mut App) {
    let _ = app.update(Message::LanguagesLoaded(Ok(languages())));
    let _ = app.update(Message::BooksLoaded(Ok(library())));
}

fn running(fraction: f32, step: usize, label: &str) -> Job {
    Job::Running {
        label: label.into(),
        fraction,
        step,
        cancel: Arc::new(AtomicBool::new(false)),
        started: Instant::now() - Duration::from_secs(1500),
        rate: None,
        left: None,
    }
}

/// A list of downloads in every state: the first book of the library
/// downloading, two waiting, and one each paused, done and stopped.
fn queue(app: &mut App) {
    let books = library();
    let states = [
        (
            0,
            Fetch::Running {
                job: Job::Running {
                    label: "41.2 GB of 111.4 GB · 12 MB/s".into(),
                    fraction: 0.37,
                    step: 0,
                    cancel: Arc::new(AtomicBool::new(false)),
                    started: Instant::now() - Duration::from_secs(3400),
                    rate: Some(12e6),
                    left: Some(Duration::from_secs(5850)),
                },
                run: 1,
            },
        ),
        (5, Fetch::Waiting),
        (11, Fetch::Waiting),
        (3, Fetch::Paused { fraction: 0.4 }),
        (
            27,
            Fetch::Done(PathBuf::from(
                "/home/reader/Downloads/wikipedia_en_ray-charles_maxi_2026-09.zim",
            )),
        ),
        (14, Fetch::Failed("The mirrors stopped answering.".into())),
    ];
    for (i, state) in states {
        app.downloads.add(books[i].clone());
        app.downloads.items.last_mut().unwrap().state = state;
    }
}

/// The run of the download at `i`, which must be running.
fn run_of(app: &App, i: usize) -> u64 {
    match app.downloads.items[i].state {
        Fetch::Running { run, .. } => run,
        ref other => panic!("not running: {other:?}"),
    }
}

fn titles(app: &App) -> Vec<String> {
    app.page().rows.iter().map(|r| r.title.clone()).collect()
}

fn card() -> Drive {
    Drive {
        name: "XTEINK".into(),
        mount: PathBuf::from(if cfg!(windows) { "E:\\" } else { "/media/card" }),
        free: 29_700_000_000,
        total: 31_900_000_000,
        file_system: "exFAT".into(),
    }
}

#[test]
fn the_home_menu_opens_each_screen() {
    let mut app = app(Model::X4Pro);
    assert_eq!(app.screen, Screen::Home);
    for (i, screen) in [
        Screen::Library,
        Screen::Convert,
        Screen::Card,
        Screen::Settings,
    ]
    .into_iter()
    .enumerate()
    {
        let _ = app.update(Message::Focus(i));
        press(&mut app, Key::Confirm);
        assert_eq!(app.screen, screen);
        press(&mut app, Key::Back);
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.focus, i, "the selection is where it was");
    }
}

#[test]
fn the_italic_pictures_turn_like_pages_and_back_returns_to_converting() {
    let mut app = app(Model::X4);
    let _ = app.update(Message::Open(Screen::Convert));
    assert!(app.real_italic, "real italic is on unless turned off");
    let _ = app.update(Message::ToggleItalic);
    assert!(!app.real_italic);
    let _ = app.update(Message::Open(Screen::Italic));
    assert_eq!(app.italic_picture, 0);
    for (key, picture) in [
        (Key::Right, 1),
        (Key::Confirm, 2),
        (Key::Down, 0),
        (Key::Up, 2),
    ] {
        press(&mut app, key);
        assert_eq!(app.italic_picture, picture, "{key:?}");
    }
    press(&mut app, Key::Back);
    assert_eq!(app.screen, Screen::Convert);
    // It opens at the first picture again.
    let _ = app.update(Message::Open(Screen::Italic));
    assert_eq!(app.italic_picture, 0);
}

#[test]
fn the_selection_goes_round_the_list_and_skips_what_cannot_be_chosen() {
    let mut app = app(Model::X4);
    press(&mut app, Key::Left);
    assert_eq!(app.focus, 3, "up from the top goes to the bottom");
    press(&mut app, Key::Right);
    assert_eq!(app.focus, 0);

    let _ = app.update(Message::Open(Screen::Settings));
    let rows = app.page().rows;
    let version = rows.iter().position(|r| r.title == "Version").unwrap();
    for _ in 0..rows.len() * 2 {
        press(&mut app, Key::Right);
        assert_ne!(app.focus, version, "facts cannot be selected");
    }
}

#[test]
fn page_keys_turn_whole_pages() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    let page = app.page();
    let pages = eink::paginate(&page.rows, app.list_room(&page));
    assert!(pages.len() >= 3, "{} pages", pages.len());
    press(&mut app, Key::Down);
    assert_eq!(app.focus, pages[1].start);
    press(&mut app, Key::Up);
    press(&mut app, Key::Up);
    assert_eq!(
        app.focus,
        pages[pages.len() - 1].start,
        "back from the first page is the last"
    );
    // The wheel does the same, a notch at a time.
    let _ = app.update(Message::Scroll(iced::mouse::ScrollDelta::Lines {
        x: 0.0,
        y: -1.0,
    }));
    assert_eq!(app.focus, pages[0].start);
}

#[test]
fn rows_fill_but_never_overflow_the_screen() {
    for model in Model::ALL {
        let mut app = app(model);
        with_library(&mut app);
        for screen in [
            Screen::Home,
            Screen::Library,
            Screen::Languages,
            Screen::Convert,
            Screen::Card,
            Screen::Settings,
            Screen::Filter,
            Screen::Downloads,
        ] {
            let _ = app.update(Message::Open(screen));
            let page = app.page();
            let room = app.list_room(&page);
            assert!(room > 0.0, "{model:?} {screen:?}");
            for range in eink::paginate(&page.rows, room) {
                let used: f32 = page.rows[range].iter().map(eink::Row::height).sum();
                assert!(used <= room + 0.5, "{model:?} {screen:?}: {used} > {room}");
            }
        }
    }
}

#[test]
fn searching_narrows_the_list_and_enter_opens_the_first_match() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    let _ = app.update(Message::Search("chem".into()));
    let titles: Vec<String> = app.page().rows.iter().map(|r| r.title.clone()).collect();
    assert_eq!(
        titles,
        [
            "Wiki",
            "Language",
            "Filter and sort",
            "Chemistry",
            "Chemistry"
        ]
    );
    let _ = app.update(Message::Submit);
    assert_eq!(app.screen, Screen::Book);
    assert_eq!(app.book.as_ref().unwrap().title, "Chemistry");
}

#[test]
fn choosing_a_language_goes_back_to_the_list() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    let _ = app.update(Message::Focus(1));
    press(&mut app, Key::Confirm);
    assert_eq!(app.screen, Screen::Languages);
    let _ = app.update(Message::LanguageChosen(languages()[4].clone()));
    assert_eq!(app.screen, Screen::Library);
    assert_eq!(app.prefs.language.as_deref(), Some("swe"));
}

#[test]
fn the_library_lists_any_wiki_filtered_and_sorted() {
    use crate::library::{Pictures, Sort, Wiki};
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    // Wiki is the first row; choosing another reloads the list.
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, Some(Popup::Choose(Choice::Wiki)));
    let _ = app.update(Message::SetWiki(Wiki::Wikivoyage));
    assert_eq!(app.prefs.wiki, Wiki::Wikivoyage);
    assert!(app.books.is_empty(), "the old wiki's files are gone");
    let _ = app.update(Message::BooksLoaded(Ok(library())));
    // Filter and sort: without pictures, smallest first.
    let _ = app.update(Message::Open(Screen::Filter));
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, Some(Popup::Choose(Choice::Pictures)));
    press(&mut app, Key::Right);
    press(&mut app, Key::Right);
    press(&mut app, Key::Confirm);
    assert_eq!(app.prefs.view.pictures, Pictures::Without);
    let _ = app.update(Message::Choose(Choice::Sort));
    let rows = app.popup_rows(Popup::Choose(Choice::Sort));
    let smallest = rows
        .iter()
        .position(|r| r.title == "Smallest first")
        .unwrap();
    let _ = app.update(Message::PopupFocus(smallest));
    press(&mut app, Key::Confirm);
    assert_eq!(app.prefs.view.sort, Sort::Smallest);
    press(&mut app, Key::Back);
    let page = app.page();
    let books: Vec<&str> = page.rows[crate::screens::LIBRARY_ROWS..]
        .iter()
        .map(|r| r.value.as_deref().unwrap())
        .collect();
    assert_eq!(
        books.first(),
        Some(&"2 MB"),
        "the smallest without pictures first"
    );
    assert!(
        page.rows[2]
            .subtitle
            .as_deref()
            .unwrap()
            .contains("Without pictures")
    );
}

#[test]
fn the_scroll_bar_turns_to_the_page_asked_for() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    let page = app.page();
    let pages = eink::paginate(&page.rows, app.list_room(&page));
    let _ = app.update(Message::ShowPage(2));
    assert_eq!(app.focus, pages[2].start);
}

#[test]
fn downloads_wait_their_turn_and_can_be_paused_resumed_or_removed() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let books = library();
    // The first starts at once.
    let _ = app.update(Message::OpenBook(books[0].clone()));
    assert_eq!(titles(&app).last().unwrap(), "Download");
    let _ = app.update(Message::Download);
    let first = run_of(&app, 0);
    assert_eq!(titles(&app), ["Pause", "Remove", "Downloads"]);
    // Another waits its turn.
    let _ = app.update(Message::OpenBook(books[5].clone()));
    let _ = app.update(Message::Download);
    assert!(matches!(app.downloads.items[1].state, Fetch::Waiting));
    assert!(titles(&app).ends_with(&["Downloads".into(), "Remove".into()]));
    // The library shows how each is getting on.
    let _ = app.update(Message::Open(Screen::Library));
    let values: Vec<Option<String>> = app.page().rows.iter().map(|r| r.value.clone()).collect();
    assert!(values.contains(&Some("0%".into())));
    assert!(values.contains(&Some("Waiting".into())));
    // Home has a way back to them.
    let _ = app.update(Message::Open(Screen::Home));
    let home = app.page().rows;
    assert_eq!(home[1].title, "Downloads");
    assert_eq!(
        home[1].subtitle.as_deref(),
        Some("Wikipedia 0% · 1 waiting")
    );
    let _ = app.update(Message::Focus(1));
    press(&mut app, Key::Confirm);
    assert_eq!(app.screen, Screen::Downloads);

    // Choosing the running one asks what to do; pausing it lets the next start.
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, Some(Popup::Download(0)));
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, None);
    assert!(
        matches!(app.downloads.items[0].state, Fetch::Running { .. }),
        "until its worker stops"
    );
    let _ = app.update(Message::Downloading(
        first,
        Event::Done(Err("cancelled".into())),
    ));
    assert!(matches!(app.downloads.items[0].state, Fetch::Paused { .. }));
    let second = run_of(&app, 1);
    // A late message from a stopped worker changes nothing.
    let _ = app.update(Message::Downloading(first, Event::Done(Err("late".into()))));
    assert_eq!(run_of(&app, 1), second);

    // Resumed, the first waits for the second.
    let _ = app.update(Message::ResumeDownload(books[0].id.clone()));
    assert!(matches!(app.downloads.items[0].state, Fetch::Waiting));
    // The second finishes with nobody watching: the first carries on, and
    // the file waits on the Convert screen.
    let _ = app.update(Message::Open(Screen::Home));
    let zim = PathBuf::from("/nowhere/chemistry.zim");
    let _ = app.update(Message::Downloading(second, Event::Done(Ok(zim.clone()))));
    assert!(matches!(app.downloads.items[1].state, Fetch::Done(_)));
    let third = run_of(&app, 0);
    assert_eq!(app.zim.as_ref(), Some(&zim));
    assert_eq!(app.screen, Screen::Home);

    // Taken off the list, it stops and is gone.
    let _ = app.update(Message::RemoveDownload(books[0].id.clone()));
    assert_eq!(app.downloads.items.len(), 1);
    assert!(!app.downloads.is_running());
    let _ = app.update(Message::Downloading(
        third,
        Event::Done(Err("cancelled".into())),
    ));
    assert_eq!(app.downloads.items.len(), 1);
}

#[test]
fn watching_a_download_finish_goes_on_to_converting_it() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::OpenBook(library()[5].clone()));
    let _ = app.update(Message::Download);
    let run = run_of(&app, 0);
    let zim = PathBuf::from("/nowhere/chemistry.zim");
    let _ = app.update(Message::Downloading(run, Event::Done(Ok(zim.clone()))));
    assert_eq!(app.screen, Screen::Convert);
    assert_eq!(app.zim, Some(zim));
    // Back on the book, it offers converting what was downloaded.
    press(&mut app, Key::Back);
    assert_eq!(app.screen, Screen::Book);
    assert!(titles(&app).contains(&"Convert".to_string()));
}

#[test]
fn a_stopped_download_can_be_tried_again() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::OpenBook(library()[5].clone()));
    let _ = app.update(Message::Download);
    let run = run_of(&app, 0);
    let _ = app.update(Message::Downloading(
        run,
        Event::Done(Err("No route to host".into())),
    ));
    assert!(matches!(app.downloads.items[0].state, Fetch::Failed(_)));
    assert!(titles(&app).contains(&"Try again".to_string()));
    let _ = app.update(Message::ResumeDownload(library()[5].id.clone()));
    assert!(run_of(&app, 0) > run);
}

#[test]
fn power_asks_what_to_do_and_back_puts_it_away() {
    let mut app = app(Model::X4Pro);
    press(&mut app, Key::Power);
    assert_eq!(app.popup, Some(Popup::Power));
    press(&mut app, Key::Right);
    assert_eq!(app.popup_focus, 1);
    press(&mut app, Key::Back);
    assert_eq!(app.popup, None);
    assert_eq!(
        app.screen,
        Screen::Home,
        "Back closed the popup, nothing more"
    );
}

#[test]
fn turning_off_while_working_asks_first() {
    let mut app = app(Model::X4Pro);
    app.conversion = running(0.4, 1, "");
    let _ = app.update(Message::CloseRequested);
    assert_eq!(app.popup, Some(Popup::TurnOffBusy));
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, None, "the first choice keeps working");
    assert!(app.conversion.running());
}

#[test]
fn clicking_a_key_acts_when_it_is_let_go_over_it() {
    let mut app = app(Model::X4);
    let _ = app.update(Message::KeyDown(Key::Right));
    assert_eq!(app.shown_pressed(), Some(Key::Right));
    assert_eq!(app.focus, 0, "nothing happens until the key is let go");
    let _ = app.update(Message::KeyUp(Key::Right));
    assert_eq!(app.focus, 1);
    // Sliding off a key before letting go cancels it.
    let _ = app.update(Message::KeyDown(Key::Right));
    let _ = app.update(Message::KeyHover(None));
    let _ = app.update(Message::KeyUp(Key::Right));
    assert_eq!(app.focus, 1);
}

#[test]
fn dropped_files_go_where_their_kind_belongs() {
    let mut app = app(Model::X4Pro);
    let _ = app.update(Message::Opened(PathBuf::from("/nowhere/wiki.XWA")));
    assert_eq!(app.screen, Screen::Card);
    assert_eq!(app.archive, Some(PathBuf::from("/nowhere/wiki.XWA")));
    // A ZIM file, even one that cannot be read, opens the Convert screen.
    let _ = app.update(Message::Opened(PathBuf::from("/nowhere/wiki.zim")));
    assert_eq!(app.screen, Screen::Convert);
    assert_eq!(app.zim, Some(PathBuf::from("/nowhere/wiki.zim")));
}

#[test]
fn a_finished_conversion_can_be_done_again_with_other_settings() {
    let mut app = app(Model::X4Pro);
    let _ = app.update(Message::Opened(PathBuf::from("/nowhere/wiki.zim")));
    app.conversion = Job::Finished(Ok("done".into()));
    let titles: Vec<String> = app.page().rows.iter().map(|r| r.title.clone()).collect();
    assert_eq!(
        titles,
        ["Copy to SD card", "Convert again", "Convert another file"]
    );
    let _ = app.update(Message::ConvertAgain);
    assert!(app.page().rows.iter().any(|r| r.title == "Made for"));
}

/// A one-article archive at `path` whose text uses π and α: with a FONT
/// holding `glyphs` when given, and its CHARS section when `chars`.
fn small_archive(path: &Path, glyphs: Option<&str>, chars: bool) {
    use xwa_format::content::{Article, Block, Inline};
    use xwa_format::font::{Font, FontChars, FontStyle, Glyph};
    use xwa_format::writer::{ArchiveWriter, WriterOptions};
    let mut w =
        ArchiveWriter::create(path, WriterOptions::new(xwa_format::Profile::Compact)).unwrap();
    let article = Article {
        title: "Pi".into(),
        body: vec![Block::Para(vec![Inline::Text("π and α".into())])],
        ..Default::default()
    };
    w.add_article(0, &article, 0).unwrap();
    if let Some(glyphs) = glyphs {
        let font = Font {
            upem: 1000,
            ascender: 1069,
            descender: -293,
            line_gap: 0,
            glyphs: glyphs
                .chars()
                .map(|c| {
                    (
                        c,
                        Glyph {
                            advance: 500,
                            outline: Default::default(),
                        },
                    )
                })
                .collect(),
        };
        w.set_font(FontStyle::Regular, font.encode().unwrap());
    }
    if chars {
        let text = w.text_chars();
        let none = xwa_format::normalize::CharSet::default();
        w.set_font_chars(&FontChars::new(true, [&text, &none, &none, &none]));
    }
    w.finish().unwrap();
}

#[test]
fn an_archive_missing_glyphs_offers_to_complete_its_font() {
    use crate::app::{ArchiveFont, archive_font};
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    small_archive(&path("missing.xwa"), Some("π"), true);
    small_archive(&path("complete.xwa"), Some("πα"), true);
    small_archive(&path("older.xwa"), Some("π"), false);
    small_archive(&path("plain.xwa"), None, false);
    assert!(matches!(
        archive_font(&path("missing.xwa")),
        ArchiveFont::Missing { glyphs: 1, .. }
    ));
    assert!(matches!(
        archive_font(&path("complete.xwa")),
        ArchiveFont::Complete
    ));
    assert!(matches!(
        archive_font(&path("older.xwa")),
        ArchiveFont::Unchecked
    ));
    assert!(matches!(
        archive_font(&path("plain.xwa")),
        ArchiveFont::None
    ));

    let mut app = app(Model::X4);
    let titles =
        |app: &App| -> Vec<String> { app.page().rows.into_iter().map(|r| r.title).collect() };
    for (name, row) in [
        ("missing.xwa", Some("Complete its fallback font")),
        ("older.xwa", Some("Check its fallback font")),
        ("complete.xwa", None),
        ("plain.xwa", None),
    ] {
        let _ = app.update(Message::Opened(path(name)));
        assert_eq!(app.screen, Screen::Card);
        let state = archive_font(&path(name));
        let _ = app.update(Message::ArchiveFontRead(path(name), state));
        let rows = titles(&app);
        assert_eq!(
            rows.get(1).map(String::as_str) == row,
            row.is_some(),
            "{name}: {rows:?}"
        );
        assert!(!rows.iter().any(|r| r.contains("fallback font")) || row.is_some());
    }
    // A late answer about another archive changes nothing.
    let _ = app.update(Message::ArchiveFontRead(
        path("missing.xwa"),
        archive_font(&path("missing.xwa")),
    ));
    assert!(!titles(&app).iter().any(|r| r.contains("fallback font")));
}

#[test]
fn settings_offer_the_fonts_for_converting_offline_until_all_are_kept() {
    use crate::app::FontsKept;
    let mut app = app(Model::X4);
    let _ = app.update(Message::Open(Screen::Settings));
    let row = |app: &App| {
        app.page()
            .rows
            .into_iter()
            .find(|r| r.title == "Fonts for converting offline")
            .expect("the fonts row")
    };
    app.fonts_kept = Some(FontsKept {
        kept: 12,
        total: 239,
        missing_bytes: 110_000_000,
    });
    let r = row(&app);
    assert_eq!(
        r.subtitle.as_deref(),
        Some("12 of 239 kept · download the rest (110 MB)")
    );
    assert!(r.on_press.is_some());
    app.fonts_kept = Some(FontsKept {
        kept: 239,
        total: 239,
        missing_bytes: 0,
    });
    let r = row(&app);
    assert_eq!(r.subtitle.as_deref(), Some("All 239 Noto font files kept"));
    assert!(r.on_press.is_none(), "nothing left to download");
    // While downloading, the row shows how far and stops it when chosen.
    app.font_download = running(0.25, 0, "27 MB of 110 MB");
    let r = row(&app);
    assert_eq!(r.subtitle.as_deref(), Some("27 MB of 110 MB"));
    assert_eq!(r.value.as_deref(), Some("25%"));
    let _ = app.update(Message::DownloadFonts);
    assert!(app.font_download.stopping());
}

#[test]
fn stopping_a_conversion_asks_first() {
    let mut app = app(Model::X4Pro);
    let _ = app.update(Message::Opened(PathBuf::from("/nowhere/wiki.zim")));
    app.conversion = running(0.3, 1, "");
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, Some(Popup::StopConversion));
    assert!(
        app.conversion.running(),
        "nothing stops until the second choice"
    );
    press(&mut app, Key::Right);
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, None);
    assert!(matches!(app.conversion, Job::Running { ref label, .. } if label == "Stopping…"));
    // While it stops, the row says so and asks nothing more.
    assert!(app.conversion.stopping());
    let rows = app.page().rows;
    assert_eq!(rows[0].title, "Stopping…");
    assert!(rows[0].on_press.is_none());
    let _ = app.update(Message::AskStopConversion);
    assert_eq!(app.popup, None);
}

#[test]
fn the_device_and_case_can_be_changed() {
    let mut app = app(Model::X4Pro);
    assert_eq!(app.prefs.finish(), Finish::Black);
    let _ = app.update(Message::Open(Screen::Settings));
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, Some(Popup::Choose(Choice::Device)));
    assert_eq!(app.popup_focus, 0, "the list opens on the model in use");
    press(&mut app, Key::Right);
    press(&mut app, Key::Confirm);
    assert_eq!(app.popup, None);
    assert_eq!(app.prefs.model, Model::X4);
    assert_eq!(
        app.prefs.finish(),
        Finish::White,
        "each model has its usual case"
    );
    let _ = app.update(Message::SetFinish(Finish::Black));
    assert_eq!(app.prefs.finish(), Finish::Black);
}

#[test]
fn display_sizes_offered_fit_the_monitor() {
    let mut app = app(Model::X4Pro);
    let _ = app.update(Message::Choose(Choice::Size));
    let rows = app.popup_rows(Popup::Choose(Choice::Size));
    // No monitor known: every step, then fitting.
    assert_eq!(rows.len(), crate::app::ZOOMS.len() + 1);
    assert_eq!(rows.last().unwrap().title, "Fit to the screen");
    assert_eq!(app.popup_focus, rows.len() - 1, "fitting is what is in use");
    let _ = app.update(Message::SetZoom(Some(0.8)));
    assert_eq!(app.zoom, 0.8);
    assert_eq!(app.prefs.zoom, Some(0.8));
    let _ = app.update(Message::Zoom(0));
    assert_eq!(app.prefs.zoom, None);
}

#[test]
fn resizing_the_window_scales_the_device_then_fits_round_it() {
    use iced::Size;
    use iced::window::Direction;
    let mut app = app(Model::X4Pro);
    app.shown = true;
    let window = app.spec().window();
    // The right edge dragged out half as far again: the device follows it.
    let _ = app.update(Message::ResizeFrom(Direction::East));
    let _ = app.update(Message::Resized(Size::new(
        window.width * 1.5,
        window.height,
    )));
    assert!((app.zoom - 1.5).abs() < 1e-3, "{}", app.zoom);
    assert!(app.settle_at.is_some());
    // Once it settles, the size is remembered.
    app.settle_at = Some(std::time::Instant::now());
    let _ = app.update(Message::Settle);
    assert_eq!(app.prefs.zoom, Some(app.zoom));
    assert!(app.settle_at.is_none());
    // The window's own size at this zoom is not taken for a drag.
    let zoom = app.zoom;
    let _ = app.update(Message::Resized(window));
    assert_eq!(app.zoom, zoom);
    // A late report of an older size, just after the app resized the
    // window itself, is not taken for the user's.
    app.own_resize_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    let _ = app.update(Message::Resized(Size::new(
        window.width * 0.2,
        window.height * 0.2,
    )));
    assert_eq!(app.zoom, zoom);
    app.own_resize_until = None;
    // A resize from elsewhere (the system's) fits the device in it, and
    // never below the smallest size.
    let _ = app.update(Message::Resized(Size::new(
        window.width * 0.5,
        window.height,
    )));
    assert!((app.zoom - zoom * 0.5).abs() < 1e-3);
    let _ = app.update(Message::Resized(Size::new(10.0, 10.0)));
    assert_eq!(app.zoom, crate::app::MIN_ZOOM);
}

#[test]
fn a_minimised_window_keeps_the_device_size() {
    let mut app = app(Model::X4Pro);
    app.shown = true;
    let zoom = app.zoom;
    // Windows reports a minimised window as 0 × 0.
    let _ = app.update(Message::Resized(iced::Size::new(0.0, 0.0)));
    assert_eq!(app.zoom, zoom);
    assert!(app.settle_at.is_none(), "nothing to settle");
    // Minimised from the taskbar it can report the small size Windows
    // keeps for minimised windows; once the system says the window is
    // minimised, that size is not taken for the device's.
    let small = iced::Size::new(160.0, 28.0);
    let _ = app.update(Message::ResizedUnlessMinimized(small, true));
    assert_eq!(app.zoom, zoom);
    assert!(app.settle_at.is_none());
    // Not minimised, the same size is the user's, and the device fits it.
    let _ = app.update(Message::ResizedUnlessMinimized(small, false));
    assert_eq!(app.zoom, crate::app::MIN_ZOOM);
}

#[test]
fn a_size_the_system_gives_just_after_the_apps_own_is_asked_again() {
    let mut app = app(Model::X4Pro);
    app.shown = true;
    app.window = Some(iced::window::Id::unique());
    let zoom = app.zoom;
    let window = app.spec().window();
    // KWin, after a drag, answers the app's fitted size with the drag's.
    app.own_resize_until = Some(Instant::now() + Duration::from_secs(5));
    let stale = iced::Size::new(window.width, window.height * 0.75);
    // With a window, a size the user is not dragging to first asks whether
    // the window is minimised, and changes nothing yet.
    assert!(app.update(Message::Resized(stale)).units() > 0);
    assert_eq!(app.zoom, zoom);
    for asked in [true, true, false] {
        let task = app.update(Message::ResizedUnlessMinimized(stale, false));
        assert_eq!(task.units() > 0, asked, "asked again twice at most");
        assert_eq!(app.zoom, zoom, "the device keeps its size");
    }
}

#[test]
fn a_second_file_dialog_waits_for_the_first() {
    let mut app = app(Model::X4Pro);
    assert!(app.update(Message::ChooseZim).units() > 0);
    assert_eq!(
        app.update(Message::ChooseArchive).units(),
        0,
        "one dialog at a time"
    );
    let _ = app.update(Message::DialogClosed(Box::new(Message::ZimChosen(None))));
    assert!(app.update(Message::ChooseArchive).units() > 0);
}

#[test]
fn the_window_opens_at_the_size_that_fits_the_monitor_it_was_on() {
    use crate::app::{FIRST_ZOOM, start_zoom};
    let mut prefs = Prefs::default();
    assert_eq!(
        start_zoom(&prefs),
        FIRST_ZOOM,
        "small until the monitor is known"
    );
    // On a 1500 × 1000 desktop the X4 Pro fits at 90%, with or without the
    // room round its case for keys (none on Windows, drawn rectangular).
    prefs.monitor = Some((1500.0, 1000.0));
    assert_eq!(start_zoom(&prefs), 0.9);
    prefs.zoom = Some(1.6);
    assert!(start_zoom(&prefs) < 1.0, "never more than fits");
    prefs.zoom = Some(0.6);
    assert_eq!(start_zoom(&prefs), 0.6);
}

// ---------------------------------------------------------------------------
// Pictures

/// Draws every screen of every model into the folder named by
/// `XWA_GUI_SCREENS`, for looking at. `XWA_GUI_ZIM` names a real ZIM file
/// to show; `XWA_GUI_ONLY` limits it to names containing that text. Run with
/// `XWA_GUI_SCREENS=target/screens cargo test -p xwa-gui -- --ignored screens`.
#[test]
#[ignore = "draws pictures; run on request"]
fn screens() {
    let Some(dir) = std::env::var_os("XWA_GUI_SCREENS").map(PathBuf::from) else {
        return;
    };
    let zim = std::env::var_os("XWA_GUI_ZIM").map(PathBuf::from);
    for model in Model::ALL {
        let tag = model.key();
        for finish in Finish::ALL {
            let mut a = app(model);
            a.prefs.finish = Some(finish);
            snap(&a, &dir, &format!("{tag}-{}-home", finish.key()));
        }

        let mut a = app(model);
        with_library(&mut a);
        let _ = a.update(Message::Open(Screen::Library));
        let _ = a.update(Message::Focus(2));
        snap(&a, &dir, &format!("{tag}-library"));
        let _ = a.update(Message::Choose(Choice::Wiki));
        snap(&a, &dir, &format!("{tag}-wikis"));
        let _ = a.update(Message::ClosePopup);
        let _ = a.update(Message::Open(Screen::Filter));
        snap(&a, &dir, &format!("{tag}-filter"));
        let _ = a.update(Message::Back);
        let _ = a.update(Message::OpenBook(library()[0].clone()));
        snap(&a, &dir, &format!("{tag}-book"));
        queue(&mut a);
        snap(&a, &dir, &format!("{tag}-downloading"));
        let _ = a.update(Message::Back);
        snap(&a, &dir, &format!("{tag}-library-downloading"));
        let _ = a.update(Message::Open(Screen::Downloads));
        snap(&a, &dir, &format!("{tag}-downloads"));
        let _ = a.update(Message::DownloadMenu(3));
        snap(&a, &dir, &format!("{tag}-download-menu"));
        let _ = a.update(Message::ClosePopup);
        let _ = a.update(Message::Open(Screen::Home));
        snap(&a, &dir, &format!("{tag}-home-downloading"));

        let mut a = app(model);
        let _ = a.update(Message::Open(Screen::Convert));
        snap(&a, &dir, &format!("{tag}-convert-empty"));
        if let Some(zim) = &zim {
            a.zim = Some(zim.clone());
            a.zim_info = Some(crate::app::describe(zim));
            a.zim_shares = xwa_convert::zim_shares(zim).ok();
            a.refresh_save_free();
            a.focus = 2;
            snap(&a, &dir, &format!("{tag}-convert"));
            // The Save to row's tip, the pointer on its title.
            let screen = a.spec().screen;
            snap_pointing(
                &a,
                &dir,
                &format!("{tag}-convert-tip"),
                Some(iced::Point::new(screen.x + 80.0, screen.y + 312.0)),
            );
            a.conversion = running(0.46, 1, "18,422 of 41,204");
            a.converting_images = true;
            snap(&a, &dir, &format!("{tag}-converting"));
            let _ = a.update(Message::Open(Screen::Home));
            snap(&a, &dir, &format!("{tag}-home-converting"));
            a.conversion = Job::Finished(Ok(format!(
                "41,204 articles and 31,833 pictures in wikipedia_en_chemistry_maxi_2026-10.xwa (412 MB), made for the {}.",
                model.name()
            )));
            a.archive = Some(zim.with_extension("xwa"));
            let _ = a.update(Message::Open(Screen::Convert));
            snap(&a, &dir, &format!("{tag}-converted"));
        }

        let mut a = app(model);
        let _ = a.update(Message::Open(Screen::Convert));
        let _ = a.update(Message::Open(Screen::Italic));
        for i in 1..=crate::screens::ITALIC_PICTURES.len() {
            snap(&a, &dir, &format!("{tag}-italic-{i}"));
            let _ = a.update(Message::ItalicPicture(1));
        }

        let mut a = app(model);
        a.archive = Some(PathBuf::from("wikipedia_en_chemistry_maxi_2026-10.xwa"));
        let _ = a.update(Message::Open(Screen::Card));
        a.drives = vec![card()];
        a.drive = a.drives.first().cloned();
        snap(&a, &dir, &format!("{tag}-card"));
        let none = xwa_format::normalize::CharSet::default();
        let chars = xwa_format::font::FontChars::new(true, [&none, &none, &none, &none]);
        a.archive_font = crate::app::ArchiveFont::Missing {
            glyphs: 161_586,
            chars: std::sync::Arc::new(chars),
        };
        snap(&a, &dir, &format!("{tag}-card-font"));
        a.font_work = running(0.4, 0, "Fonts: 24 of 61");
        snap(&a, &dir, &format!("{tag}-card-font-working"));
        a.font_work = Job::Idle;
        a.archive_font = crate::app::ArchiveFont::Unknown;
        a.copy = running(0.62, 0, "255 MB of 412 MB");
        snap(&a, &dir, &format!("{tag}-copying"));

        let mut a = app(model);
        let _ = a.update(Message::Open(Screen::Settings));
        a.fonts_kept = Some(crate::app::FontsKept {
            kept: 12,
            total: 239,
            missing_bytes: 110_000_000,
        });
        snap(&a, &dir, &format!("{tag}-settings"));
        press(&mut a, Key::Power);
        snap(&a, &dir, &format!("{tag}-power"));
    }
}

fn snap(app: &App, dir: &Path, name: &str) {
    snap_pointing(app, dir, name, None);
}

/// `snap` with the pointer resting at `at`, to show a tip.
fn snap_pointing(app: &App, dir: &Path, name: &str, at: Option<iced::Point>) {
    // `XWA_GUI_ONLY` draws just the screens whose names contain it.
    if let Some(only) = std::env::var_os("XWA_GUI_ONLY")
        && !name.contains(only.to_string_lossy().as_ref())
    {
        return;
    }
    let settings = iced::Settings {
        fonts: vec![
            eink::UBUNTU_REGULAR.into(),
            eink::UBUNTU_BOLD.into(),
            eink::NOTO_SANS.into(),
        ],
        default_font: eink::UI,
        ..iced::Settings::default()
    };
    let mut sim = iced_test::Simulator::with_size(settings, app.spec().window(), app.view());
    if let Some(at) = at {
        sim.point_at(at);
        let _ = sim.simulate([iced::Event::Mouse(iced::mouse::Event::CursorMoved {
            position: at,
        })]);
    }
    let snapshot = sim.snapshot(&crate::eink_theme()).expect("drawn");
    // The snapshot is written only where none is; clear the old one first.
    for renderer in ["tiny-skia", "wgpu"] {
        let _ = std::fs::remove_file(dir.join(format!("{name}-{renderer}.png")));
    }
    std::fs::create_dir_all(dir).unwrap();
    assert!(
        snapshot
            .matches_image(dir.join(format!("{name}.png")))
            .unwrap()
    );
}

#[test]
fn the_archive_is_saved_where_chosen_and_converting_asks_first_when_it_may_not_fit() {
    let zims = tempfile::tempdir().unwrap();
    let mut app = app(Model::X4);
    let zim = zims.path().join("wiki.zim");
    let _ = app.update(Message::Opened(zim.clone()));
    let row = |app: &App| {
        app.page()
            .rows
            .into_iter()
            .find(|r| r.title == "Save to")
            .expect("the row")
    };
    // At first, beside the ZIM, under its name.
    assert_eq!(app.save_dir().as_deref(), Some(zims.path()));
    assert_eq!(app.output_path(), Some(zims.path().join("wiki.xwa")));
    let r = row(&app);
    assert!(r.keep_end, "a long path keeps its end");
    // With the ZIM sampled: the size goes in the tip, and the free space in
    // the subtitle while it is enough.
    app.zim_shares = Some(xwa_convert::ZimShares {
        text_bytes: 2_000_000_000,
        media_bytes: 10_000_000_000,
    });
    app.save_free = Some((zims.path().to_path_buf(), 100_000_000_000));
    let r = row(&app);
    assert!(
        r.subtitle.as_deref().unwrap().ends_with("100.0 GB free"),
        "{:?}",
        r.subtitle
    );
    assert!(
        r.tip.as_deref().unwrap().contains("up to about 11."),
        "{:?}",
        r.tip
    );
    app.save_free = Some((zims.path().to_path_buf(), 5_000_000_000));
    let r = row(&app);
    assert!(
        r.subtitle
            .as_deref()
            .unwrap()
            .starts_with("Needs up to about 11.")
    );
    // Converting over an archive of the same name asks first; then, with
    // more to write than the drive can have free, asks again.
    std::fs::write(zims.path().join("wiki.xwa"), b"old").unwrap();
    app.zim_shares = Some(xwa_convert::ZimShares {
        text_bytes: 0,
        media_bytes: u64::MAX / 4,
    });
    let _ = app.update(Message::Convert);
    assert_eq!(app.popup, Some(Popup::ReplaceArchive));
    assert!(!app.conversion.running());
    let _ = app.update(Message::ConvertReplacing);
    assert!(
        matches!(app.popup, Some(Popup::LowSpace { .. })),
        "{:?}",
        app.popup
    );
    assert!(!app.conversion.running());
    let _ = app.update(Message::ConvertAnyway);
    assert_eq!(app.popup, None);
    assert!(app.conversion.running() || matches!(app.conversion, Job::Finished(_)));
    assert_eq!(app.output, Some(zims.path().join("wiki.xwa")));
    // A folder chosen is kept for next time.
    let saves = tempfile::tempdir().unwrap();
    app.conversion = Job::Idle;
    let _ = app.update(Message::SaveDirChosen(Some(saves.path().to_path_buf())));
    assert_eq!(app.prefs.save_dir.as_deref(), Some(saves.path()));
    assert_eq!(app.output_path(), Some(saves.path().join("wiki.xwa")));
    assert!(
        app.save_free
            .as_ref()
            .is_some_and(|(d, _)| d == saves.path())
    );
}

#[test]
fn settings_and_choices_a_newcomer_may_not_know_have_tips() {
    let mut app = app(Model::X4Pro);
    let _ = app.update(Message::Opened(PathBuf::from("/nowhere/wiki.zim")));
    let tipped = |app: &App, title: &str| {
        app.page()
            .rows
            .iter()
            .find(|r| r.title == title)
            .unwrap_or_else(|| panic!("{title}"))
            .tip
            .is_some()
    };
    for title in [
        "File",
        "Save to",
        "Made for",
        "Pictures",
        "Sharper diagrams",
        "Real italic",
    ] {
        assert!(tipped(&app, title), "{title}");
    }
    let _ = app.update(Message::Open(Screen::Settings));
    for title in ["Graphics", "Refresh flash", "Fonts for converting offline"] {
        assert!(tipped(&app, title), "{title}");
    }
}
