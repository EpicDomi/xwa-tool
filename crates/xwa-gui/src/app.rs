//! The app's state and what each message does to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iced::widget::operation;
use iced::{Point, Size, Subscription, Task, event, keyboard, mouse, window};
use xwa_convert::Stage;
use xwa_fetch::drives::Drive;
use xwa_fetch::wikimedia::Diagrams;
use xwa_fetch::{Book, Language, size_text};

use crate::device::{Finish, Key, Model};
use crate::eink;
use crate::prefs::{self, Prefs};

/// Zoom steps, in desktop pixels per screen pixel.
pub const ZOOMS: [f32; 10] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6];
/// The zoom the window opens at before the monitor's size is known: small
/// enough for any screen.
pub const FIRST_ZOOM: f32 = 0.7;
/// How much of the monitor's height the device may take when fitted.
const FIT: f32 = 0.92;

/// A full refresh: black, then white, then the new screen.
const FLASH_BLACK: Duration = Duration::from_millis(45);
const FLASH_WHITE: Duration = Duration::from_millis(90);
/// How long a key looks pressed when pressed from the keyboard.
const KEY_PRESS: Duration = Duration::from_millis(130);

pub const SEARCH_ID: &str = "search";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Library,
    Languages,
    Book,
    Convert,
    Card,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    Power,
    /// Turning off would stop work in progress.
    TurnOffBusy,
    /// Stopping a conversion throws away the work so far.
    StopConversion,
    /// Picking one of a setting's values.
    Choose(Choice),
}

/// A setting chosen from a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Device,
    Case,
    Size,
}

/// A long task on a worker thread: converting, downloading or copying.
#[derive(Debug, Default)]
pub enum Job {
    #[default]
    Idle,
    Running {
        label: String,
        fraction: f32,
        /// Which step of the work it is on, for conversions.
        step: usize,
        cancel: Arc<AtomicBool>,
        started: Instant,
    },
    Finished(Result<String, String>),
}

impl Job {
    fn start() -> (Job, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        (
            Job::Running {
                label: "Starting…".into(),
                fraction: 0.0,
                step: 0,
                cancel: cancel.clone(),
                started: Instant::now(),
            },
            cancel,
        )
    }

    pub fn running(&self) -> bool {
        matches!(self, Job::Running { .. })
    }

    fn cancel(&mut self) {
        if let Job::Running { cancel, label, .. } = self {
            cancel.store(true, Ordering::Relaxed);
            *label = "Stopping…".into();
        }
    }

    fn progress(&mut self, p: Progress) {
        if let Job::Running {
            label,
            fraction,
            step,
            cancel,
            ..
        } = self
            && !cancel.load(Ordering::Relaxed)
        {
            *label = p.label;
            *fraction = p.fraction;
            *step = p.step;
        }
    }

    pub fn fraction(&self) -> Option<f32> {
        match self {
            Job::Running { fraction, .. } => Some(*fraction),
            _ => None,
        }
    }

    /// Roughly how long is left, once there is enough to go on.
    pub fn time_left(&self) -> Option<Duration> {
        let Job::Running {
            fraction, started, ..
        } = self
        else {
            return None;
        };
        let elapsed = started.elapsed().as_secs_f32();
        (*fraction >= 0.02 && elapsed >= 10.0)
            .then(|| Duration::from_secs_f32(elapsed * (1.0 - fraction) / fraction))
    }
}

/// What a ZIM file says about itself.
#[derive(Debug, Clone)]
pub struct ZimInfo {
    pub title: String,
    pub language: String,
    pub flavour: String,
    pub date: String,
    pub entries: u32,
    pub size: u64,
    /// Its illustration in the panel's greys.
    pub cover: Option<iced::widget::image::Handle>,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub label: String,
    pub fraction: f32,
    pub step: usize,
}

/// Messages from a worker thread.
#[derive(Debug, Clone)]
pub enum Event<T> {
    Progress(Progress),
    Done(Result<T, String>),
}

