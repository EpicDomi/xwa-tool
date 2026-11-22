//! The app's state and what each message does to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iced::widget::operation;
use iced::{Point, Size, Subscription, Task, event, keyboard, mouse, window};
use xwa_convert::Stage;
use xwa_fetch::drives::Drive;
use xwa_fetch::fonts::Fonts;
use xwa_fetch::wikimedia::Diagrams;
use xwa_fetch::{Book, Language, size_text};
use xwa_format::font::FontChars;

use crate::device::{Finish, Key, Model};
use crate::downloads::{self, Download, Fetch, Queue, Rate};
use crate::eink;
use crate::graphics::{self, Graphics};
use crate::library::{View, Wiki};
use crate::prefs::{self, Prefs};

/// Zoom steps, in desktop pixels per screen pixel.
pub const ZOOMS: [f32; 10] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6];
/// The zoom the window opens at before the monitor's size is known: small
/// enough for any screen.
pub const FIRST_ZOOM: f32 = 0.7;
/// How much of the monitor's height the device may take when fitted.
const FIT: f32 = 0.92;
/// How many times to ask for the monitor's size once the window is up.
const MONITOR_TRIES: u8 = 10;
/// The smallest the device can be drawn.
pub const MIN_ZOOM: f32 = 0.4;
/// How long after the last resize the window settles round the device.
const SETTLE: Duration = Duration::from_millis(300);
/// How long size reports may lag behind the app's own resizing.
const OWN_RESIZE: Duration = Duration::from_millis(700);

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
    /// How the library's list is filtered and sorted.
    Filter,
    Downloads,
    /// Pictures of real italic beside slanted regular letters.
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    Power,
    /// Turning off would stop work in progress.
    TurnOffBusy,
    /// Stopping a conversion throws away the work so far.
    StopConversion,
    /// Where the archive is to be saved lacks the space for reading the
    /// ZIM once: what it needs and what it has.
    NoRoomToReadOnce {
        needs: u64,
        has: u64,
    },
    /// Picking one of a setting's values.
    Choose(Choice),
    /// What can be done with one of the downloads.
    Download(usize),
}

/// A setting chosen from a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Device,
    Case,
    Size,
    Graphics,
    Wiki,
    Pictures,
    Selection,
    Articles,
    Sort,
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
        /// Bytes a second, for downloads and copies.
        rate: Option<f64>,
        /// Time left, when the worker can tell from its speed.
        left: Option<Duration>,
    },
    Finished(Result<String, String>),
}

impl Job {
    pub(crate) fn start() -> (Job, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        (
            Job::Running {
                label: "Starting…".into(),
                fraction: 0.0,
                step: 0,
                cancel: cancel.clone(),
                started: Instant::now(),
                rate: None,
                left: None,
            },
            cancel,
        )
    }

    pub fn label(&self) -> &str {
        match self {
            Job::Running { label, .. } => label,
            _ => "",
        }
    }

    pub fn running(&self) -> bool {
        matches!(self, Job::Running { .. })
    }

    /// Asked to stop, and finishing the work in hand.
    pub fn stopping(&self) -> bool {
        matches!(self, Job::Running { cancel, .. } if cancel.load(Ordering::Relaxed))
    }

    pub(crate) fn cancel(&mut self) {
        if let Job::Running { cancel, label, .. } = self {
            cancel.store(true, Ordering::Relaxed);
            *label = "Stopping…".into();
        }
    }

    pub(crate) fn progress(&mut self, p: Progress) {
        if let Job::Running {
            label,
            fraction,
            step,
            cancel,
            rate,
            left,
            ..
        } = self
            && !cancel.load(Ordering::Relaxed)
        {
            *label = p.label;
            *fraction = p.fraction;
            *step = p.step;
            *rate = p.rate;
            *left = p.left;
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
            fraction,
            started,
            left,
            ..
        } = self
        else {
            return None;
        };
        if left.is_some() {
            return *left;
        }
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
    /// How much of it is pages and media, for estimating the space a
    /// conversion needs.
    pub shares: Option<xwa_convert::ZimShares>,
    /// Its illustration in the panel's greys.
    pub cover: Option<iced::widget::image::Handle>,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub label: String,
    pub fraction: f32,
    pub step: usize,
    pub rate: Option<f64>,
    pub left: Option<Duration>,
}

impl Progress {
    /// Bytes moved: how many of how many, how fast, and so how long is left.
    fn bytes(done: u64, total: u64, rate: Option<f64>, step: usize) -> Progress {
        let mut label = format!("{} of {}", size_text(done), size_text(total));
        if let Some(r) = rate {
            label += &format!(" · {}", downloads::speed_text(r));
        }
        Progress {
            label,
            fraction: done as f32 / total.max(1) as f32,
            step,
            rate,
            left: rate
                .filter(|r| *r > 0.0)
                .map(|r| Duration::from_secs_f64(total.saturating_sub(done) as f64 / r)),
        }
    }
}

/// The zoom at which a device with this `window` fills as much of `monitor`
/// as it may; with no monitor known, the largest step.
pub fn fit_zoom_for(monitor: Option<Size>, window: Size) -> f32 {
    let Some(monitor) = monitor else {
        return ZOOMS[ZOOMS.len() - 1];
    };
    (FIT * monitor.height / window.height)
        .min(FIT * monitor.width / window.width)
        .max(MIN_ZOOM)
}

/// The largest zoom step no bigger than `fit`.
fn step_below(fit: f32) -> f32 {
    ZOOMS
        .iter()
        .copied()
        .rfind(|&z| z <= fit + 1e-3)
        .unwrap_or(ZOOMS[0])
}

/// The zoom to use: the one `chosen`, or the biggest step up to life size
/// that fits; never more than fits.
pub fn wanted_zoom_for(chosen: Option<f32>, monitor: Option<Size>, window: Size) -> f32 {
    let fit = fit_zoom_for(monitor, window);
    chosen
        .unwrap_or(step_below(fit).min(1.0))
        .clamp(MIN_ZOOM, fit)
}

/// The zoom the window opens at: the one wanted on the monitor it was on
/// last time, or, the very first time, a small one until the monitor's size
/// is known.
pub fn start_zoom(prefs: &Prefs) -> f32 {
    match prefs.monitor {
        Some((w, h)) => wanted_zoom_for(
            prefs.zoom,
            Some(Size::new(w, h)),
            prefs.model.spec().window(),
        ),
        None => prefs.zoom.unwrap_or(FIRST_ZOOM),
    }
}

/// Messages from a worker thread.
#[derive(Debug, Clone)]
pub enum Event<T> {
    Progress(Progress),
    Done(Result<T, String>),
}

/// How many of the fallback font's Noto font files are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontsKept {
    pub kept: usize,
    pub total: usize,
    /// The size of the rest.
    pub missing_bytes: u64,
}

