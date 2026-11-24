//! Talking to the reader over USB. Xteink readers show up as the ESP32
//! chips' own USB serial port (Espressif's vendor ID 0x303A, product
//! 0x1001), whose bootloader in ROM espflash talks to.

use std::borrow::Cow;

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::image_format::Segment;
use espflash::target::ProgressCallbacks;
use serialport::{SerialPortType, UsbPortInfo};

use crate::image::{self, AppDesc};
use crate::layout::{self, Layout, Ota};
use crate::{Chip, Error, Model, Result};

const ESPRESSIF: u16 = 0x303A;
const USB_SERIAL_JTAG: u16 = 0x1001;

/// A reader plugged in, not yet connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    info: UsbPortInfo,
}

/// The readers plugged in over USB.
pub fn ports() -> Vec<Port> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    ports
        .into_iter()
        .filter_map(|p| match p.port_type {
            SerialPortType::UsbPort(info)
                if info.vid == ESPRESSIF && info.pid == USB_SERIAL_JTAG =>
            {
                Some(Port {
                    name: p.port_name,
                    info,
                })
            }
            _ => None,
        })
        // macOS lists each port twice; the call-out device is the one to use.
        .filter(|p| !(cfg!(target_os = "macos") && p.name.starts_with("/dev/tty.")))
        .collect()
}

/// What is on a connected reader.
#[derive(Debug, Clone)]
pub struct Reader {
    pub model: Model,
    pub layout: Layout,
    /// The firmware that boots now, if it says what it is.
    pub installed: Option<AppDesc>,
    ota: Ota,
    otadata: Vec<u8>,
}

/// What [`Session::install`] is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Writing the firmware: the fraction done.
    Writing(f32),
    Verifying,
    Restarting,
}

/// What installing did.
#[derive(Debug, Clone)]
pub struct Installed {
    pub model: Model,
    /// The slot (0 or 1) it went into.
    pub slot: usize,
    pub previous: Option<AppDesc>,
    pub firmware: Option<AppDesc>,
    /// Whether the reader restarts by itself; the X4 Pro's ESP32-S3 cannot
    /// be restarted over USB, so it is unplugged and switched on again.
    pub restarts: bool,
}

/// A connection to a reader in its bootloader, with what is on it.
pub struct Session {
    flasher: Flasher,
    reader: Reader,
}

fn reader_error(port: &str, e: espflash::Error) -> Error {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("permission denied") || lower.contains("access is denied") {
        return Error::Reader(if cfg!(target_os = "linux") {
            format!(
                "no permission to use {port}: add yourself to the dialout group (uucp on \
                 Arch), then log out and in again"
            )
        } else {
            format!("{port} is in use by another program; close it and try again")
        });
    }
    Error::Reader(format!("could not talk to the reader on {port}: {text}"))
}

impl Session {
    /// Connects to the reader on `port`, which restarts it into its
    /// bootloader, and reads what is on it. Nothing is written.
    pub fn connect(port: &Port) -> Result<Session> {
        let serial = serialport::new(&port.name, 115_200)
            .flow_control(serialport::FlowControl::None)
            .open_native()
            .map_err(|e| reader_error(&port.name, e.into()))?;
        let connection = Connection::new(
            serial,
            port.info.clone(),
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            115_200,
        );
        let mut flasher = Flasher::connect(connection, true, true, false, None, None)
            .map_err(|e| reader_error(&port.name, e))?;
        let chip = match flasher.chip() {
            espflash::target::Chip::Esp32c3 => Chip::Esp32c3,
            espflash::target::Chip::Esp32s3 => Chip::Esp32s3,
            other => {
                return Err(Error::Reader(format!(
                    "the device on {} is an {other}, not an Xteink reader",
                    port.name
                )));
            }
        };
        let table = read(&mut flasher, layout::TABLE_AT, layout::TABLE_LEN)?;
        let layout = layout::identify(&layout::parse_table(&table))?;
        if layout.model.chip() != chip {
            return Err(Error::Reader(format!(
                "its partitions are an {}'s but its chip is an {}; nothing was written",
                layout.model.name(),
                chip.name()
            )));
        }
        let otadata = read(&mut flasher, layout.otadata, layout::OTADATA_LEN)?;
        let ota = Ota::parse(&otadata)?;
        let slot = layout.slots[ota.active];
        let head = read(&mut flasher, slot.offset, image::APP_DESC_END as u32)?;
        Ok(Session {
            flasher,
            reader: Reader {
                model: layout.model,
                layout,
                installed: image::app_desc(&head),
                ota,
                otadata,
            },
        })
    }

    pub fn reader(&self) -> &Reader {
        &self.reader
    }

    /// Lets the reader go without writing anything: restarts it into its
    /// firmware (the X4 Pro is unplugged and switched on instead).
    pub fn leave(mut self) {
        let _ = self.flasher.connection().reset();
    }