#[derive(Debug, Clone)]
pub enum Message {
    // The window and the device.
    Window(Option<window::Id>),
    Monitor(Option<Size>),
    Placed(Option<Point>),
    Drag,
    SystemMenu,
    KeyHover(Option<Key>),
    KeyDown(Key),
    KeyUp(Key),
    /// A key's action, from the device, the screen or the keyboard.
    Press(Key),
    Frame(Instant),
    Clock,
    Zoom(i32),
    CloseRequested,
    Sleep,
    TurnOff,
    ClosePopup,
    // Moving about.
    Open(Screen),
    Back,
    Focus(usize),
    PopupFocus(usize),
    Scroll(mouse::ScrollDelta),
    // Settings.
    Choose(Choice),
    SetModel(Model),
    SetFinish(Finish),
    /// A zoom, or `None` to fit the monitor.
    SetZoom(Option<f32>),
    ToggleFlash,
    ChooseDownloadDir,
    DownloadDirChosen(Option<PathBuf>),
    /// A file opened with the app or dropped on it.
    Opened(PathBuf),
    // Converting.
    ChooseZim,
    ZimChosen(Option<PathBuf>),
    ZimRead(PathBuf, Result<ZimInfo, String>),
    ToggleImages,
    ToggleSharper,
    Convert,
    OutputChosen(Option<PathBuf>),
    Conversion(Event<String>),
    AskStopConversion,
    CancelConversion,
    /// Back to the settings, to convert the same file again.
    ConvertAgain,
    // Copying.
    ChooseArchive,
    ArchiveChosen(Option<PathBuf>),
    LookForCards,
    DriveChosen(Drive),
    CopyToCard,
    Copying(Event<PathBuf>),
    CancelCopy,
    // The Kiwix library.
    LoadLibrary,
    LanguagesLoaded(Result<Vec<Language>, String>),
    LanguageChosen(Language),
    BooksLoaded(Result<Vec<Book>, String>),
    Search(String),
    LanguageSearch(String),
    Submit,
    OpenBook(Book),
    Download,
    Downloading(Event<PathBuf>),
    CancelDownload,
}

pub struct App {
    pub prefs: Prefs,
    pub window: Option<window::Id>,
    pub zoom: f32,
    /// The monitor's size in desktop pixels, once known.
    monitor: Option<Size>,
    retried_monitor: bool,
    /// Whether the window has been shown.
    shown: bool,
    pub screen: Screen,
    history: Vec<(Screen, usize)>,
    pub focus: usize,
    scrolled: f32,
    pub popup: Option<Popup>,
    pub popup_focus: usize,
    pub flash: Option<Instant>,
    pub hovered_key: Option<Key>,
    pub pressed_key: Option<Key>,
    /// A key pressed from the keyboard, shown pressed for a moment.
    pub tapped_key: Option<(Key, Instant)>,
    pub clock: String,

    // Converting.
    pub zim: Option<PathBuf>,
    pub zim_info: Option<Result<ZimInfo, String>>,
    pub images: bool,
    pub sharper: bool,
    pub conversion: Job,
    /// Whether the running conversion fetches sharper diagrams.
    pub converting_sharper: bool,
    pub converting_images: bool,
    pub output: Option<PathBuf>,

    // Copying.
    pub archive: Option<PathBuf>,
    pub drives: Vec<Drive>,
    pub drive: Option<Drive>,
    pub copy: Job,

    // The Kiwix library.
    pub languages: Vec<Language>,
    pub language: Option<Language>,
    pub books: Vec<Book>,
    pub library_status: Option<String>,
    pub library_failed: bool,
    pub search: String,
    pub language_search: String,
    pub book: Option<Book>,
    pub download: Job,
    pub downloading: Option<Book>,
}

impl App {
    pub fn new(prefs: Prefs, zoom: f32, open: Option<PathBuf>) -> (Self, Task<Message>) {
        let mut app = App {
            prefs,
            window: None,
            zoom,
            monitor: None,
            retried_monitor: false,
            shown: false,
            screen: Screen::Home,
            history: Vec::new(),
            focus: 0,
            scrolled: 0.0,
            popup: None,
            popup_focus: 0,
            flash: None,
            hovered_key: None,
            pressed_key: None,
            tapped_key: None,
            clock: clock(),
            zim: None,
            zim_info: None,
            images: true,
            sharper: false,
            conversion: Job::Idle,
            converting_sharper: false,
            converting_images: true,
            output: None,
            archive: None,
            drives: Vec::new(),
            drive: None,
            copy: Job::Idle,
            languages: Vec::new(),
            language: None,
            books: Vec::new(),
            library_status: None,
            library_failed: false,
            search: String::new(),
            language_search: String::new(),
            book: None,
            download: Job::Idle,
            downloading: None,
        };
        let mut tasks = vec![window::oldest().map(Message::Window)];
        if let Some(path) = open {
            tasks.push(app.open_file(path));
        }
        app.settle();
        (app, Task::batch(tasks))
    }