impl FontsKept {
    /// Looks in the font cache.
    pub fn count() -> Option<FontsKept> {
        let fonts = Fonts::new(Fonts::default_cache()?);
        let files = xwa_convert::fallback::all_files();
        let missing: Vec<_> = files
            .iter()
            .filter(|f| !fonts.is_cached(&f.sha256))
            .collect();
        Some(FontsKept {
            kept: files.len() - missing.len(),
            total: files.len(),
            missing_bytes: missing.iter().map(|f| f.size).sum(),
        })
    }

    pub fn all(&self) -> bool {
        self.kept == self.total
    }
}

/// What the app knows of the chosen archive's fallback font (spec 10).
#[derive(Debug, Clone, Default)]
pub enum ArchiveFont {
    /// Not looked at yet.
    #[default]
    Unknown,
    /// Every glyph the Noto fonts have for its text.
    Complete,
    /// Glyphs the Noto fonts could add, and the characters to build for.
    Missing {
        glyphs: usize,
        chars: Arc<FontChars>,
    },
    /// Made before archives listed their characters: finding out means
    /// reading its text.
    Unchecked,
    /// No fallback font, or the archive could not be read.
    None,
}

/// How complete `archive`'s fallback font is, from its CHARS section;
/// without one, [`ArchiveFont::Unchecked`] when it has a FONT to check.
pub(crate) fn archive_font(path: &Path) -> ArchiveFont {
    let look = || -> Result<ArchiveFont, xwa_format::Error> {
        let archive = xwa_format::reader::Archive::open(path)?;
        match archive.font_chars()? {
            Some(chars) => Ok(font_state(&archive, chars)?),
            None if archive.font()?.is_some() => Ok(ArchiveFont::Unchecked),
            None => Ok(ArchiveFont::None),
        }
    };
    look().unwrap_or(ArchiveFont::None)
}

/// Reads an older archive's text (spec 10.2) to see what its font lacks.
fn check_font(
    path: &Path,
    send: &mut dyn FnMut(String, u64, u64) -> bool,
) -> Result<(ArchiveFont, String), String> {
    let archive = xwa_format::reader::Archive::open(path).map_err(|e| e.to_string())?;
    let mut stopped = false;
    let chars = archive
        .scan_font_chars(&mut |done, total| {
            let label = format!("Reading articles: {} of {}", group(done), group(total));
            stopped |= !send(label, done, total);
        })
        .map_err(|e| e.to_string())?;
    if stopped {
        return Err("cancelled".into());
    }
    let state = font_state(&archive, chars).map_err(|e| e.to_string())?;
    let said = match &state {
        ArchiveFont::Missing { glyphs, .. } => format!(
            "Its fallback font lacks {} glyphs the Noto fonts have.",
            group(*glyphs as u64)
        ),
        _ => "Its fallback font has every glyph it needs.".into(),
    };
    Ok((state, said))
}

/// Gets the glyphs `path`'s font lacks and puts them in the archive.
fn complete_font(
    path: &Path,
    chars: &FontChars,
    fonts: &Fonts,
    send: &mut dyn FnMut(String, u64, u64) -> bool,
) -> Result<(ArchiveFont, String), String> {
    let report = xwa_convert::rebuild_font(path, chars, fonts, &mut |p| {
        let label = format!("Fonts: {} of {}", group(p.done), group(p.total));
        send(label, p.done, p.total)
    })
    .map_err(|e| e.to_string())?;
    let state = archive_font(path);
    let said = match &state {
        ArchiveFont::Complete => "Its fallback font now has every glyph it needs.".to_string(),
        ArchiveFont::Missing { glyphs, .. } => {
            let why = report
                .failed
                .first()
                .map(|(_, why)| format!(" ({why})"))
                .unwrap_or_default();
            format!(
                "{} glyphs are still missing: some Noto fonts could not be downloaded{why}.",
                group(*glyphs as u64)
            )
        }
        _ => "The archive could not be read again.".into(),
    };
    Ok((state, said))
}

fn font_state(
    archive: &xwa_format::reader::Archive,
    chars: FontChars,
) -> Result<ArchiveFont, xwa_format::Error> {
    let status = xwa_convert::font_status(archive, &chars)
        .map_err(|e| xwa_format::Error::Invalid(e.to_string()))?;
    Ok(if status.complete() {
        ArchiveFont::Complete
    } else {
        ArchiveFont::Missing {
            glyphs: status.missing + status.missing_styled,
            chars: Arc::new(chars),
        }
    })
}