    /// Writes `firmware` into the slot not in use, verifies it, and sets
    /// the reader to boot it; then restarts the reader where it can. The
    /// OTA data is written after the firmware, and only once the firmware
    /// is verified: if writing fails or stops, the reader still boots what
    /// it had.
    pub fn install(mut self, firmware: &[u8], progress: &mut dyn FnMut(Step)) -> Result<Installed> {
        let model = self.reader.model;
        let info = image::check(firmware)?;
        if info.chip != model.chip() {
            return Err(Error::Image(format!(
                "it is for an {}, and the {} has an {}",
                info.chip.name(),
                model.name(),
                model.chip().name()
            )));
        }
        let slot_index = self.reader.ota.inactive();
        let slot = self.reader.layout.slots[slot_index];
        if firmware.len() as u64 > slot.size as u64 {
            return Err(Error::Image(format!(
                "it is {} bytes and the {}'s firmware slots hold {}",
                firmware.len(),
                model.name(),
                slot.size
            )));
        }
        let mut app = firmware.to_vec();
        // Flash is written in words.
        app.resize(app.len().next_multiple_of(4), 0xFF);
        let (ota_at, ota_sector) = self
            .reader
            .ota
            .switch(&self.reader.otadata, self.reader.layout.otadata);
        let segments = [
            Segment {
                addr: slot.offset,
                data: Cow::Owned(app),
            },
            Segment {
                addr: ota_at,
                data: Cow::Owned(ota_sector),
            },
        ];
        let mut report = Report {
            app_at: slot.offset,
            writing_app: false,
            total: 1,
            progress,
        };
        // Each segment is verified (its MD5 read back) before the next is
        // written, so the OTA data changes only after the firmware checks
        // out; the reader then restarts.
        self.flasher
            .write_bins_to_flash(&segments, &mut report)
            .map_err(|e| Error::Reader(format!("writing failed: {e}")))?;
        (report.progress)(Step::Restarting);
        Ok(Installed {
            model,
            slot: slot_index,
            previous: self.reader.installed.clone(),
            firmware: info.app,
            restarts: model != Model::X4Pro,
        })
    }
}

/// Reads `size` bytes of the reader's flash at `offset`.
fn read(flasher: &mut Flasher, offset: u32, size: u32) -> Result<Vec<u8>> {
    let file = tempfile::NamedTempFile::new()?;
    flasher
        .read_flash(offset, size, 0x1000, 64, file.path().to_path_buf())
        .map_err(|e| Error::Reader(format!("could not read the reader's flash: {e}")))?;
    let data = std::fs::read(file.path())?;
    if data.len() != size as usize {
        return Err(Error::Reader(
            "the reader's flash could not be read whole".into(),
        ));
    }
    Ok(data)
}

/// espflash's progress, as [`Step`]s for the firmware segment.
struct Report<'a> {
    app_at: u32,
    writing_app: bool,
    total: usize,
    progress: &'a mut dyn FnMut(Step),
}

impl ProgressCallbacks for Report<'_> {
    fn init(&mut self, addr: u32, total: usize) {
        self.writing_app = addr == self.app_at;
        self.total = total.max(1);
        if self.writing_app {
            (self.progress)(Step::Writing(0.0));
        }
    }

    fn update(&mut self, current: usize) {
        if self.writing_app {
            (self.progress)(Step::Writing(current as f32 / self.total as f32));
        }
    }

    fn verifying(&mut self) {
        if self.writing_app {
            (self.progress)(Step::Verifying);
        }
    }

    fn finish(&mut self, _skipped: bool) {}
}

/// Where the firmware comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A release: the file in it for the reader's model.
    Release(crate::release::Release),
    /// A firmware file's contents (other firmware, when allowed).
    File(Vec<u8>),
}

/// What [`install`] is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Connecting,
    /// Downloading the release's firmware: bytes done and in all.
    Downloading(u64, u64),
    Writing(f32),
    Verifying,
    Restarting,
}

impl Stage {
    /// Whether stopping is still possible: until writing starts.
    pub fn can_stop(self) -> bool {
        matches!(self, Stage::Connecting | Stage::Downloading(..))
    }
}

/// Installs firmware from `source` on the reader on `port`: connects,
/// finds which reader it is, takes the release's firmware for it (or the
/// file), checks it, and writes it. `progress` stops it by returning false,
/// which it heeds until writing starts.
pub fn install(
    port: &Port,
    source: &Source,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<Installed> {
    if !progress(Stage::Connecting) {
        return Err(Error::Cancelled);
    }
    let session = Session::connect(port)?;
    let model = session.reader().model;
    let firmware = match source {
        Source::File(data) => Cow::Borrowed(data.as_slice()),
        Source::Release(release) => {
            let Some(asset) = release.firmware_for(model) else {
                session.leave();
                return Err(Error::Reader(format!(
                    "release {} has no firmware for the {}",
                    release.tag,
                    model.name()
                )));
            };
            match crate::release::download(asset, &mut |done, total| {
                progress(Stage::Downloading(done, total))
            }) {
                Ok(data) => Cow::Owned(data),
                Err(e) => {
                    session.leave();
                    return Err(e);
                }
            }
        }
    };
    // Checked before anything is written, so a bad file leaves the reader
    // as it was.
    if let Err(e) = image::check(&firmware) {
        session.leave();
        return Err(e);
    }
    if !progress(Stage::Writing(0.0)) {
        session.leave();
        return Err(Error::Cancelled);
    }
    session.install(&firmware, &mut |step| {
        progress(match step {
            Step::Writing(f) => Stage::Writing(f),
            Step::Verifying => Stage::Verifying,
            Step::Restarting => Stage::Restarting,
        });
    })
}
