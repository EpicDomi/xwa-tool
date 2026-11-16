//! How the app behaves, driven through its messages; and, on request, a
//! picture of every screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use xwa_fetch::drives::Drive;
use xwa_fetch::{Book, Language};

use crate::app::{App, Choice, Job, Message, Popup, Screen};
use crate::device::{Finish, Key, Model};
use crate::eink;
use crate::prefs::Prefs;

fn app(model: Model) -> App {
    let prefs = Prefs {
        model,
        ..Prefs::default()
    };
    let (mut app, _) = App::new(prefs, 1.0, None);
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
    }
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
    assert_eq!(titles, ["Language", "Chemistry", "Chemistry"]);
    let _ = app.update(Message::Submit);
    assert_eq!(app.screen, Screen::Book);
    assert_eq!(app.book.as_ref().unwrap().title, "Chemistry");
}

#[test]
fn choosing_a_language_goes_back_to_the_list() {
    let mut app = app(Model::X4Pro);
    with_library(&mut app);
    let _ = app.update(Message::Open(Screen::Library));
    press(&mut app, Key::Confirm);
    assert_eq!(app.screen, Screen::Languages);
    let _ = app.update(Message::LanguageChosen(languages()[4].clone()));
    assert_eq!(app.screen, Screen::Library);
    assert_eq!(app.prefs.language.as_deref(), Some("swe"));
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
        let _ = a.update(Message::OpenBook(library()[0].clone()));
        snap(&a, &dir, &format!("{tag}-book"));
        a.downloading = a.book.clone();
        a.download = running(0.37, 0, "41.2 GB of 111.4 GB");
        snap(&a, &dir, &format!("{tag}-downloading"));

        let mut a = app(model);
        let _ = a.update(Message::Open(Screen::Convert));
        snap(&a, &dir, &format!("{tag}-convert-empty"));
        if let Some(zim) = &zim {
            a.zim = Some(zim.clone());
            a.zim_info = Some(crate::app::describe(zim));
            a.focus = 2;
            snap(&a, &dir, &format!("{tag}-convert"));
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
        a.archive = Some(PathBuf::from("wikipedia_en_chemistry_maxi_2026-10.xwa"));
        let _ = a.update(Message::Open(Screen::Card));
        a.drives = vec![card()];
        a.drive = a.drives.first().cloned();
        snap(&a, &dir, &format!("{tag}-card"));
        a.copy = running(0.62, 0, "255 MB of 412 MB");
        snap(&a, &dir, &format!("{tag}-copying"));

        let mut a = app(model);
        let _ = a.update(Message::Open(Screen::Settings));
        snap(&a, &dir, &format!("{tag}-settings"));
        press(&mut a, Key::Power);
        snap(&a, &dir, &format!("{tag}-power"));
    }
}

fn snap(app: &App, dir: &Path, name: &str) {
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
