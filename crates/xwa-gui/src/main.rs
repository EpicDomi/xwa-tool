#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end, two pages:
//! - Get Wikipedia: browse the Kiwix library and download a ZIM (resumes,
//!   checks the download).
//! - Convert: pick a ZIM (button, drag and drop, "open with", or a fresh
//!   download), choose the e-reader, convert, and copy the result to an
//!   SD card.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, scrollable, text, text_input,
};
use iced::{Alignment, Element, Length, Subscription, Task, event, window};
use xwa_convert::Stage;
use xwa_fetch::drives::Drive;
use xwa_fetch::{Book, Language, size_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Device {
    #[default]
    X4Pro,
    X4OrX3,
}

impl Device {
    const ALL: [Device; 2] = [Device::X4Pro, Device::X4OrX3];

    fn converter(self) -> xwa_convert::Device {
        match self {
            Device::X4Pro => xwa_convert::Device::X4Pro,
            Device::X4OrX3 => xwa_convert::Device::X4,
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Device::X4Pro => "Xteink X4 Pro",
            Device::X4OrX3 => "Xteink X4 / X3",
        })
    }
}

/// A language in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LanguageItem(Language);

impl fmt::Display for LanguageItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name)
    }
}

/// A drive in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DriveItem(Drive);

impl fmt::Display for DriveItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Convert,
    Library,
}

/// A long task on a worker thread: converting, downloading or copying.
enum Job {
    Idle,
    Running {
        stage: String,
        fraction: f32,
        cancel: Arc<AtomicBool>,
    },
    Finished(Result<String, String>),
}

impl Job {
    fn running(&self) -> bool {
        matches!(self, Job::Running { .. })
    }

    fn cancel(&mut self) {
        if let Job::Running { cancel, stage, .. } = self {
            cancel.store(true, Ordering::Relaxed);
            *stage = "Cancelling…".into();
        }
    }

    fn progress(&mut self, label: String, fraction: f32) {
        if let Job::Running {
            stage, fraction: f, ..
        } = self
        {
            *stage = label;
            *f = fraction;
        }
    }

    fn view(&self, failed: &'static str) -> Element<'_, Message> {
        match self {
            Job::Idle => text("").into(),
            Job::Running {
                stage, fraction, ..
            } => column![
                progress_bar(0.0..=1.0, *fraction),
                row![
                    text(stage.as_str()),
                    button("Cancel").on_press(Message::Cancel)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ]
            .spacing(8)
            .into(),
            Job::Finished(Ok(s)) => text(s.as_str()).into(),
            Job::Finished(Err(e)) => text(format!("{failed}: {e}")).into(),
        }
    }
}

struct App {
    page: Page,
    zim: Option<PathBuf>,
    details: Option<Result<String, String>>,
    device: Device,
    images: bool,
    loading: bool,
    /// The converting or copying job on the Convert page.
    job: Job,
    /// The archive the last conversion wrote, ready to copy.
    converted: Option<PathBuf>,
    output: Option<PathBuf>,
    drives: Vec<Drive>,
    drive: Option<Drive>,
    // Library page.
    languages: Vec<Language>,
    language: Option<Language>,
    books: Vec<Book>,
    library_status: Option<String>,
    search: String,
    download_dir: PathBuf,
    download: Job,
}

#[derive(Debug, Clone)]
enum Message {
    Show(Page),
    ChooseZim,
    ZimChosen(Option<PathBuf>),
    DetailsLoaded(Result<String, String>),
    DeviceChosen(Device),
    ImagesToggled(bool),
    Convert,
    OutputChosen(Option<PathBuf>),
    Conversion(Event<String>),
    Cancel,
    LookForCards,
    DriveChosen(DriveItem),
    CopyToCard,
    Copying(Event<PathBuf>),
    LanguagesLoaded(Result<Vec<Language>, String>),
    LanguageChosen(LanguageItem),
    BooksLoaded(Result<Vec<Book>, String>),
    SearchChanged(String),
    Download(Book),
    Downloading(Event<PathBuf>),
    ChooseDownloadDir,
    DownloadDirChosen(Option<PathBuf>),
}

/// Messages from a worker thread.
#[derive(Debug, Clone)]
enum Event<T> {
    Progress(String, f32),
    Done(Result<T, String>),
}

/// Runs `f` on its own thread and resolves to its result.
fn background<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.expect("the worker thread stopped") }
}