#[derive(Debug, Clone)]
pub enum Message {
    // The window and the device.
    Window(Option<window::Id>),
    Monitor(Option<Size>),
    Placed(Option<Point>),
    /// The window's new size, in this window's units.
    Resized(Size),
    /// A resize handle was pressed: the system takes over the drag.
    ResizeFrom(window::Direction),
    Settle,
    SettleUnlessMaximized(bool),
    /// Sizes the window for the device at the current zoom.
    ApplySize,
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
    SetGraphics(Graphics),
    ToggleFlash,
    ChooseDownloadDir,
    DownloadDirChosen(Option<PathBuf>),
    /// A file dialog has closed, with what it gives on to.
    DialogClosed(Box<Message>),
    /// Asks again which monitor the window is on.
    FindMonitor,
    /// A file opened with the app or dropped on it.
    Opened(PathBuf),
    // Converting.
    ChooseZim,
    ZimChosen(Option<PathBuf>),
    ZimRead(PathBuf, Result<ZimInfo, String>),
    ToggleImages,
    ToggleSharper,
    ToggleItalic,
    ToggleOneRead,
    /// Convert to the chosen place, reading the ZIM twice after all.
    ConvertReadingTwice,
    /// The next (1) or previous (-1) picture of italic.
    ItalicPicture(isize),
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
    // Fonts.
    /// Download every Noto font the fallback font could need, or stop.
    DownloadFonts,
    FontDownload(Event<String>),
    /// What looking at an archive's fallback font found.
    ArchiveFontRead(PathBuf, ArchiveFont),
    /// Read an older archive's text to find what its font lacks.
    CheckFont,
    /// Get the glyphs the archive's font lacks and put them in it.
    CompleteFont,
    FontWork(Event<(ArchiveFont, String)>),
    CancelFontWork,
    // The Kiwix library.
    LoadLibrary,
    LanguagesLoaded(Result<Vec<Language>, String>),
    LanguageChosen(Language),
    BooksLoaded(Result<Vec<Book>, String>),
    Search(String),
    SetWiki(Wiki),
    SetView(View),
    /// A page of the list, from its scroll bar.
    ShowPage(usize),
    LanguageSearch(String),
    Submit,
    OpenBook(Book),
    /// Adds the book on screen to the downloads.
    Download,
    /// From the worker of a download's run.
    Downloading(u64, Event<PathBuf>),
    PauseDownload(String),
    ResumeDownload(String),
    RemoveDownload(String),
    /// What can be done with a download, in a popup.
    DownloadMenu(usize),
}

pub struct App {
    pub prefs: Prefs,
    pub window: Option<window::Id>,
    pub zoom: f32,
    /// The monitor's size in desktop pixels, once known.
    monitor: Option<Size>,
    retried_monitor: bool,
    /// The edge being dragged to resize the window, when one of ours.
    resizing: Option<window::Direction>,
    /// When the window, resized, is to be fitted round the device again.
    pub(crate) settle_at: Option<Instant>,
    /// Until when size reports are taken as the echo of the app's own
    /// resizing (they can lag behind it), rather than the user's.
    pub(crate) own_resize_until: Option<Instant>,
    /// How many times the device's size was asked for again, after the
    /// system answered the app's resize with another size.
    insisted: u8,
    /// How many times the monitor's size has been asked for, on systems
    /// (Wayland) that only know it some time after the window is up.
    monitor_tries: u8,
    /// A file dialog is open, or opening: Windows can take a while to show
    /// one, and a second click would open another.
    dialog_open: bool,
    /// Whether the window has been shown.
    pub(crate) shown: bool,
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
    /// How this launch draws, and whether that has been seen to work.
    pub graphics: Graphics,
    shown_at: Option<Instant>,
    graphics_confirmed: bool,

    // Converting.
    pub zim: Option<PathBuf>,
    pub zim_info: Option<Result<ZimInfo, String>>,
    pub images: bool,
    pub sharper: bool,
    /// Real italic letters in the fallback font.
    pub real_italic: bool,
    /// Read the ZIM once, with a temporary file of articles.
    pub one_read: bool,
    /// Free space where the archive would be saved by default, and where.
    pub save_free: Option<(PathBuf, u64)>,
    /// A place chosen for the archive, waiting on a question.
    pub pending_output: Option<PathBuf>,
    /// Which picture the italic screen shows.
    pub italic_picture: usize,
    pub conversion: Job,
    /// Whether the running conversion fetches sharper diagrams.
    pub converting_sharper: bool,
    /// Whether it reads the ZIM once.
    pub converting_one_read: bool,
    pub converting_images: bool,
    pub output: Option<PathBuf>,

    /// The fallback font's Noto fonts kept for converting offline.
    pub fonts_kept: Option<FontsKept>,
    pub font_download: Job,

    // Copying.
    pub archive: Option<PathBuf>,
    /// How complete the archive's fallback font is.
    pub archive_font: ArchiveFont,
    /// Checking or completing it.
    pub font_work: Job,
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
    pub downloads: Queue,
}