    pub fn spec(&self) -> &'static crate::device::Spec {
        self.prefs.model.spec()
    }

    pub fn download_dir(&self) -> PathBuf {
        self.prefs
            .download_dir
            .clone()
            .unwrap_or_else(prefs::default_download_dir)
    }

    pub fn busy(&self) -> bool {
        self.conversion.running() || self.copy.running() || self.download.running()
    }

    /// What is running, for the status line.
    pub fn status(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(f) = self.conversion.fraction() {
            parts.push(format!("Converting {:.0}%", f * 100.0));
        }
        if let Some(f) = self.download.fraction() {
            parts.push(format!("Downloading {:.0}%", f * 100.0));
        }
        if let Some(f) = self.copy.fraction() {
            parts.push(format!("Copying {:.0}%", f * 100.0));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// A ZIM to convert or an archive to copy, from the command line or a
    /// file dropped on the window.
    fn open_file(&mut self, path: PathBuf) -> Task<Message> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        if ext.as_deref() == Some("xwa") {
            self.archive = Some(path);
            self.copy = Job::Idle;
            self.refresh_drives();
            self.go(Screen::Card);
            Task::none()
        } else {
            self.update(Message::ZimChosen(Some(path)))
        }
    }

    // -----------------------------------------------------------------------
    // Moving between screens

    /// Opens a screen on top of this one.
    pub fn go(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        if screen == Screen::Home {
            self.history.clear();
        } else {
            self.history.push((self.screen, self.focus));
        }
        self.show(screen, 0);
        self.focus = self.first_focusable();
    }

    fn show(&mut self, screen: Screen, focus: usize) {
        self.screen = screen;
        self.focus = focus;
        self.scrolled = 0.0;
        if self.prefs.refresh_flash {
            self.flash = Some(Instant::now());
        }
    }

    fn back(&mut self) {
        match self.history.pop() {
            Some((screen, focus)) => self.show(screen, focus),
            None if self.screen != Screen::Home => self.show(Screen::Home, 0),
            None => {}
        }
    }

    fn first_focusable(&self) -> usize {
        self.page()
            .rows
            .iter()
            .position(eink::Row::focusable)
            .unwrap_or(0)
    }

    /// Keeps the selection on a row that can take it.
    fn settle(&mut self) {
        let rows = self.page().rows;
        if rows.is_empty() {
            self.focus = 0;
            return;
        }
        self.focus = self.focus.min(rows.len() - 1);
        if !rows[self.focus].focusable() {
            let after = (self.focus..rows.len()).find(|&i| rows[i].focusable());
            let before = (0..self.focus).rev().find(|&i| rows[i].focusable());
            self.focus = after.or(before).unwrap_or(self.focus);
        }
    }

    /// Moves the selection by `step` rows, round the ends.
    fn step_focus(&mut self, step: isize) {
        let rows = self.page().rows;
        let n = rows.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.focus as isize;
        for _ in 0..n {
            i = (i + step).rem_euclid(n);
            if rows[i as usize].focusable() {
                self.focus = i as usize;
                return;
            }
        }
    }

    /// Shows the next or previous page of the list.
    fn turn_page(&mut self, step: isize) {
        let page = self.page();
        let pages = eink::paginate(&page.rows, self.list_room(&page));
        if pages.len() < 2 {
            self.step_focus(step);
            return;
        }
        let current = pages
            .iter()
            .position(|p| p.contains(&self.focus))
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(pages.len() as isize) as usize;
        self.focus = pages[next].start;
        self.settle();
    }

    /// The height left for the list rows on the current screen.
    pub fn list_room(&self, page: &crate::screens::Page) -> f32 {
        let spec = self.spec();
        let hints = if spec.hints.is_some() {
            eink::HINTS
        } else {
            0.0
        };
        spec.screen.height - eink::HEADER - page.top - page.bottom - hints
    }

    fn press(&mut self, key: Key) -> Task<Message> {
        if let Some(popup) = self.popup {
            let rows = self.popup_rows(popup);
            let n = rows.len();
            match key {
                Key::Back | Key::Power | Key::Home => self.popup = None,
                Key::Left | Key::Up => self.popup_focus = (self.popup_focus + n - 1) % n,
                Key::Right | Key::Down => self.popup_focus = (self.popup_focus + 1) % n,
                Key::Confirm => {
                    if let Some(m) = rows.get(self.popup_focus).and_then(|r| r.on_press.clone()) {
                        return self.update(m);
                    }
                }
            }
            return Task::none();
        }
        match key {
            Key::Power => {
                self.popup = Some(Popup::Power);
                self.popup_focus = 0;
            }
            Key::Home => self.go(Screen::Home),
            Key::Back => self.back(),
            Key::Confirm => {
                let rows = self.page().rows;
                if let Some(m) = rows.get(self.focus).and_then(|r| r.on_press.clone()) {
                    return self.update(m);
                }
            }
            Key::Left => self.step_focus(-1),
            Key::Right => self.step_focus(1),
            Key::Up => self.turn_page(-1),
            Key::Down => self.turn_page(1),
        }
        Task::none()
    }

    // -----------------------------------------------------------------------
    // The window

    fn resize(&self) -> Task<Message> {
        match self.window {
            Some(id) => window::resize(id, self.spec().window()),
            None => Task::none(),
        }
    }

    /// The largest zoom step at which the device fits the monitor.
    pub fn max_zoom(&self) -> f32 {
        let Some(monitor) = self.monitor else {
            return ZOOMS[ZOOMS.len() - 1];
        };
        let body = self.spec().window();
        let fit = (FIT * monitor.height / body.height).min(FIT * monitor.width / body.width);
        ZOOMS
            .iter()
            .copied()
            .rfind(|&z| z <= fit + 1e-3)
            .unwrap_or(ZOOMS[0])
    }

    /// The zoom to use: the one chosen, or the biggest up to life size that
    /// fits; never more than fits.
    fn wanted_zoom(&self) -> f32 {
        let max = self.max_zoom();
        self.prefs.zoom.unwrap_or(max.min(1.0)).clamp(ZOOMS[0], max)
    }

    /// Changes the zoom and resizes the window to match.
    fn zoom_to(&mut self, zoom: f32) -> Task<Message> {
        if (zoom - self.zoom).abs() < 1e-3 {
            return Task::none();
        }
        self.zoom = zoom;
        self.resize()
    }

    /// Changes the zoom by choice, and remembers it.
    fn choose_zoom(&mut self, zoom: f32) -> Task<Message> {
        self.prefs.zoom = Some(zoom.clamp(ZOOMS[0], self.max_zoom()));
        self.prefs.save();
        let zoom = self.wanted_zoom();
        self.zoom_to(zoom)
    }

    fn show_window(&mut self) -> Task<Message> {
        let Some(id) = self.window else {
            return Task::none();
        };
        if self.shown {
            return Task::none();
        }
        self.shown = true;
        if self.prefs.refresh_flash {
            self.flash = Some(Instant::now());
        }
        Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::gain_focus(id),
        ])
    }

    // -----------------------------------------------------------------------
    // Updating

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.apply(message);
        self.settle();
        task
    }

    fn apply(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Window(Some(id)) => {
                self.window = Some(id);
                return window::monitor_size(id).map(Message::Monitor);
            }
            Message::Window(None) => {}
            Message::Monitor(size) => {
                let Some(id) = self.window else {
                    return Task::none();
                };
                let Some(size) = size else {
                    // Some systems (Wayland) only say once the window is up.
                    let show = self.show_window();
                    return if self.monitor.is_none() && !self.retried_monitor {
                        self.retried_monitor = true;
                        show.chain(window::monitor_size(id).map(Message::Monitor))
                    } else {
                        show
                    };
                };
                // The size comes in this window's units: undo the zoom.
                self.monitor = Some(Size::new(size.width * self.zoom, size.height * self.zoom));
                if (self.wanted_zoom() - self.zoom).abs() < 1e-3 {
                    return self.show_window();
                }
                return window::position(id).map(Message::Placed);
            }
            Message::Placed(position) => {
                // Grow or shrink about the window's middle, so the device
                // stays on screen.
                let Some(id) = self.window else {
                    return Task::none();
                };
                let wanted = self.wanted_zoom();
                let body = self.spec().window();
                let mut tasks = Vec::new();
                if let Some(at) = position {
                    let change = wanted - self.zoom;
                    tasks.push(window::move_to(
                        id,
                        Point::new(
                            at.x - body.width * change / 2.0,
                            (at.y - body.height * change / 2.0).max(0.0),
                        ),
                    ));
                }
                tasks.push(self.zoom_to(wanted));
                let show = self.show_window();
                return Task::batch(tasks).chain(show);
            }
            Message::Zoom(0) => {
                // Back to fitting the monitor.
                self.prefs.zoom = None;
                self.prefs.save();
                let zoom = self.wanted_zoom();
                return self.zoom_to(zoom);
            }
            Message::Zoom(step) => {
                let i = ZOOMS
                    .iter()
                    .position(|&z| z >= self.zoom - 1e-3)
                    .unwrap_or(ZOOMS.len() - 1) as i32;
                let target = ZOOMS[(i + step).clamp(0, ZOOMS.len() as i32 - 1) as usize];
                return self.choose_zoom(target);
            }
            Message::SetZoom(zoom) => {
                self.popup = None;
                return match zoom {
                    Some(z) => self.choose_zoom(z),
                    None => self.update(Message::Zoom(0)),
                };
            }
            Message::Drag => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::SystemMenu => {
                if let Some(id) = self.window {
                    return window::show_system_menu(id);
                }
            }
            Message::KeyHover(key) => {
                self.hovered_key = key;
                if key.is_none() {
                    self.pressed_key = None;
                }
            }
            Message::KeyDown(key) => self.pressed_key = Some(key),
            Message::KeyUp(key) => {
                if self.pressed_key.take() == Some(key) {
                    return self.press(key);
                }
            }
            Message::Press(key) => {
                self.tapped_key = Some((key, Instant::now()));
                return self.press(key);
            }
            Message::Frame(now) => {
                if self
                    .flash
                    .is_some_and(|f| now.saturating_duration_since(f) >= FLASH_WHITE)
                {
                    self.flash = None;
                }
                if self
                    .tapped_key
                    .is_some_and(|(_, at)| now.saturating_duration_since(at) >= KEY_PRESS)
                {
                    self.tapped_key = None;
                }
            }
            Message::Clock => self.clock = clock(),
            Message::CloseRequested => {
                if self.busy() {
                    self.popup = Some(Popup::TurnOffBusy);
                    self.popup_focus = 0;
                } else {
                    return iced::exit();
                }
            }
            Message::TurnOff => {
                self.conversion.cancel();
                self.copy.cancel();
                self.download.cancel();
                return iced::exit();
            }
            Message::Sleep => {
                self.popup = None;
                if let Some(id) = self.window {
                    return window::minimize(id, true);
                }
            }
            Message::ClosePopup => self.popup = None,
            Message::Opened(path) => return self.open_file(path),
            Message::Open(screen) => {
                self.go(screen);
                return self.entered();
            }
            Message::Back => self.back(),
            Message::Focus(i) => self.focus = i,
            Message::PopupFocus(i) => self.popup_focus = i,
            Message::Scroll(delta) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                self.scrolled += lines;
                if self.scrolled.abs() >= 1.0 {
                    let step = if self.scrolled > 0.0 { -1 } else { 1 };
                    self.scrolled = 0.0;
                    self.turn_page(step);
                }
            }

            // Settings.
            Message::Choose(choice) => {
                // What a running conversion is made for cannot change.
                if choice == Choice::Device && self.conversion.running() {
                    return Task::none();
                }
                let popup = Popup::Choose(choice);
                // The list opens on the value in use.
                self.popup_focus = self
                    .popup_rows(popup)
                    .iter()
                    .position(|r| r.value.as_deref() == Some(crate::screens::SELECTED))
                    .unwrap_or(0);
                self.popup = Some(popup);
            }
            Message::SetModel(model) => {
                self.popup = None;
                if !self.conversion.running() && model != self.prefs.model {
                    self.prefs.model = model;
                    self.prefs.save();
                    // A bigger case may no longer fit at this zoom.
                    self.zoom = self.wanted_zoom();
                    return self.resize();
                }
            }
            Message::SetFinish(finish) => {
                self.popup = None;
                self.prefs.finish = Some(finish);
                self.prefs.save();
            }
            Message::ToggleFlash => {
                self.prefs.refresh_flash = !self.prefs.refresh_flash;
                self.prefs.save();
            }
            Message::ChooseDownloadDir => {
                let dir = self.download_dir();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_directory(dir)
                            .pick_folder()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::DownloadDirChosen,
                );
            }
            Message::DownloadDirChosen(Some(dir)) => {
                self.prefs.download_dir = Some(dir);
                self.prefs.save();
            }
            Message::DownloadDirChosen(None) => {}

            // Converting.
            Message::ChooseZim => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("Kiwix ZIM file", &["zim"])
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::ZimChosen,
                );
            }
            Message::ZimChosen(Some(path)) if !self.conversion.running() => {
                self.zim = Some(path.clone());
                self.zim_info = None;
                self.conversion = Job::Idle;
                self.go(Screen::Convert);
                let read = path.clone();
                return Task::perform(background(move || describe(&read)), move |info| {
                    Message::ZimRead(path.clone(), info)
                });
            }
            Message::ZimChosen(_) => {}
            Message::ZimRead(path, info) => {
                if self.zim.as_ref() == Some(&path) {
                    self.zim_info = Some(info);
                }
            }
            Message::ToggleImages => {
                if !self.conversion.running() {
                    self.images = !self.images;
                }
            }
            Message::ToggleSharper => {
                if !self.conversion.running() && self.images {
                    self.sharper = !self.sharper;
                }
            }
            Message::Convert => {
                let Some(zim) = self.zim.clone() else {
                    return Task::none();
                };
                let name = zim
                    .with_extension("xwa")
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir = zim.parent().map(Path::to_path_buf);
                return Task::perform(
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
                );
            }
            Message::OutputChosen(Some(out)) => return self.start_conversion(out),
            Message::OutputChosen(None) => {}
            Message::Conversion(Event::Progress(p)) => self.conversion.progress(p),
            Message::Conversion(Event::Done(result)) => {
                if result.is_ok() {
                    self.archive = self.output.take();
                    self.copy = Job::Idle;
                    self.refresh_drives();
                }
                self.conversion = Job::Finished(match result {
                    Err(e) if e == "cancelled" => Err("Conversion stopped.".into()),
                    other => other,
                });
                self.focus = self.first_focusable();
            }
            Message::ConvertAgain => {
                if !self.conversion.running() {
                    self.conversion = Job::Idle;
                    self.focus = self.first_focusable();
                }
            }
            Message::AskStopConversion => {
                if self.conversion.running() {
                    self.popup = Some(Popup::StopConversion);
                    self.popup_focus = 0;
                }
            }
            Message::CancelConversion => {
                self.popup = None;
                self.conversion.cancel();
            }

            // Copying.
            Message::ChooseArchive => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("XWA archive", &["xwa"])
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::ArchiveChosen,
                );
            }
            Message::ArchiveChosen(Some(path)) if !self.copy.running() => {
                self.archive = Some(path);
                self.copy = Job::Idle;
            }
            Message::ArchiveChosen(_) => {}
            Message::LookForCards => self.refresh_drives(),
            Message::DriveChosen(drive) => self.drive = Some(drive),
            Message::CopyToCard => return self.start_copy(),
            Message::Copying(Event::Progress(p)) => self.copy.progress(p),
            Message::Copying(Event::Done(result)) => {
                self.copy = Job::Finished(match result {
                    Ok(path) => Ok(format!(
                        "Copied to {}. Eject the card, then put it back in your e-reader.",
                        path.display()
                    )),
                    Err(e) if e == "cancelled" => Err("Copy stopped.".into()),
                    Err(e) => Err(e),
                });
                self.focus = self.first_focusable();
            }
            Message::CancelCopy => self.copy.cancel(),

            // The library.
            Message::LoadLibrary => return self.load_library(),
            Message::LanguagesLoaded(Ok(languages)) => {
                let wanted = self.prefs.language.clone().unwrap_or_else(|| "eng".into());
                self.language = languages
                    .iter()
                    .find(|l| l.code == wanted)
                    .or_else(|| languages.iter().find(|l| l.code == "eng"))
                    .or(languages.first())
                    .cloned();
                self.languages = languages;
                return self.load_books();
            }
            Message::LanguagesLoaded(Err(e)) => {
                self.library_status = Some(format!("Could not reach the Kiwix library: {e}"));
                self.library_failed = true;
            }
            Message::LanguageChosen(language) => {
                self.prefs.language = Some(language.code.clone());
                self.prefs.save();
                self.language = Some(language);
                self.language_search.clear();
                self.back();
                return self.load_books();
            }
            Message::BooksLoaded(Ok(books)) => {
                self.library_status = books
                    .is_empty()
                    .then(|| "The library has no Wikipedia in this language.".into());
                self.books = sort_books(books);
                self.library_failed = false;
            }
            Message::BooksLoaded(Err(e)) => {
                self.library_status = Some(format!("Could not load the list: {e}"));
                self.library_failed = true;
            }
            Message::Search(s) => {
                self.search = s;
                // The first match is where Enter goes.
                self.focus = 1;
            }
            Message::LanguageSearch(s) => {
                self.language_search = s;
                self.focus = 0;
            }
            Message::Submit => return self.press(Key::Confirm),
            Message::OpenBook(book) => {
                self.book = Some(book);
                self.go(Screen::Book);
            }
            Message::Download => return self.start_download(),
            Message::Downloading(Event::Progress(p)) => self.download.progress(p),
            Message::Downloading(Event::Done(Ok(path))) => {
                self.download = Job::Finished(Ok(format!("Downloaded {}.", path.display())));
                self.downloading = None;
                // On to converting it, unless something else is converting.
                if !self.conversion.running() {
                    return self.update(Message::ZimChosen(Some(path)));
                }
            }
            Message::Downloading(Event::Done(Err(e))) => {
                self.download = Job::Finished(Err(if e == "cancelled" {
                    "Download paused. Download again to carry on from where it stopped.".into()
                } else {
                    e
                }));
                self.downloading = None;
            }
            Message::CancelDownload => self.download.cancel(),
        }
        Task::none()
    }

    /// Work a screen needs when it is opened.
    fn entered(&mut self) -> Task<Message> {
        match self.screen {
            Screen::Library if self.languages.is_empty() && !self.library_failed => {
                self.load_library()
            }
            Screen::Library | Screen::Languages => operation::focus(SEARCH_ID),
            Screen::Card => {
                self.refresh_drives();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn refresh_drives(&mut self) {
        self.drives = xwa_fetch::drives::removable();
        if self.drive.as_ref().is_none_or(|d| !self.drives.contains(d)) {
            self.drive = self.drives.first().cloned();
        }
    }

    fn load_library(&mut self) -> Task<Message> {
        self.library_status = Some("Loading the library…".into());
        self.library_failed = false;
        Task::perform(
            background(|| {
                xwa_fetch::Client::new()
                    .languages()
                    .map_err(|e| e.to_string())
            }),
            Message::LanguagesLoaded,
        )
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

    fn start_conversion(&mut self, out: PathBuf) -> Task<Message> {
        let Some(zim) = self.zim.clone() else {
            return Task::none();
        };
        if out == zim {
            self.conversion = Job::Finished(Err(
                "Choose a different name: that would replace the ZIM file.".into(),
            ));
            return Task::none();
        }
        let (job, cancel) = Job::start();
        self.conversion = job;
        self.output = Some(out.clone());
        self.focus = 0;
        let device = self.prefs.model.converter();
        let images = self.images;
        let diagrams = (self.images && self.sharper)
            .then(Diagrams::default_cache)
            .flatten()
            .map(|cache| Arc::new(Diagrams::new(cache)));
        let sharper = diagrams.is_some();
        self.converting_sharper = sharper;
        self.converting_images = images;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let opts = xwa_convert::Options {
                images,
                diagrams,
                ..xwa_convert::Options::new(device)
            };
            let mut throttle = Throttle::default();
            let result = xwa_convert::convert(&zim, &out, &opts, &mut |p| {
                if throttle.due(p.stage as usize) {
                    let _ =
                        tx.unbounded_send(Event::Progress(conversion_progress(p, images, sharper)));
                }
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

    fn start_copy(&mut self) -> Task<Message> {
        let (Some(file), Some(drive)) = (self.archive.clone(), self.drive.clone()) else {
            return Task::none();
        };
        let (job, cancel) = Job::start();
        self.copy = job;
        self.focus = 0;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let dir = xwa_fetch::drives::archive_dir(&drive.mount);
            let mut throttle = Throttle::default();
            let result = xwa_fetch::drives::copy(&file, &dir, Some(&drive), &mut |p| {
                if throttle.due(0) {
                    let fraction = p.done as f32 / p.total.max(1) as f32;
                    let label = format!("{} of {}", size_text(p.done), size_text(p.total));
                    let _ = tx.unbounded_send(Event::Progress(Progress {
                        label,
                        fraction,
                        step: 0,
                    }));
                }
                !cancel.load(Ordering::Relaxed)
            });
            let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
        });
        Task::run(rx, Message::Copying)
    }

    fn start_download(&mut self) -> Task<Message> {
        let Some(book) = self.book.clone() else {
            return Task::none();
        };
        if self.download.running() {
            return Task::none();
        }
        let (job, cancel) = Job::start();
        self.download = job;
        self.downloading = Some(book.clone());
        let dir = self.download_dir();
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut throttle = Throttle::default();
            let result = xwa_fetch::Client::new().download(&book.url, &dir, &mut |p| {
                if !throttle.due(p.stage as usize) {
                    return !cancel.load(Ordering::Relaxed);
                }
                let fraction = p.done as f32 / p.total.max(1) as f32;
                let (label, step) = match p.stage {
                    xwa_fetch::Stage::Downloading => (
                        format!("{} of {}", size_text(p.done), size_text(p.total)),
                        0,
                    ),
                    xwa_fetch::Stage::Checking => ("Checking the download…".to_string(), 1),
                };
                let _ = tx.unbounded_send(Event::Progress(Progress {
                    label,
                    fraction,
                    step,
                }));
                !cancel.load(Ordering::Relaxed)
            });
            let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
        });
        Task::run(rx, Message::Downloading)
    }

    // -----------------------------------------------------------------------
    // Events

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            event::listen_with(|event, status, _window| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    Some(Message::Opened(path))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    keyboard_message(&key, modifiers, status)
                }
                _ => None,
            }),
            window::close_requests().map(|_| Message::CloseRequested),
            iced::time::every(Duration::from_secs(5)).map(|_| Message::Clock),
        ];
        if self.flash.is_some() || self.tapped_key.is_some() {
            subs.push(window::frames().map(Message::Frame));
        }
        Subscription::batch(subs)
    }

    /// The key shown pressed on the device.
    pub fn shown_pressed(&self) -> Option<Key> {
        self.pressed_key.or(self.tapped_key.map(|(k, _)| k))
    }

    /// The flash covering the screen, if a refresh is under way.
    pub fn flash_colour(&self) -> Option<iced::Color> {
        let elapsed = self.flash?.elapsed();
        if elapsed < FLASH_BLACK {
            Some(eink::INK)
        } else if elapsed < FLASH_WHITE {
            Some(eink::PAPER)
        } else {
            None
        }
    }
}