impl App {
    fn new(zim: Option<PathBuf>) -> (Self, Task<Message>) {
        let download_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let app = App {
            page: Page::default(),
            zim: None,
            details: None,
            device: Device::default(),
            images: true,
            loading: false,
            job: Job::Idle,
            converted: None,
            output: None,
            drives: Vec::new(),
            drive: None,
            languages: Vec::new(),
            language: None,
            books: Vec::new(),
            library_status: None,
            search: String::new(),
            download_dir,
            download: Job::Idle,
        };
        let task = match zim {
            Some(path) => Task::done(Message::ZimChosen(Some(path))),
            None => Task::none(),
        };
        (app, task)
    }

    fn busy(&self) -> bool {
        self.job.running()
    }

    fn load_books(&mut self) -> Task<Message> {
        let lang = self.language.as_ref().map(|l| l.code.clone());
        self.books.clear();
        self.library_status = Some("Loading the list…".into());
        Task::perform(
            background(move || {
                let filter = xwa_fetch::Filter {
                    lang: lang.as_deref(),
                    category: Some("wikipedia"),
                    name: None,
                };
                xwa_fetch::Client::new()
                    .books(&filter)
                    .map_err(|e| e.to_string())
            }),
            Message::BooksLoaded,
        )
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Show(page) => {
            app.page = page;
            if page == Page::Library && app.languages.is_empty() {
                app.library_status = Some("Loading the library…".into());
                return Task::perform(
                    background(|| {
                        xwa_fetch::Client::new()
                            .languages()
                            .map_err(|e| e.to_string())
                    }),
                    Message::LanguagesLoaded,
                );
            }
            Task::none()
        }
        Message::ChooseZim => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .add_filter("Kiwix ZIM archive", &["zim"])
                    .pick_file()
                    .await
                    .map(|f| f.path().to_path_buf())
            },
            Message::ZimChosen,
        ),
        Message::ZimChosen(Some(path)) if !app.busy() => {
            app.page = Page::Convert;
            app.zim = Some(path.clone());
            app.details = None;
            app.loading = true;
            app.job = Job::Idle;
            app.converted = None;
            Task::perform(background(move || describe(&path)), Message::DetailsLoaded)
        }
        Message::ZimChosen(_) => Task::none(),
        Message::DetailsLoaded(details) => {
            app.details = Some(details);
            app.loading = false;
            Task::none()
        }
        Message::DeviceChosen(device) => {
            app.device = device;
            Task::none()
        }
        Message::ImagesToggled(on) => {
            app.images = on;
            Task::none()
        }
        Message::Convert => {
            let Some(zim) = app.zim.clone() else {
                return Task::none();
            };
            let name = zim
                .with_extension("xwa")
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = zim.parent().map(Path::to_path_buf);
            Task::perform(
                async move {
                    let mut dialog = rfd::AsyncFileDialog::new()
                        .add_filter("XWA archive", &["xwa"])
                        .set_file_name(name);
                    if let Some(dir) = dir {
                        dialog = dialog.set_directory(dir);
                    }
                    dialog.save_file().await.map(|f| f.path().to_path_buf())
                },
                Message::OutputChosen,
            )
        }
        Message::OutputChosen(Some(out)) => {
            let Some(zim) = app.zim.clone() else {
                return Task::none();
            };
            if out == zim {
                app.job = Job::Finished(Err(
                    "Choose a different name: that would replace the ZIM file.".into(),
                ));
                return Task::none();
            }
            let cancel = Arc::new(AtomicBool::new(false));
            app.job = Job::Running {
                stage: "Starting…".into(),
                fraction: 0.0,
                cancel: cancel.clone(),
            };
            app.converted = None;
            app.output = Some(out.clone());
            let device = app.device.converter();
            let with_images = app.images;
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                let opts = xwa_convert::Options {
                    images: with_images,
                    ..xwa_convert::Options::new(device)
                };
                let result = xwa_convert::convert(&zim, &out, &opts, &mut |p| {
                    let (label, fraction) = conversion_progress(p);
                    let _ = tx.unbounded_send(Event::Progress(label.into(), fraction));
                    !cancel.load(Ordering::Relaxed)
                });
                let _ = tx.unbounded_send(Event::Done(
                    result
                        .map(|r| summary(&r, &out, device))
                        .map_err(|e| e.to_string()),
                ));
            });
            Task::run(rx, Message::Conversion)
        }
        Message::OutputChosen(None) => Task::none(),
        Message::Conversion(Event::Progress(label, fraction)) => {
            app.job.progress(label, fraction);
            Task::none()
        }
        Message::Conversion(Event::Done(result)) => {
            if result.is_ok() {
                app.converted = app.output.take();
                app.drives = xwa_fetch::drives::removable();
                app.drive = app.drives.first().cloned();
            }
            app.job = Job::Finished(match result {
                Err(e) if e == "cancelled" => Err("Conversion cancelled.".into()),
                other => other,
            });
            Task::none()
        }
        Message::Cancel => {
            app.job.cancel();
            app.download.cancel();
            Task::none()
        }
        Message::LookForCards => {
            app.drives = xwa_fetch::drives::removable();
            if app.drive.as_ref().is_none_or(|d| !app.drives.contains(d)) {
                app.drive = app.drives.first().cloned();
            }
            Task::none()
        }
        Message::DriveChosen(DriveItem(d)) => {
            app.drive = Some(d);
            Task::none()
        }
        Message::CopyToCard => {
            let (Some(file), Some(drive)) = (app.converted.clone(), app.drive.clone()) else {
                return Task::none();
            };
            let cancel = Arc::new(AtomicBool::new(false));
            app.job = Job::Running {
                stage: "Copying to the card…".into(),
                fraction: 0.0,
                cancel: cancel.clone(),
            };
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                let result = xwa_fetch::drives::copy(&file, &drive.mount, Some(&drive), &mut |p| {
                    let fraction = p.done as f32 / p.total.max(1) as f32;
                    let label = format!(
                        "Copying to the card… {} of {}",
                        size_text(p.done),
                        size_text(p.total)
                    );
                    let _ = tx.unbounded_send(Event::Progress(label, fraction));
                    !cancel.load(Ordering::Relaxed)
                });
                let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
            });
            Task::run(rx, Message::Copying)
        }
        Message::Copying(Event::Progress(label, fraction)) => {
            app.job.progress(label, fraction);
            Task::none()
        }
        Message::Copying(Event::Done(result)) => {
            app.job = Job::Finished(match result {
                Ok(path) => Ok(format!(
                    "Copied to {}. You can now eject the card safely and put it in your e-reader.",
                    path.display()
                )),
                Err(e) if e == "cancelled" => Err("Copy cancelled.".into()),
                Err(e) => Err(e),
            });
            Task::none()
        }
        Message::LanguagesLoaded(Ok(languages)) => {
            app.language = languages
                .iter()
                .find(|l| l.code == "eng")
                .or(languages.first())
                .cloned();
            app.languages = languages;
            app.load_books()
        }
        Message::LanguagesLoaded(Err(e)) => {
            app.library_status = Some(format!("Could not reach the Kiwix library: {e}"));
            Task::none()
        }
        Message::LanguageChosen(LanguageItem(l)) => {
            app.language = Some(l);
            app.load_books()
        }
        Message::BooksLoaded(Ok(mut books)) => {
            app.library_status = books
                .is_empty()
                .then(|| "No Wikipedia files in this language.".into());
            // The whole Wikipedia (names ending in "_all") first, then topic
            // selections by title; most complete first within a title.
            let flavour_rank = |f: &str| match f {
                "maxi" => 0,
                "nopic" => 1,
                "mini" => 2,
                _ => 3,
            };
            books.sort_by(|a, b| {
                let key = |x: &Book| {
                    (
                        !x.name.ends_with("_all"),
                        x.title.clone(),
                        flavour_rank(&x.flavour),
                    )
                };
                key(a).cmp(&key(b)).then_with(|| b.date.cmp(&a.date))
            });
            app.books = books;
            Task::none()
        }
        Message::BooksLoaded(Err(e)) => {
            app.library_status = Some(format!("Could not load the list: {e}"));
            Task::none()
        }
        Message::SearchChanged(s) => {
            app.search = s;
            Task::none()
        }
        Message::ChooseDownloadDir => {
            let dir = app.download_dir.clone();
            Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_directory(dir)
                        .pick_folder()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Message::DownloadDirChosen,
            )
        }
        Message::DownloadDirChosen(Some(dir)) => {
            app.download_dir = dir;
            Task::none()
        }
        Message::DownloadDirChosen(None) => Task::none(),
        Message::Download(book) if !app.download.running() => {
            let cancel = Arc::new(AtomicBool::new(false));
            app.download = Job::Running {
                stage: "Starting the download…".into(),
                fraction: 0.0,
                cancel: cancel.clone(),
            };
            let dir = app.download_dir.clone();
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                let result = xwa_fetch::Client::new().download(&book.url, &dir, &mut |p| {
                    let fraction = p.done as f32 / p.total.max(1) as f32;
                    let label = match p.stage {
                        xwa_fetch::Stage::Downloading => {
                            format!(
                                "Downloading {}… {} of {}",
                                book.title,
                                size_text(p.done),
                                size_text(p.total)
                            )
                        }
                        xwa_fetch::Stage::Checking => "Checking the download…".to_string(),
                    };
                    let _ = tx.unbounded_send(Event::Progress(label, fraction));
                    !cancel.load(Ordering::Relaxed)
                });
                let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
            });
            Task::run(rx, Message::Downloading)
        }
        Message::Download(_) => Task::none(),
        Message::Downloading(Event::Progress(label, fraction)) => {
            app.download.progress(label, fraction);
            Task::none()
        }
        Message::Downloading(Event::Done(Ok(path))) => {
            app.download = Job::Finished(Ok(format!("Downloaded {}.", path.display())));
            // On to converting it.
            update(app, Message::ZimChosen(Some(path)))
        }
        Message::Downloading(Event::Done(Err(e))) => {
            app.download = Job::Finished(Err(if e == "cancelled" {
                "Download paused. Press Download again to continue where it stopped.".into()
            } else {
                e
            }));
            Task::none()
        }
    }
}

