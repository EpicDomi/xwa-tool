//! Firmware files: ESP-IDF app images, checked the way the reader's
//! bootloader will check them, so that nothing it would refuse is written.

use sha2::{Digest, Sha256};

use crate::{Chip, Error, Result};

const MAGIC: u8 = 0xE9;
/// The image header, before the first segment.
const HEADER: usize = 24;
/// A segment's header: its load address and length.
const SEGMENT_HEADER: usize = 8;
const CHECKSUM_SEED: u8 = 0xEF;
/// The SHA-256 of the rest that the image may end with.
const DIGEST: usize = 32;
/// `esp_app_desc_t`, which starts the first segment.
const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const APP_DESC_AT: usize = HEADER + SEGMENT_HEADER;
const APP_DESC_LEN: usize = 256;

/// What a firmware image says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub chip: Chip,
    pub app: Option<AppDesc>,
}

/// The app description ESP-IDF builds into firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDesc {
    pub project: String,
    pub version: String,
    /// When it was built, as the build put it ("Nov 20 2026 14:02:11").
    pub built: String,
}

impl AppDesc {
    /// "cpr-vcodex 1.5.0.31", or just the version when there is no name.
    pub fn label(&self) -> String {
        if self.project.is_empty() {
            self.version.clone()
        } else {
            format!("{} {}", self.project, self.version)
        }
    }
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

/// Checks that `data` is a whole, undamaged ESP-IDF app image (its
/// segments, checksum and, where it has one, SHA-256), and reads its chip
/// and app description.
pub fn check(data: &[u8]) -> Result<Image> {
    let bad = |s: String| Err(Error::Image(s));
    if data.len() < HEADER {
        return bad("it is too short".into());
    }
    if data[0] != MAGIC {
        return bad("it does not start as ESP firmware does (0xE9)".into());
    }
    let chip_id = u16::from_le_bytes([data[12], data[13]]);
    let Some(chip) = Chip::from_image_id(chip_id) else {
        return bad(format!(
            "it is for a chip no Xteink reader has (chip ID {chip_id})"
        ));
    };
    let segments = data[1] as usize;
    let has_digest = data[23] & 1 != 0;
    let mut checksum = CHECKSUM_SEED;
    let mut at = HEADER;
    for _ in 0..segments {
        if data.len() - at < SEGMENT_HEADER {
            return bad("a segment's header runs past the end".into());
        }
        let len = u32_at(data, at + 4) as usize;
        at += SEGMENT_HEADER;
        if len > data.len() - at {
            return bad("a segment runs past the end".into());
        }
        checksum = data[at..at + len].iter().fold(checksum, |c, b| c ^ b);
        at += len;
    }
    // The checksum byte ends the image, padded to 16 bytes.
    let padded = (at + 16) & !15;
    let end = padded + if has_digest { DIGEST } else { 0 };
    if end != data.len() {
        return bad(format!(
            "it should be {end} bytes long but is {} (cut short or with something added)",
            data.len()
        ));
    }
    if data[padded - 1] != checksum {
        return bad("its checksum does not match (it is damaged)".into());
    }
    if has_digest && Sha256::digest(&data[..end - DIGEST])[..] != data[end - DIGEST..] {
        return bad("its SHA-256 does not match (it is damaged)".into());
    }
    Ok(Image {
        chip,
        app: app_desc(data),
    })
}

/// The app description in the first bytes of an image (or of an app
/// slot), if it has one.
pub fn app_desc(data: &[u8]) -> Option<AppDesc> {
    let d = data.get(APP_DESC_AT..APP_DESC_AT + APP_DESC_LEN)?;
    if data[0] != MAGIC || u32_at(d, 0) != APP_DESC_MAGIC {
        return None;
    }
    let text = |at: usize, len: usize| {
        let s = &d[at..at + len];
        let end = s.iter().position(|&b| b == 0).unwrap_or(len);
        String::from_utf8_lossy(&s[..end]).trim().to_string()
    };
    let (time, date) = (text(80, 16), text(96, 16));
    Some(AppDesc {
        version: text(16, 32),
        project: text(48, 32),
        built: format!("{date} {time}").trim().to_string(),
    })
}

/// How many bytes from the start of an app slot hold its app description.
pub const APP_DESC_END: usize = APP_DESC_AT + APP_DESC_LEN;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small image for `chip`, with an app description and, if asked, a
    /// SHA-256.
    pub(crate) fn image(chip: Chip, version: &str, digest: bool) -> Vec<u8> {
        let mut desc = vec![0u8; APP_DESC_LEN];
        desc[..4].copy_from_slice(&APP_DESC_MAGIC.to_le_bytes());
        desc[16..16 + version.len()].copy_from_slice(version.as_bytes());
        desc[48..48 + 10].copy_from_slice(b"cpr-vcodex");
        desc[80..88].copy_from_slice(b"14:02:11");
        desc[96..107].copy_from_slice(b"Nov 20 2026");
        let code: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut out = vec![0u8; HEADER];
        out[0] = MAGIC;
        out[1] = 2;
        out[12..14].copy_from_slice(&chip.image_id().to_le_bytes());
        out[23] = digest as u8;
        let mut checksum = CHECKSUM_SEED;
        for (addr, seg) in [(0x3C00_0020u32, &desc), (0x4200_0000, &code)] {
            out.extend_from_slice(&addr.to_le_bytes());
            out.extend_from_slice(&(seg.len() as u32).to_le_bytes());
            out.extend_from_slice(seg);
            checksum = seg.iter().fold(checksum, |c, b| c ^ b);
        }
        let padded = (out.len() + 16) & !15;
        out.resize(padded, 0);
        out[padded - 1] = checksum;
        if digest {
            let d = Sha256::digest(&out);
            out.extend_from_slice(&d);
        }
        out
    }

    #[test]
    fn a_whole_image_is_read() {
        for (chip, digest) in [(Chip::Esp32c3, true), (Chip::Esp32s3, false)] {
            let img = image(chip, "1.5.0.31", digest);
            let info = check(&img).unwrap();
            assert_eq!(info.chip, chip);
            let app = info.app.unwrap();
            assert_eq!(app.label(), "cpr-vcodex 1.5.0.31");
            assert_eq!(app.built, "Nov 20 2026 14:02:11");
        }
    }

    #[test]
    fn damaged_or_foreign_files_are_refused() {
        let good = image(Chip::Esp32c3, "1.0", true);
        let mut flipped = good.clone();
        flipped[HEADER + SEGMENT_HEADER + 300] ^= 1;
        let mut wrong_chip = good.clone();
        wrong_chip[12] = 2; // ESP32-S2
        let cases: [(&str, Vec<u8>); 5] = [
            ("short", good[..good.len() - 1].to_vec()),
            ("longer", [good.clone(), vec![0]].concat()),
            ("a flipped bit", flipped),
            ("another chip", wrong_chip),
            (
                "not firmware",
                b"PK\x03\x04 a zip file, not firmware".to_vec(),
            ),
        ];
        for (what, data) in cases {
            assert!(check(&data).is_err(), "{what}");
        }
    }
}
