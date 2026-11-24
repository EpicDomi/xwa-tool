//! Installs firmware on an Xteink e-reader over USB, as the CrossPoint
//! and CPR-vCodex browser flashers do: the reader's partition table must be
//! one of the known Xteink layouts, the firmware goes into the app slot not
//! in use and is verified there, and only then is the OTA data switched to
//! boot it. The bootloader, partition table, settings (NVS) and the SD card
//! are never touched, and the firmware in the other slot stays as it was.
//!
//! [`image`] checks a firmware file, [`layout`] the reader's partitions and
//! OTA data, [`release`] finds firmware in GitHub releases, and [`device`]
//! talks to the reader.

pub mod device;
pub mod image;
pub mod layout;
pub mod release;

/// The e-readers firmware can be installed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    X4,
    X3,
    X4Pro,
}

impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Model::X4 => "Xteink X4",
            Model::X3 => "Xteink X3",
            Model::X4Pro => "Xteink X4 Pro",
        }
    }

    /// The chip it runs on.
    pub fn chip(self) -> Chip {
        match self {
            Model::X4 | Model::X3 => Chip::Esp32c3,
            Model::X4Pro => Chip::Esp32s3,
        }
    }
}

/// The Espressif chips of the readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chip {
    Esp32c3,
    Esp32s3,
}

impl Chip {
    /// Its chip ID in a firmware image's header.
    pub fn image_id(self) -> u16 {
        match self {
            Chip::Esp32c3 => 5,
            Chip::Esp32s3 => 9,
        }
    }

    pub fn from_image_id(id: u16) -> Option<Chip> {
        match id {
            5 => Some(Chip::Esp32c3),
            9 => Some(Chip::Esp32s3),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Chip::Esp32c3 => "ESP32-C3",
            Chip::Esp32s3 => "ESP32-S3",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a firmware file: {0}")]
    Image(String),
    #[error("{0}")]
    Reader(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected reply from GitHub: {0}")]
    Format(String),
    #[error("the download was damaged (its checksum does not match); please try again")]
    Checksum,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