fn conversion_progress(p: xwa_convert::Progress) -> (&'static str, f32) {
    let label = match p.stage {
        Stage::Scanning => "Reading the ZIM file…",
        Stage::Articles => "Converting articles…",
        Stage::Images => "Converting pictures…",
        Stage::Finishing => "Finishing…",
    };
    // Rough shares of the time: reading 5%, articles 55%, pictures 35%,
    // finishing 5%.
    let part = if p.total == 0 {
        1.0
    } else {
        p.done as f32 / p.total as f32
    };
    let fraction = match p.stage {
        Stage::Scanning => 0.05 * part,
        Stage::Articles => 0.05 + 0.55 * part,
        Stage::Images => 0.60 + 0.35 * part,
        Stage::Finishing => 0.95 + 0.05 * part,
    };
    (label, fraction)
}

fn summary(r: &xwa_convert::Report, out: &Path, device: xwa_convert::Device) -> String {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pictures = match r.summary.images {
        0 => "no pictures".to_string(),
        n => format!("{n} pictures"),
    };
    format!(
        "Done: {} articles and {pictures} in {} ({:.0} MB), made for the {}.",
        r.summary.articles,
        name,
        (r.summary.file_size as f64 / 1e6).max(1.0),
        device.name()
    )
}

/// A short summary read from the ZIM header and metadata (no full scan).
fn describe(path: &Path) -> Result<String, String> {
    let zim = xwa_zim::Archive::open(path).map_err(|e| e.to_string())?;
    let meta = |name: &str| -> String {
        zim.metadata(name)
            .ok()
            .flatten()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|| "?".into())
    };
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "{}\n{} · {} · {}\n{} entries · {}",
        meta("Title"),
        meta("Language"),
        meta("Flavour"),
        meta("Date"),
        zim.entry_count(),
        size_text(size)
    ))
}