impl App {
    pub fn new(
        prefs: Prefs,
        zoom: f32,
        open: Option<PathBuf>,
        graphics: Graphics,
    ) -> (Self, Task<Message>) {
        let mut app = App {
            prefs,
            graphics,
            shown_at: None,
            graphics_confirmed: false,
            window: None,
            zoom,
            monitor: None,
            retried_monitor: false,
            resizing: None,
            settle_at: None,
            own_resize_until: None,
            insisted: 0,
            monitor_tries: 0,
            dialog_open: false,
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
            real_italic: true,
            one_read: false,
            save_free: None,
            pending_output: None,
            italic_picture: 0,
            conversion: Job::Idle,
            converting_sharper: false,
            converting_one_read: false,
            converting_images: true,
            output: None,
            fonts_kept: None,
            font_download: Job::Idle,
            archive: None,
            archive_font: ArchiveFont::Unknown,
            font_work: Job::Idle,
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
            downloads: Queue::default(),
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
        self.conversion.running() || self.copy.running() || self.downloads.is_running()
    }

    /// What is running, for the status line.
    pub fn status(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(f) = self.conversion.fraction() {
            parts.push(format!("Converting {:.0}%", f * 100.0));
        }
        if let Some(f) = self.downloads.running().and_then(Download::fraction) {
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
            if self.copy.running() || self.font_work.running() {
                return Task::none();
            }
            let look = self.use_archive(path);
            self.refresh_drives();
            self.go(Screen::Card);
            look
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

    /// Goes back to the screen before, and does the work it needs, such as
    /// putting the cursor back in its search field.
    fn back(&mut self) -> Task<Message> {
        match self.history.pop() {
            Some((screen, focus)) => self.show(screen, focus),
            None if self.screen != Screen::Home => self.show(Screen::Home, 0),
            None => return Task::none(),
        }
        self.entered()
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
        if self.screen == Screen::Italic {
            self.turn_italic(step);
            return;
        }
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

    fn turn_italic(&mut self, step: isize) {
        let n = crate::screens::ITALIC_PICTURES.len() as isize;
        self.italic_picture = (self.italic_picture as isize + step).rem_euclid(n) as usize;
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
            if n == 0 {
                self.popup = None;
                return Task::none();
            }
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
            Key::Back => return self.back(),
            // The italic pictures turn like pages, whichever way.
            Key::Left | Key::Up if self.screen == Screen::Italic => self.turn_italic(-1),
            Key::Right | Key::Down | Key::Confirm if self.screen == Screen::Italic => {
                self.turn_italic(1)
            }
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

    /// Sizes the window round the device at this zoom.
    fn resize(&mut self) -> Task<Message> {
        self.insisted = 0;
        self.size_window()
    }

    fn size_window(&mut self) -> Task<Message> {
        match self.window {
            Some(id) => {
                self.own_resize_until = Some(Instant::now() + OWN_RESIZE);
                window::resize(id, self.spec().window())
            }
            None => Task::none(),
        }
    }

    /// Opens a file dialog, unless one is open already, and passes on what
    /// it gives.
    fn dialog(
        &mut self,
        dialog: impl std::future::Future<Output = Option<PathBuf>> + Send + 'static,
        then: fn(Option<PathBuf>) -> Message,
    ) -> Task<Message> {
        if self.dialog_open {
            return Task::none();
        }
        self.dialog_open = true;
        Task::perform(dialog, move |path| {
            Message::DialogClosed(Box::new(then(path)))
        })
    }

    /// The largest zoom step at which the device fits the monitor.
    /// The zoom at which the device fills as much of the monitor as it may.
    pub fn fit_zoom(&self) -> f32 {
        fit_zoom_for(self.monitor, self.spec().window())
    }

    /// The largest zoom step at which the device fits the monitor.
    pub fn max_zoom(&self) -> f32 {
        step_below(self.fit_zoom())
    }

    /// The zoom to use: the one chosen, or the biggest step up to life size
    /// that fits; never more than fits.
    fn wanted_zoom(&self) -> f32 {
        wanted_zoom_for(self.prefs.zoom, self.monitor, self.spec().window())
    }

    /// Changes the zoom and resizes the window to match.
    fn zoom_to(&mut self, zoom: f32) -> Task<Message> {
        if (zoom - self.zoom).abs() < 1e-3 {
            return Task::none();
        }
        self.zoom = zoom;
        Task::done(Message::ApplySize)
    }

    /// Changes the zoom by choice, and remembers it.
    fn choose_zoom(&mut self, zoom: f32) -> Task<Message> {
        self.prefs.zoom = Some(zoom.clamp(MIN_ZOOM, self.fit_zoom()));
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
        self.shown_at = Some(Instant::now());
        if self.prefs.refresh_flash {
            self.flash = Some(Instant::now());
        }
        Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::gain_focus(id),
        ])
        // A message after showing makes sure the first frame is drawn now.
        .chain(Task::done(Message::Clock))
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
                let monitor = Size::new(size.width * self.zoom, size.height * self.zoom);
                self.monitor = Some(monitor);
                // Remembered, so the window opens at its size next time.
                let remembered = Some((monitor.width.round(), monitor.height.round()));
                if self.prefs.monitor != remembered {
                    self.prefs.monitor = remembered;
                    self.prefs.save();
                }
                // A size being dragged to is the user's: the window is not
                // fitted to the monitor under them when it comes to be known
                // late (Wayland).
                let dragging = self.resizing.is_some() || self.settle_at.is_some();
                if dragging || (self.wanted_zoom() - self.zoom).abs() < 1e-3 {
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
                self.zoom = wanted;
                // The window is shown once it has its size.
                tasks.push(Task::done(Message::ApplySize));
                return Task::batch(tasks);
            }
            Message::ApplySize => {
                // A message of its own, so that iced has taken up the new
                // zoom before the size, given in the window's units, is
                // turned into pixels.
                let resize = self.resize();
                return if self.shown {
                    resize
                } else {
                    let show = self.show_window();
                    resize.chain(show)
                };
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
            Message::ResizeFrom(direction) => {
                self.resizing = Some(direction);
                if let Some(id) = self.window {
                    return window::drag_resize(id, direction);
                }
            }
            Message::Resized(size) => {
                // A minimised window has no size (Windows says 0 × 0):
                // nothing is being resized.
                if !self.shown || size.width < 1.0 || size.height < 1.0 {
                    return Task::none();
                }
                // In desktop pixels, against the device at this zoom.
                let window = self.spec().window();
                let (w, h) = (size.width * self.zoom, size.height * self.zoom);
                if (w - window.width * self.zoom).abs() <= 2.0
                    && (h - window.height * self.zoom).abs() <= 2.0
                {
                    return Task::none();
                }
                if self.resizing.is_none()
                    && self.own_resize_until.is_some_and(|t| Instant::now() < t)
                {
                    // Just after the app sized the window, another size is
                    // the system catching up with something earlier: KWin
                    // sends the size a drag ended at after the app has fitted
                    // the window round the device. The device's size is
                    // asked for again, twice at most.
                    if self.insisted < 2 {
                        self.insisted += 1;
                        return self.size_window();
                    }
                    return Task::none();
                }
                let (across, down) = (w / window.width, h / window.height);
                use window::Direction::{East, North, South, West};
                // The device follows the edge being dragged; anything else
                // scales it to fit.
                let zoom = match self.resizing {
                    Some(East | West) => across,
                    Some(North | South) => down,
                    Some(_) => across.max(down),
                    None => across.min(down),
                };
                self.zoom = zoom.clamp(MIN_ZOOM, self.fit_zoom());
                self.settle_at = Some(Instant::now() + SETTLE);
            }
            Message::Settle => {
                if self.settle_at.is_some_and(|t| Instant::now() >= t) {
                    self.settle_at = None;
                    self.resizing = None;
                    self.prefs.zoom = Some(self.zoom);
                    self.prefs.save();
                    if let Some(id) = self.window {
                        return window::is_maximized(id).map(Message::SettleUnlessMaximized);
                    }
                }
            }
            Message::SettleUnlessMaximized(maximized) => {
                // A maximised window keeps its size, the device in its middle.
                if !maximized {
                    return self.resize();
                }
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
            Message::Clock => {
                self.clock = clock();
                // Drawing has worked for a few seconds: keep this way.
                if !self.graphics_confirmed
                    && self
                        .shown_at
                        .is_some_and(|t| t.elapsed() >= Duration::from_secs(4))
                {
                    self.graphics_confirmed = true;
                    graphics::confirm(self.graphics);
                }
            }
            Message::FindMonitor => {
                if self.monitor.is_none() && self.monitor_tries < MONITOR_TRIES {
                    self.monitor_tries += 1;
                    if let Some(id) = self.window {
                        return window::monitor_size(id).map(Message::Monitor);
                    }
                }
            }
            Message::CloseRequested => {
                if self.busy() {
                    self.popup = Some(Popup::TurnOffBusy);
                    self.popup_focus = 0;
                } else {
                    graphics::confirm(self.graphics);
                    return iced::exit();
                }
            }
            Message::TurnOff => {
                self.conversion.cancel();
                self.copy.cancel();
                for d in &mut self.downloads.items {
                    if let Fetch::Running { job, .. } = &mut d.state {
                        job.cancel();
                    }
                }
                graphics::confirm(self.graphics);
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
                if screen == Screen::Italic {
                    self.italic_picture = 0;
                }
                self.go(screen);
                return self.entered();
            }
            Message::Back => return self.back(),
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
                    return Task::done(Message::ApplySize);
                }
            }
            Message::SetGraphics(g) => {
                self.popup = None;
                self.prefs.graphics = g;
                self.prefs.save();
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
                return self.dialog(
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
            Message::DialogClosed(then) => {
                self.dialog_open = false;
                return self.update(*then);
            }
            Message::DownloadDirChosen(Some(dir)) => {
                self.prefs.download_dir = Some(dir);
                self.prefs.save();
            }
            Message::DownloadDirChosen(None) => {}

            // Converting.
            Message::ChooseZim => {
                return self.dialog(
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
                self.popup = None;
                self.go(Screen::Convert);
                return self.use_zim(path);
            }
            Message::ZimChosen(_) => {}
            Message::ZimRead(path, info) => {
                if self.zim.as_ref() == Some(&path) {
                    self.zim_info = Some(info);
                    self.refresh_save_free();
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
            Message::ToggleItalic => {
                if !self.conversion.running() {
                    self.real_italic = !self.real_italic;
                }
            }
            Message::ToggleOneRead => {
                if !self.conversion.running() && self.one_read_fits() != Some(false) {
                    self.one_read = !self.one_read;
                }
            }
            Message::ConvertReadingTwice => {
                self.popup = None;
                if let Some(out) = self.pending_output.take() {
                    return self.start_conversion(out, false);
                }
            }
            Message::ItalicPicture(step) => self.turn_italic(step),
            Message::Convert => {
                let Some(zim) = self.zim.clone() else {
                    return Task::none();
                };
                let name = zim
                    .with_extension("xwa")
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir = self.save_dir();
                return self.dialog(
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
            Message::OutputChosen(Some(out)) => {
                if let Some(dir) = out.parent() {
                    self.prefs.save_dir = Some(dir.to_path_buf());
                    self.prefs.save();
                }
                if !self.one_read {
                    return self.start_conversion(out, false);
                }
                // Reading once needs room for the archive and the
                // temporary file where the archive is saved.
                let needs = self.space_needed().map(|n| n.archive + n.one_read);
                let has = out.parent().and_then(xwa_fetch::drives::free_space);
                match (needs, has) {
                    (Some(needs), Some(has)) if has < needs => {
                        self.pending_output = Some(out);
                        self.popup = Some(Popup::NoRoomToReadOnce { needs, has });
                        self.popup_focus = 0;
                    }
                    _ => return self.start_conversion(out, true),
                }
            }
            Message::OutputChosen(None) => {}
            Message::Conversion(Event::Progress(p)) => self.conversion.progress(p),
            Message::Conversion(Event::Done(result)) => {
                let mut look = Task::none();
                if result.is_ok() {
                    if let Some(out) = self.output.take()
                        && !self.copy.running()
                        && !self.font_work.running()
                    {
                        look = self.use_archive(out);
                    }
                    self.refresh_drives();
                }
                self.conversion = Job::Finished(match result {
                    Err(e) if e == "cancelled" => Err("Conversion stopped.".into()),
                    other => other,
                });
                self.focus = self.first_focusable();
                return look;
            }
            Message::ConvertAgain => {
                if !self.conversion.running() {
                    self.conversion = Job::Idle;
                    self.focus = self.first_focusable();
                }
            }
            Message::AskStopConversion => {
                if self.conversion.running() && !self.conversion.stopping() {
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
                return self.dialog(
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
            Message::ArchiveChosen(Some(path))
                if !self.copy.running() && !self.font_work.running() =>
            {
                return self.use_archive(path);
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
            Message::DownloadFonts => {
                if self.font_download.running() {
                    self.font_download.cancel();
                } else {
                    return self.start_font_download();
                }
            }
            Message::FontDownload(Event::Progress(p)) => self.font_download.progress(p),
            Message::FontDownload(Event::Done(result)) => {
                self.fonts_kept = FontsKept::count();
                self.font_download = match result {
                    Ok(_) => Job::Idle,
                    Err(e) if e == "cancelled" => Job::Idle,
                    Err(e) => Job::Finished(Err(e)),
                };
            }
            Message::ArchiveFontRead(path, state) => {
                if self.archive.as_ref() == Some(&path) && !self.font_work.running() {
                    self.archive_font = state;
                }
            }
            Message::CheckFont => return self.start_font_work(false),
            Message::CompleteFont => return self.start_font_work(true),
            Message::FontWork(Event::Progress(p)) => self.font_work.progress(p),
            Message::FontWork(Event::Done(result)) => {
                self.font_work = Job::Finished(match result {
                    Ok((state, said)) => {
                        self.archive_font = state;
                        Ok(said)
                    }
                    Err(e) if e == "cancelled" => Err("Stopped.".into()),
                    Err(e) => Err(format!(
                        "The fallback font could not be checked or completed: {e}"
                    )),
                });
                self.focus = self.first_focusable();
            }
            Message::CancelFontWork => self.font_work.cancel(),

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
                let back = self.back();
                return Task::batch([back, self.load_books()]);
            }
            Message::BooksLoaded(Ok(books)) => {
                self.library_status = books.is_empty().then(|| {
                    format!(
                        "The library has no {} in this language.",
                        self.prefs.wiki.name()
                    )
                });
                self.books = books;
                self.library_failed = false;
            }
            Message::SetWiki(wiki) => {
                self.popup = None;
                if wiki != self.prefs.wiki {
                    self.prefs.wiki = wiki;
                    self.prefs.save();
                    return self.load_books();
                }
            }
            Message::SetView(view) => {
                self.popup = None;
                self.prefs.view = view;
                self.prefs.save();
            }
            Message::ShowPage(i) => {
                let page = self.page();
                let pages = eink::paginate(&page.rows, self.list_room(&page));
                if let Some(p) = pages.get(i) {
                    self.focus = p.start;
                }
            }
            Message::BooksLoaded(Err(e)) => {
                self.library_status = Some(format!("Could not load the list: {e}"));
                self.library_failed = true;
            }
            Message::Search(s) => {
                self.search = s;
                // The first match is where Enter goes.
                self.focus = crate::screens::LIBRARY_ROWS;
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
            Message::Download => {
                if let Some(book) = self.book.clone() {
                    self.downloads.add(book);
                }
                let start = self.next_download();
                // The rows change: start again at the top, not on whatever
                // took the place of the Download button (such as Remove).
                self.focus = self.first_focusable();
                return start;
            }
            Message::Downloading(run, Event::Progress(p)) => {
                if let Some(Download {
                    state: Fetch::Running { job, .. },
                    ..
                }) = self.downloads.by_run(run)
                {
                    job.progress(p);
                }
            }
            Message::Downloading(run, Event::Done(result)) => {
                self.downloads.stopped(run);
                let Some(d) = self.downloads.by_run(run) else {
                    return self.next_download();
                };
                let watching = self.screen == Screen::Downloads
                    || (self.screen == Screen::Book
                        && self.book.as_ref().is_some_and(|b| b.id == d.book.id));
                let mut finished = None;
                d.state = match result {
                    Ok(path) => {
                        finished = Some(path.clone());
                        Fetch::Done(path)
                    }
                    Err(e) if e == "cancelled" => Fetch::Paused {
                        fraction: d.fraction().unwrap_or(0.0),
                    },
                    Err(e) => Fetch::Failed(e),
                };
                let next = self.next_download();
                let Some(path) = finished else {
                    return next;
                };
                if self.conversion.running() {
                    return next;
                }
                if watching {
                    // Someone watching it finish goes on to converting it.
                    let convert = self.update(Message::ZimChosen(Some(path)));
                    return Task::batch([next, convert]);
                }
                if self.zim.is_none() {
                    // Otherwise it waits on the Convert screen, if that has
                    // nothing else to convert.
                    return Task::batch([next, self.use_zim(path)]);
                }
                return next;
            }
            Message::PauseDownload(id) => {
                self.popup = None;
                if let Some(Download {
                    state: Fetch::Running { job, .. },
                    ..
                }) = self.downloads.items.iter_mut().find(|d| d.id() == id)
                {
                    job.cancel();
                }
            }
            Message::ResumeDownload(id) => {
                self.popup = None;
                if let Some(d) = self.downloads.get(&id) {
                    self.downloads.add(d.book.clone());
                }
                return self.next_download();
            }
            Message::RemoveDownload(id) => {
                self.popup = None;
                self.downloads.remove(&id);
                return self.next_download();
            }
            Message::DownloadMenu(i) => {
                if i < self.downloads.items.len() {
                    self.popup = Some(Popup::Download(i));
                    self.popup_focus = 0;
                }
            }
        }
        Task::none()
    }

    /// Work a screen needs when it is opened.
    fn entered(&mut self) -> Task<Message> {
        match self.screen {
            Screen::Library if self.languages.is_empty() && !self.library_failed => {
                Task::batch([self.load_library(), operation::focus(SEARCH_ID)])
            }
            Screen::Library | Screen::Languages => operation::focus(SEARCH_ID),
            Screen::Card => {
                self.refresh_drives();
                Task::none()
            }
            Screen::Convert => {
                self.refresh_save_free();
                Task::none()
            }
            Screen::Settings => {
                if !self.font_download.running() {
                    self.fonts_kept = FontsKept::count();
                }
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
        let category = self.prefs.wiki.category();
        self.books.clear();
        self.library_status = Some("Loading the list…".into());
        Task::perform(
            background(move || {
                let filter = xwa_fetch::Filter {
                    lang: lang.as_deref(),
                    category: Some(category),
                    name: None,
                };
                xwa_fetch::Client::new()
                    .books(&filter)
                    .map_err(|e| e.to_string())
            }),
            Message::BooksLoaded,
        )
    }

    /// The folder an archive is offered to be saved in: the last one used,
    /// else the ZIM's.
    pub fn save_dir(&self) -> Option<PathBuf> {
        self.prefs
            .save_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| self.zim.as_ref()?.parent().map(Path::to_path_buf))
    }

    /// Looks again at the free space where archives are saved.
    pub fn refresh_save_free(&mut self) {
        self.save_free = self
            .save_dir()
            .and_then(|d| xwa_fetch::drives::free_space(&d).map(|free| (d, free)));
    }

    /// What converting the chosen ZIM needs, as far as it can be told.
    pub fn space_needed(&self) -> Option<xwa_convert::SpaceNeeded> {
        match &self.zim_info {
            Some(Ok(ZimInfo {
                shares: Some(s), ..
            })) => Some(xwa_convert::space_needed(s, self.images)),
            _ => None,
        }
    }

    /// Whether the default save folder has room for reading the ZIM once;
    /// `None` when that cannot be told.
    pub fn one_read_fits(&self) -> Option<bool> {
        let needs = self.space_needed()?;
        let (_, free) = self.save_free.as_ref()?;
        Some(*free >= needs.archive + needs.one_read)
    }

    fn start_conversion(&mut self, out: PathBuf, one_read: bool) -> Task<Message> {
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
        // The fallback font, for letters and symbols the e-reader's fonts lack.
        let fonts = Fonts::default_cache().map(|cache| Arc::new(Fonts::new(cache)));
        let real_italic = self.real_italic;
        self.converting_one_read = one_read;
        self.converting_sharper = sharper;
        self.converting_images = images;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let opts = xwa_convert::Options {
                images,
                diagrams,
                fonts,
                real_italic,
                one_read,
                ..xwa_convert::Options::new(device)
            };
            let mut throttle = Throttle::default();
            let result = xwa_convert::convert(&zim, &out, &opts, &mut |p| {
                if throttle.due(p.stage as usize) {
                    let _ = tx.unbounded_send(Event::Progress(conversion_progress(
                        p, images, sharper, one_read,
                    )));
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

    /// Downloads every Noto font file not kept yet, one at a time.
    fn start_font_download(&mut self) -> Task<Message> {
        let Some(cache) = Fonts::default_cache() else {
            return Task::none();
        };
        let (job, cancel) = Job::start();
        self.font_download = job;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut throttle = Throttle::default();
            let result = xwa_convert::download_all_fonts(&Fonts::new(cache), &mut |done, total| {
                if throttle.due(0) {
                    let _ =
                        tx.unbounded_send(Event::Progress(Progress::bytes(done, total, None, 0)));
                }
                !cancel.load(Ordering::Relaxed)
            });
            let result = result.map(|n| n.to_string()).map_err(|e| e.to_string());
            let _ = tx.unbounded_send(Event::Done(result));
        });
        Task::run(rx, Message::FontDownload)
    }

    /// Makes `path` the archive to copy, and looks at its fallback font.
    fn use_archive(&mut self, path: PathBuf) -> Task<Message> {
        self.archive = Some(path.clone());
        self.copy = Job::Idle;
        self.font_work = Job::Idle;
        self.archive_font = ArchiveFont::Unknown;
        let look = path.clone();
        Task::perform(background(move || archive_font(&look)), move |state| {
            Message::ArchiveFontRead(path.clone(), state)
        })
    }

    /// Reads an older archive's text to find what its font lacks or, with
    /// `complete`, gets what it lacks and puts it in the archive.
    fn start_font_work(&mut self, complete: bool) -> Task<Message> {
        let Some(path) = self.archive.clone() else {
            return Task::none();
        };
        if self.copy.running() || self.font_work.running() {
            return Task::none();
        }
        let chars = match (&self.archive_font, complete) {
            (ArchiveFont::Missing { chars, .. }, true) => Some(chars.clone()),
            (ArchiveFont::Unchecked, false) => None,
            _ => return Task::none(),
        };
        let Some(cache) = Fonts::default_cache() else {
            self.font_work = Job::Finished(Err("There is no folder to keep fonts in.".into()));
            return Task::none();
        };
        let (job, cancel) = Job::start();
        self.font_work = job;
        self.focus = 0;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut throttle = Throttle::default();
            let mut send = |label: String, done: u64, total: u64| {
                if throttle.due(0) {
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
                    let _ = tx.unbounded_send(Event::Progress(Progress {
                        label,
                        fraction,
                        step: 0,
                        rate: None,
                        left: None,
                    }));
                }
                !cancel.load(Ordering::Relaxed)
            };
            let result = match chars {
                None => check_font(&path, &mut send),
                Some(chars) => complete_font(&path, &chars, &Fonts::new(cache), &mut send),
            };
            let _ = tx.unbounded_send(Event::Done(result));
        });
        Task::run(rx, Message::FontWork)
    }

    fn start_copy(&mut self) -> Task<Message> {
        let (Some(file), Some(drive)) = (self.archive.clone(), self.drive.clone()) else {
            return Task::none();
        };
        if self.font_work.running() {
            return Task::none();
        }
        self.font_work = Job::Idle;
        let (job, cancel) = Job::start();
        self.copy = job;
        self.focus = 0;
        let (tx, rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let dir = xwa_fetch::drives::archive_dir(&drive.mount);
            let mut throttle = Throttle::default();
            let mut rate = Rate::default();
            let result = xwa_fetch::drives::copy(&file, &dir, Some(&drive), &mut |p| {
                let speed = rate.add(p.done);
                if throttle.due(0) {
                    let _ = tx.unbounded_send(Event::Progress(Progress::bytes(
                        p.done, p.total, speed, 0,
                    )));
                }
                !cancel.load(Ordering::Relaxed)
            });
            let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
        });
        Task::run(rx, Message::Copying)
    }

    /// Makes `path` the ZIM file to convert, and reads what it holds.
    fn use_zim(&mut self, path: PathBuf) -> Task<Message> {
        self.zim = Some(path.clone());
        self.zim_info = None;
        self.conversion = Job::Idle;
        let read = path.clone();
        Task::perform(background(move || describe(&read)), move |info| {
            Message::ZimRead(path.clone(), info)
        })
    }

    /// Starts the next waiting download, if nothing is downloading. Its
    /// worker starts when the task runs.
    fn next_download(&mut self) -> Task<Message> {
        use iced::futures::StreamExt;
        let (job, cancel) = Job::start();
        let Some((run, url, dir)) = self.downloads.start_next(job, self.download_dir()) else {
            return Task::none();
        };
        let worker = async move {
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                let mut throttle = Throttle::default();
                let mut rate = Rate::default();
                let result = xwa_fetch::Client::new().download(&url, &dir, &mut |p| {
                    let speed = rate.add(p.done);
                    if !throttle.due(p.stage as usize) {
                        return !cancel.load(Ordering::Relaxed);
                    }
                    let progress = match p.stage {
                        xwa_fetch::Stage::Downloading => Progress::bytes(p.done, p.total, speed, 0),
                        xwa_fetch::Stage::Checking => Progress {
                            label: "Checking the download…".into(),
                            fraction: p.done as f32 / p.total.max(1) as f32,
                            step: 1,
                            rate: None,
                            left: None,
                        },
                    };
                    let _ = tx.unbounded_send(Event::Progress(progress));
                    !cancel.load(Ordering::Relaxed)
                });
                let _ = tx.unbounded_send(Event::Done(result.map_err(|e| e.to_string())));
            });
            rx
        };
        Task::run(iced::futures::stream::once(worker).flatten(), move |e| {
            Message::Downloading(run, e)
        })
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
        subs.push(window::resize_events().map(|(_, size)| Message::Resized(size)));
        if self.settle_at.is_some() {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Settle));
        }
        // Wayland says which monitor the window is on only once it shows on
        // one: until then the app cannot fit the window to it.
        if self.shown && self.monitor.is_none() && self.monitor_tries < MONITOR_TRIES {
            subs.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::FindMonitor));
        }
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

/// The steps of a conversion with these options, as shown while it runs.
pub fn conversion_steps(images: bool, sharper: bool, one_read: bool) -> Vec<&'static str> {
    let mut steps = vec!["Reading the ZIM file", "Converting articles"];
    if one_read {
        steps.push("Writing articles");
    }
    if sharper {
        steps.push("Downloading sharper diagrams");
    }
    if images {
        steps.push("Converting pictures");
    }
    steps.push("Finishing");
    steps
}

fn conversion_progress(
    p: xwa_convert::Progress,
    images: bool,
    sharper: bool,
    one_read: bool,
) -> Progress {
    let steps = conversion_steps(images, sharper, one_read);
    let name = match p.stage {
        Stage::Scanning => "Reading the ZIM file",
        Stage::Articles => "Converting articles",
        Stage::Writing => "Writing articles",
        Stage::Diagrams => "Downloading sharper diagrams",
        Stage::Images => "Converting pictures",
        Stage::Fonts | Stage::Finishing => "Finishing",
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
    // Reading once, writing the articles takes a tenth of their share.
    let (articles, diagrams) = if sharper { (0.35, 0.30) } else { (0.55, 0.0) };
    let pictures = 0.90 - articles - diagrams;
    let writing = if one_read { articles / 10.0 } else { 0.0 };
    let articles = articles - writing;
    let fraction = match p.stage {
        Stage::Scanning => 0.05 * part,
        Stage::Articles => 0.05 + articles * part,
        Stage::Writing => 0.05 + articles + writing * part,
        Stage::Diagrams => 0.05 + articles + writing + diagrams * part,
        Stage::Images => 0.05 + articles + writing + diagrams + pictures * part,
        Stage::Fonts => 0.95 + 0.025 * part,
        Stage::Finishing => 0.975 + 0.025 * part,
    };
    let label = if p.stage == Stage::Fonts {
        format!("Fonts: {} of {}", group(p.done), group(p.total))
    } else if p.stage == Stage::Scanning {
        // Reading goes over the ZIM three times; its counts are only
        // shares of the step, so say which pass it is on instead.
        let pass = (p.done * 3).checked_div(p.total).unwrap_or(0).min(2) as usize;
        let what = ["Listing entries", "Sorting pages", "Listing redirects"][pass];
        format!("{what} ({} of 3)", pass + 1)
    } else if p.total > 0 && p.stage != Stage::Finishing {
        format!("{} of {}", group(p.done), group(p.total))
    } else {
        String::new()
    };
    Progress {
        label,
        fraction,
        step,
        rate: None,
        left: None,
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
    if let Some(f) = &r.fallback_font
        && let Some((_, why)) = f.failed.first()
    {
        done.push_str(&format!(
            " The fallback font is incomplete: {} of its Noto fonts could not be had ({why}), \
             so {} characters the e-reader's fonts lack have no glyph. Converting again \
             downloads what is missing.",
            group(f.failed.len() as u64),
            group(f.missing as u64),
        ));
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
        shares: xwa_convert::zim_shares(path).ok(),
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
    fn the_summary_says_when_the_fallback_font_is_incomplete() {
        let mut r = xwa_convert::Report {
            summary: Default::default(),
            stats: Default::default(),
            failed_articles: 0,
            failed_images: 0,
            dangling_redirects: 0,
            pages_left_out: 0,
            diagrams: None,
            sharper_images: 0,
            fallback_font: None,
            temp_bytes: None,
            zim_bytes: 0,
            elapsed: Duration::ZERO,
        };
        r.summary.articles = 2;
        let out = std::path::Path::new("wiki.xwa");
        let device = xwa_convert::Device::X4;
        assert!(!summary(&r, out, device).contains("fallback"));
        r.fallback_font = Some(xwa_convert::fallback::Report {
            missing: 1_234,
            failed: vec![
                ("Noto Sans".into(), "network: timed out".into()),
                (
                    "Noto Sans SC".into(),
                    "not downloaded: no connection".into(),
                ),
            ],
            ..Default::default()
        });
        let s = summary(&r, out, device);
        assert!(
            s.contains("2 of its Noto fonts could not be had (network: timed out)"),
            "{s}"
        );
        assert!(s.contains("1,234 characters"), "{s}");
    }

    #[test]
    fn conversion_steps_follow_the_options() {
        assert_eq!(conversion_steps(false, false, false).len(), 3);
        assert_eq!(conversion_steps(true, true, false).len(), 5);
        assert_eq!(conversion_steps(true, false, true).len(), 5);
        let p = conversion_progress(
            xwa_convert::Progress {
                stage: Stage::Images,
                done: 5,
                total: 10,
            },
            true,
            false,
            false,
        );
        assert_eq!(p.step, 2);
        assert!((p.fraction - (0.05 + 0.55 + 0.35 * 0.5)).abs() < 1e-6);
        assert_eq!(p.label, "5 of 10");
        // Reading reports shares of its three passes, not counts to show.
        let part = 1 << 20;
        for (done, label) in [
            (0, "Listing entries (1 of 3)"),
            (part + part / 2, "Sorting pages (2 of 3)"),
            (3 * part, "Listing redirects (3 of 3)"),
        ] {
            let p = conversion_progress(
                xwa_convert::Progress {
                    stage: Stage::Scanning,
                    done,
                    total: 3 * part,
                },
                true,
                false,
                false,
            );
            assert_eq!(p.label, label);
        }
    }
}