/// Lets progress through at most ten times a second, and whenever the step
/// changes: workers report far more often than a screen can show, and each
/// report redraws the window.
#[derive(Default)]
struct Throttle {
    last: Option<(Instant, usize)>,
}

impl Throttle {
    fn due(&mut self, step: usize) -> bool {
        let now = Instant::now();
        let due = self.last.is_none_or(|(at, s)| {
            s != step || now.duration_since(at) >= Duration::from_millis(100)
        });
        if due {
            self.last = Some((now, step));
        }
        due
    }
}

fn keyboard_message(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    status: event::Status,
) -> Option<Message> {
    use keyboard::key::Named;
    if modifiers.command() {
        return match key.as_ref() {
            keyboard::Key::Character("q") => Some(Message::CloseRequested),
            keyboard::Key::Character("=" | "+") => Some(Message::Zoom(1)),
            keyboard::Key::Character("-") => Some(Message::Zoom(-1)),
            keyboard::Key::Character("0") => Some(Message::Zoom(0)),
            keyboard::Key::Character("m") => Some(Message::Sleep),
            _ => None,
        };
    }
    // Keys a text field used stay with it.
    if status == event::Status::Captured {
        return None;
    }
    let pressed = match key.as_ref() {
        keyboard::Key::Named(Named::ArrowUp) => Key::Left,
        keyboard::Key::Named(Named::ArrowDown) => Key::Right,
        keyboard::Key::Named(Named::ArrowLeft | Named::PageUp) => Key::Up,
        keyboard::Key::Named(Named::ArrowRight | Named::PageDown) => Key::Down,
        keyboard::Key::Named(Named::Enter | Named::Space) => Key::Confirm,
        keyboard::Key::Named(Named::Escape | Named::Backspace) => Key::Back,
        keyboard::Key::Named(Named::Home) => Key::Home,
        _ => return None,
    };
    Some(Message::Press(pressed))
}