fn view(app: &App) -> Element<'_, Message> {
    let tab = |label: &'static str, page: Page| {
        let b = button(label).on_press(Message::Show(page));
        if app.page == page {
            b
        } else {
            b.style(button::secondary)
        }
    };
    let tabs = row![
        tab("Convert", Page::Convert),
        tab("Get Wikipedia", Page::Library)
    ]
    .spacing(8);
    let body = match app.page {
        Page::Convert => convert_view(app),
        Page::Library => library_view(app),
    };
    column![tabs, body].spacing(20).padding(24).into()
}

fn convert_view(app: &App) -> Element<'_, Message> {
    let file = app.zim.as_ref().map_or_else(
        || "No file chosen (you can also drop one here)".to_string(),
        |p| p.display().to_string(),
    );
    let details = match (&app.details, app.loading) {
        (_, true) => text("Reading…"),
        (Some(Ok(d)), _) => text(d.as_str()),
        (Some(Err(e)), _) => text(format!("Could not read this file: {e}")),
        (None, _) => text(""),
    };
    let busy = app.busy();
    let readable = matches!(app.details, Some(Ok(_)));
    let choose = button("Choose ZIM file…").on_press_maybe((!busy).then_some(Message::ChooseZim));
    let devices = pick_list(&Device::ALL[..], Some(app.device), Message::DeviceChosen);
    let convert = button("Convert").on_press_maybe((readable && !busy).then_some(Message::Convert));
    let mut page = column![
        text("Convert a Kiwix ZIM for your Xteink").size(22),
        row![choose, text(file)]
            .spacing(12)
            .align_y(Alignment::Center),
        details,
        row![text("Device"), devices]
            .spacing(12)
            .align_y(Alignment::Center),
        checkbox(app.images)
            .label("Include pictures")
            .on_toggle_maybe((!busy).then_some(Message::ImagesToggled)),
        convert,
        app.job.view("It did not work"),
    ]
    .spacing(16);
    if app.converted.is_some() {
        let cards: Vec<DriveItem> = app.drives.iter().cloned().map(DriveItem).collect();
        let chosen = app.drive.clone().map(DriveItem);
        let card_row: Element<'_, Message> = if cards.is_empty() {
            text("No SD card found. Insert the card (in a card reader, or the e-reader connected by USB), then look again.")
                .into()
        } else {
            row![
                pick_list(cards, chosen, Message::DriveChosen),
                button("Copy")
                    .on_press_maybe((!busy && app.drive.is_some()).then_some(Message::CopyToCard)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        };
        page = page.push(
            column![
                text("Copy to your e-reader").size(18),
                card_row,
                button("Look again")
                    .style(button::secondary)
                    .on_press_maybe((!busy).then_some(Message::LookForCards)),
            ]
            .spacing(10),
        );
    }
    page.into()
}

