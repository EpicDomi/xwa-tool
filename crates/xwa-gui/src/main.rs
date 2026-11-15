#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end: pick a ZIM (button, drag and drop, or "open with"),
//! choose the e-reader, convert.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::widget::{button, column, pick_list, progress_bar, row, text};
use iced::{Element, Subscription, Task, event, window};
use xwa_convert::Stage;

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

enum Job {
    Idle,
    Running {
        stage: &'static str,
        fraction: f32,
        cancel: Arc<AtomicBool>,
    },
    Finished(Result<String, String>),
}

struct App {
    zim: Option<PathBuf>,
    details: Option<Result<String, String>>,
    device: Device,
    loading: bool,
    job: Job,
}

#[derive(Debug, Clone)]
enum Message {
    ChooseZim,
    ZimChosen(Option<PathBuf>),
    DetailsLoaded(Result<String, String>),
    DeviceChosen(Device),
    Convert,
    OutputChosen(Option<PathBuf>),
    Conversion(Event),
    Cancel,
}

/// Messages from the conversion thread.
#[derive(Debug, Clone)]
enum Event {
    Progress(xwa_convert::Progress),
    Done(Result<String, String>),
}

impl App {
    fn new(zim: Option<PathBuf>) -> (Self, Task<Message>) {
        let app = App {
            zim: None,
            details: None,
            device: Device::default(),
            loading: false,
            job: Job::Idle,
        };
        let task = match zim {
            Some(path) => Task::done(Message::ZimChosen(Some(path))),
            None => Task::none(),
        };
        (app, task)
    }

    fn running(&self) -> bool {
        matches!(self.job, Job::Running { .. })
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
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
        Message::ZimChosen(Some(path)) if !app.running() => {
            app.zim = Some(path.clone());
            app.details = None;
            app.loading = true;
            app.job = Job::Idle;
            Task::perform(async move { describe(&path) }, Message::DetailsLoaded)
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
                stage: "Starting…",
                fraction: 0.0,
                cancel: cancel.clone(),
            };
            let device = app.device.converter();
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                let opts = xwa_convert::Options::new(device);
                let result = xwa_convert::convert(&zim, &out, &opts, &mut |p| {
                    let _ = tx.unbounded_send(Event::Progress(p));
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
        Message::Conversion(Event::Progress(p)) => {
            if let Job::Running {
                stage, fraction, ..
            } = &mut app.job
            {
                *stage = match p.stage {
                    Stage::Scanning => "Reading the ZIM file…",
                    Stage::Articles => "Converting articles…",
                    Stage::Finishing => "Finishing…",
                };
                // Reading takes a few percent of the time, articles most of it.
                let part = if p.total == 0 {
                    1.0
                } else {
                    p.done as f32 / p.total as f32
                };
                *fraction = match p.stage {
                    Stage::Scanning => 0.05 * part,
                    Stage::Articles => 0.05 + 0.9 * part,
                    Stage::Finishing => 0.95 + 0.05 * part,
                };
            }
            Task::none()
        }
        Message::Conversion(Event::Done(result)) => {
            app.job = Job::Finished(match result {
                Err(e) if e == "cancelled" => Err("Conversion cancelled.".into()),
                other => other,
            });
            Task::none()
        }
        Message::Cancel => {
            if let Job::Running { cancel, stage, .. } = &mut app.job {
                cancel.store(true, Ordering::Relaxed);
                *stage = "Cancelling…";
            }
            Task::none()
        }
    }
}

fn summary(r: &xwa_convert::Report, out: &Path, device: xwa_convert::Device) -> String {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(
        "Done: {} articles in {} ({:.0} MB), made for the {}.\n\
         Copy it to your e-reader's SD card.\n\
         Pictures are not included yet, and formulas appear as text for now.",
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
        "{}\n{} · {} · {}\n{} entries · {:.1} GB",
        meta("Title"),
        meta("Language"),
        meta("Flavour"),
        meta("Date"),
        zim.entry_count(),
        size as f64 / 1e9
    ))
}

fn view(app: &App) -> Element<'_, Message> {
    let file = app.zim.as_ref().map_or_else(
        || "No file chosen (you can also drop one here)".to_string(),
        |p| p.display().to_string(),
    );
    let details = match (&app.details, app.loading) {
        (_, true) => text("Reading…"),
        (Some(Ok(d)), _) => text(d.clone()),
        (Some(Err(e)), _) => text(format!("Could not read this file: {e}")),
        (None, _) => text(""),
    };
    let running = app.running();
    let readable = matches!(app.details, Some(Ok(_)));
    let choose =
        button("Choose ZIM file…").on_press_maybe((!running).then_some(Message::ChooseZim));
    let devices = pick_list(&Device::ALL[..], Some(app.device), Message::DeviceChosen);
    let convert =
        button("Convert").on_press_maybe((readable && !running).then_some(Message::Convert));
    let status: Element<'_, Message> = match &app.job {
        Job::Idle => text("").into(),
        Job::Running {
            stage, fraction, ..
        } => column![
            progress_bar(0.0..=1.0, *fraction),
            row![text(*stage), button("Cancel").on_press(Message::Cancel)].spacing(12),
        ]
        .spacing(8)
        .into(),
        Job::Finished(Ok(s)) => text(s.clone()).into(),
        Job::Finished(Err(e)) => text(format!("Conversion failed: {e}")).into(),
    };
    column![
        text("Convert a Kiwix ZIM for your Xteink").size(22),
        row![choose, text(file)].spacing(12),
        details,
        row![text("Device"), devices].spacing(12),
        convert,
        status,
    ]
    .spacing(16)
    .padding(24)
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
        .window_size((720.0, 460.0))
        .run()
}