fn clock() -> String {
    chrono::Local::now().format("%H:%M").to_string()
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

/// The whole Wikipedia (names ending in "_all") first, then selections by
/// title; within a title the most complete first, then the newest.
fn sort_books(mut books: Vec<Book>) -> Vec<Book> {
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
                x.title.to_lowercase(),
                flavour_rank(&x.flavour),
            )
        };
        key(a).cmp(&key(b)).then_with(|| b.date.cmp(&a.date))
    });
    books
}

/// The steps of a conversion with these options, as shown while it runs.
pub fn conversion_steps(images: bool, sharper: bool) -> Vec<&'static str> {
    let mut steps = vec!["Reading the ZIM file", "Converting articles"];
    if sharper {
        steps.push("Downloading sharper diagrams");
    }
    if images {
        steps.push("Converting pictures");
    }
    steps.push("Finishing");
    steps
}

fn conversion_progress(p: xwa_convert::Progress, images: bool, sharper: bool) -> Progress {
    let steps = conversion_steps(images, sharper);
    let name = match p.stage {
        Stage::Scanning => "Reading the ZIM file",
        Stage::Articles => "Converting articles",
        Stage::Diagrams => "Downloading sharper diagrams",
        Stage::Images => "Converting pictures",
        Stage::Finishing => "Finishing",
    };
    let step = steps.iter().position(|s| *s == name).unwrap_or(0);
    let part = if p.total == 0 {
        1.0
    } else {
        p.done as f32 / p.total as f32
    };
    // Rough shares of the time: reading 5%, articles 55%, pictures 35%,
    // finishing 5%; downloading diagrams, when asked for, takes 30% from
    // articles and pictures.
    let (articles, diagrams) = if sharper { (0.35, 0.30) } else { (0.55, 0.0) };
    let pictures = 0.90 - articles - diagrams;
    let fraction = match p.stage {
        Stage::Scanning => 0.05 * part,
        Stage::Articles => 0.05 + articles * part,
        Stage::Diagrams => 0.05 + articles + diagrams * part,
        Stage::Images => 0.05 + articles + diagrams + pictures * part,
        Stage::Finishing => 0.95 + 0.05 * part,
    };
    let label = if p.total > 0 && p.stage != Stage::Finishing {
        format!("{} of {}", group(p.done), group(p.total))
    } else {
        String::new()
    };
    Progress {
        label,
        fraction,
        step,
    }
}