fn library_view(app: &App) -> Element<'_, Message> {
    let languages: Vec<LanguageItem> = app.languages.iter().cloned().map(LanguageItem).collect();
    let chosen = app.language.clone().map(LanguageItem);
    let filters = row![
        text("Language"),
        pick_list(languages, chosen, Message::LanguageChosen),
        text_input("Search", &app.search)
            .on_input(Message::SearchChanged)
            .width(Length::Fixed(240.0)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let downloading = app.download.running();
    let needle = app.search.to_lowercase();
    let mut list = column![].spacing(12);
    for b in app.books.iter().filter(|b| {
        needle.is_empty()
            || b.title.to_lowercase().contains(&needle)
            || b.name.to_lowercase().contains(&needle)
    }) {
        let info = column![
            text(format!("{}: {}", b.title, b.flavour_description())).size(16),
            text(format!(
                "{} · {} · {}",
                size_text(b.size),
                b.date,
                b.file_name()
            ))
            .size(13),
        ]
        .width(Length::Fill);
        let download =
            button("Download").on_press_maybe((!downloading).then(|| Message::Download(b.clone())));
        list = list.push(row![info, download].spacing(12).align_y(Alignment::Center));
    }
    let status: Element<'_, Message> = match &app.library_status {
        Some(s) => text(s.as_str()).into(),
        None => scrollable(container(list).padding([0, 16]))
            .height(Length::Fill)
            .into(),
    };
    column![
        text("Download Wikipedia from the Kiwix library").size(22),
        filters,
        row![
            text(format!("Saves to {}", app.download_dir.display())),
            button("Change…")
                .style(button::secondary)
                .on_press_maybe((!downloading).then_some(Message::ChooseDownloadDir)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        app.download.view("The download stopped"),
        status,
    ]
    .spacing(16)
    .into()
}

fn subscription(_app: &App) -> Subscription<Message> {
    event::listen_with(|event, _status, _window| match event {
        iced::Event::Window(window::Event::FileDropped(path)) => {
            Some(Message::ZimChosen(Some(path)))
        }
        _ => None,
    })
}

fn main() -> iced::Result {
    let zim = std::env::args_os().nth(1).map(PathBuf::from);
    iced::application(move || App::new(zim.clone()), update, view)
        .subscription(subscription)
        .title("xwa-tool")
        .window_size((900.0, 680.0))
        .run()
}