/// A count with thousands separated: 1,234,567.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn summary(r: &xwa_convert::Report, out: &Path, device: xwa_convert::Device) -> String {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pictures = match r.summary.images {
        0 => "no pictures".to_string(),
        1 => "1 picture".to_string(),
        n => format!("{} pictures", group(n as u64)),
    };
    let mut done = format!(
        "{} articles and {pictures} in {name} ({}), made for the {}.",
        group(r.summary.articles as u64),
        size_text(r.summary.file_size),
        device.name()
    );
    if let Some(d) = &r.diagrams {
        done.push_str(&format!(
            " {} diagrams are sharper copies from Wikipedia.",
            group(r.sharper_images)
        ));
        if let Some(why) = &d.stopped {
            done.push_str(&format!(" Some could not be downloaded: {why}."));
        }
    }
    done
}

/// What the ZIM's header and metadata say (no full scan), with its
/// illustration dithered for the panel.
pub(crate) fn describe(path: &Path) -> Result<ZimInfo, String> {
    let zim = xwa_zim::Archive::open(path).map_err(|e| e.to_string())?;
    let meta = |name: &str| -> Option<Vec<u8>> { zim.metadata(name).ok().flatten() };
    let text = |name: &str| -> String {
        meta(name)
            .map(|v| String::from_utf8_lossy(&v).trim().to_string())
            .unwrap_or_default()
    };
    let cover = meta("Illustration_48x48@1").and_then(|png| eink::panel_picture(&png));
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let file_title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = Some(text("Title"))
        .filter(|t| !t.is_empty())
        .unwrap_or(file_title);
    Ok(ZimInfo {
        title,
        language: text("Language"),
        flavour: text("Flavour"),
        date: text("Date"),
        entries: zim.entry_count(),
        size,
        cover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_with_separators() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(6_912_345), "6,912,345");
    }

    #[test]
    fn conversion_steps_follow_the_options() {
        assert_eq!(conversion_steps(false, false).len(), 3);
        assert_eq!(conversion_steps(true, true).len(), 5);
        let p = conversion_progress(
            xwa_convert::Progress {
                stage: Stage::Images,
                done: 5,
                total: 10,
            },
            true,
            false,
        );
        assert_eq!(p.step, 2);
        assert!((p.fraction - (0.05 + 0.55 + 0.35 * 0.5)).abs() < 1e-6);
    }
}
