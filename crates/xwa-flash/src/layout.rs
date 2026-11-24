//! The reader's partition table and OTA data. Firmware goes only onto a
//! reader whose partitions are one of the known Xteink layouts (as the
//! factory and CrossPoint-based firmware lay them out), into the app slot
//! the OTA data says is not in use; the OTA data then says to boot it.

use crate::{Error, Model, Result};

/// Where the partition table is, and how much of it to read.
pub const TABLE_AT: u32 = 0x8000;
pub const TABLE_LEN: u32 = 0x1000;
/// The OTA data: two 4 KB sectors, each starting with a selection entry.
pub const OTADATA_LEN: u32 = 0x2000;
const SECTOR: usize = 0x1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Nvs,
    OtaData,
    App0,
    App1,
    Spiffs,
    Coredump,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub kind: Kind,
    pub offset: u32,
    pub size: u32,
}

/// Reads an ESP-IDF partition table: 32-byte entries starting 0xAA 0x50,
/// up to the MD5 entry or the first blank one.
pub fn parse_table(data: &[u8]) -> Vec<Partition> {
    let mut out = Vec::new();
    for e in data.as_chunks::<32>().0 {
        if e.iter().all(|&b| b == 0xFF) || e[..2] == [0xEB, 0xEB] {
            break;
        }
        if e[..2] != [0xAA, 0x50] {
            break;
        }
        let kind = match (e[2], e[3]) {
            (0, 0x10) => Kind::App0,
            (0, 0x11) => Kind::App1,
            (1, 0) => Kind::OtaData,
            (1, 2) => Kind::Nvs,
            (1, 3) => Kind::Coredump,
            (1, 0x82) => Kind::Spiffs,
            _ => Kind::Other,
        };
        let u32_at = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
        out.push(Partition {
            kind,
            offset: u32_at(4),
            size: u32_at(8),
        });
    }
    out
}

const fn p(kind: Kind, offset: u32, size: u32) -> Partition {
    Partition { kind, offset, size }
}

/// The layouts firmware is installed on, as the readers come and as
/// CrossPoint and CPR-vCodex keep them.
const LAYOUTS: &[(Model, [Partition; 6])] = &[
    (
        Model::X4,
        [
            p(Kind::Nvs, 0x9000, 0x5000),
            p(Kind::OtaData, 0xE000, 0x2000),
            p(Kind::App0, 0x10000, 0x640000),
            p(Kind::App1, 0x650000, 0x640000),
            p(Kind::Spiffs, 0xC90000, 0x360000),
            p(Kind::Coredump, 0xFF0000, 0x10000),
        ],
    ),
    (
        Model::X3,
        [
            p(Kind::Nvs, 0x9000, 0x5000),
            p(Kind::OtaData, 0xE000, 0x2000),
            p(Kind::App0, 0x10000, 0x770000),
            p(Kind::App1, 0x780000, 0x770000),
            p(Kind::Spiffs, 0xEF0000, 0x100000),
            p(Kind::Coredump, 0xFF0000, 0x10000),
        ],
    ),
    (
        Model::X4Pro,
        [
            p(Kind::Nvs, 0x9000, 0x5000),
            p(Kind::OtaData, 0xE000, 0x2000),
            p(Kind::App0, 0x10000, 0x7E0000),
            p(Kind::App1, 0x7F0000, 0x7E0000),
            p(Kind::Spiffs, 0xFD0000, 0x14000),
            p(Kind::Coredump, 0xFE4000, 0x1C000),
        ],
    ),
];

/// A reader's layout: its model, and where its OTA data and slots are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub model: Model,
    pub otadata: u32,
    pub slots: [Partition; 2],
}

/// Which reader a partition table is, if it is one of the known layouts.
pub fn identify(table: &[Partition]) -> Result<Layout> {
    let (model, parts) = LAYOUTS
        .iter()
        .find(|(_, l)| table == l.as_slice())
        .ok_or_else(|| {
            Error::Reader(
                "its partition table is not one of the Xteink layouts, so nothing was \
                 written; flash it once with the CrossPoint web installer first"
                    .into(),
            )
        })?;
    Ok(Layout {
        model: *model,
        otadata: parts[1].offset,
        slots: [parts[2], parts[3]],
    })
}

/// ESP-IDF's `esp_ota_img_states_t`.
const STATE_NEW: u32 = 0;
const STATE_INVALID: u32 = 3;
const STATE_ABORTED: u32 = 4;
const BLANK: u32 = 0xFFFF_FFFF;

/// The CRC ESP-IDF keeps of a selection entry's sequence number: the ROM's
/// little-endian CRC-32 from `u32::MAX`, which starts from 0 and inverts at
/// the end.
fn seq_crc(seq: u32) -> u32 {
    let mut crc = 0u32;
    for b in seq.to_le_bytes() {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// What the OTA data says, and how to make it boot the other slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ota {
    /// The slot (0 or 1) that boots now.
    pub active: usize,
    /// Its sequence number; 0 when neither entry is valid (slot 0 boots).
    pub seq: u32,
    /// The sector (0 or 1) the active entry is in, if any is valid.
    sector: Option<usize>,
}

impl Ota {
    pub fn parse(data: &[u8]) -> Result<Ota> {
        if data.len() < 2 * SECTOR {
            return Err(Error::Reader("its OTA data could not be read whole".into()));
        }
        let valid = |sector: usize| {
            let e = &data[sector * SECTOR..sector * SECTOR + 32];
            let u32_at = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
            let (seq, state, crc) = (u32_at(0), u32_at(24), u32_at(28));
            (seq != BLANK
                && crc == seq_crc(seq)
                && state != STATE_INVALID
                && state != STATE_ABORTED)
                .then_some(seq)
        };
        let best = [0, 1]
            .into_iter()
            .filter_map(|s| valid(s).map(|seq| (s, seq)))
            .max_by_key(|&(_, seq)| seq);
        Ok(match best {
            Some((sector, seq)) => Ota {
                active: ((seq - 1) % 2) as usize,
                seq,
                sector: Some(sector),
            },
            None => Ota {
                active: 0,
                seq: 0,
                sector: None,
            },
        })
    }

    /// The slot not in use, where new firmware goes.
    pub fn inactive(&self) -> usize {
        1 - self.active
    }

    /// The OTA data sector to write, and where it goes, to boot the
    /// inactive slot next: the other sector than the active entry's,
    /// holding the next sequence number that selects that slot, marked new
    /// (the firmware confirms itself when it starts; a bootloader with
    /// rollback goes back to this slot if it does not).
    pub fn switch(&self, data: &[u8], otadata: u32) -> (u32, Vec<u8>) {
        let target = self.inactive() as u32;
        let mut seq = self.seq + 1;
        while (seq - 1) % 2 != target {
            seq += 1;
        }
        let sector = self.sector.map_or(0, |s| 1 - s);
        let mut out = data[sector * SECTOR..(sector + 1) * SECTOR].to_vec();
        out[0..4].copy_from_slice(&seq.to_le_bytes());
        out[24..28].copy_from_slice(&STATE_NEW.to_le_bytes());
        out[28..32].copy_from_slice(&seq_crc(seq).to_le_bytes());
        (otadata + (sector * SECTOR) as u32, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(parts: &[Partition]) -> Vec<u8> {
        let mut out = vec![0xFF; TABLE_LEN as usize];
        for (i, part) in parts.iter().enumerate() {
            let (t, s) = match part.kind {
                Kind::App0 => (0, 0x10),
                Kind::App1 => (0, 0x11),
                Kind::OtaData => (1, 0),
                Kind::Nvs => (1, 2),
                Kind::Coredump => (1, 3),
                Kind::Spiffs => (1, 0x82),
                Kind::Other => (1, 0x99),
            };
            let e = &mut out[i * 32..(i + 1) * 32];
            e.fill(0);
            e[..4].copy_from_slice(&[0xAA, 0x50, t, s]);
            e[4..8].copy_from_slice(&part.offset.to_le_bytes());
            e[8..12].copy_from_slice(&part.size.to_le_bytes());
        }
        // The MD5 entry ends it.
        let n = parts.len();
        out[n * 32..n * 32 + 2].copy_from_slice(&[0xEB, 0xEB]);
        out
    }

    #[test]
    fn the_three_readers_are_told_apart_and_others_refused() {
        for (model, parts) in LAYOUTS {
            let layout = identify(&parse_table(&table(parts))).unwrap();
            assert_eq!(layout.model, *model);
            assert_eq!(layout.otadata, 0xE000);
            assert_eq!(layout.slots[0].kind, Kind::App0);
        }
        // A slot a sector smaller is someone else's layout.
        let mut odd = LAYOUTS[0].1;
        odd[3].size -= 0x1000;
        assert!(identify(&parse_table(&table(&odd))).is_err());
        assert!(identify(&parse_table(&[0xFF; 4096])).is_err());
    }

    fn entry(seq: u32, state: u32) -> Vec<u8> {
        let mut s = vec![0xFF; SECTOR];
        s[0..4].copy_from_slice(&seq.to_le_bytes());
        s[24..28].copy_from_slice(&state.to_le_bytes());
        s[28..32].copy_from_slice(&seq_crc(seq).to_le_bytes());
        s
    }

    #[test]
    fn the_crc_is_esp_idfs() {
        // As ESP-IDF's OTA data holds them (zlib's CRC-32 from 0xFFFFFFFF,
        // the ROM's convention): `9a 98 43 47` after sequence 1.
        assert_eq!(seq_crc(1), 0x4743_989A);
        assert_eq!(seq_crc(2), 0x55F6_3774);
    }

    #[test]
    fn the_other_slot_is_chosen_and_switched_to() {
        let blank = vec![0xFF; 2 * SECTOR];
        // Fresh: slot 0 boots; the next firmware goes to slot 1, via seq 2
        // in sector 0.
        let ota = Ota::parse(&blank).unwrap();
        assert_eq!((ota.active, ota.inactive()), (0, 1));
        let (at, sector) = ota.switch(&blank, 0xE000);
        assert_eq!(at, 0xE000);
        let mut after = blank.clone();
        after[..SECTOR].copy_from_slice(&sector);
        let ota = Ota::parse(&after).unwrap();
        assert_eq!((ota.active, ota.seq), (1, 2));
        // And back: slot 0, seq 3, in the other sector.
        let (at, sector) = ota.switch(&after, 0xE000);
        assert_eq!(at, 0xF000);
        after[SECTOR..].copy_from_slice(&sector);
        let ota = Ota::parse(&after).unwrap();
        assert_eq!((ota.active, ota.seq), (0, 3));

        // The newer entry wins; a bad CRC or an aborted one does not count.
        let data = [entry(4, 2), entry(5, 2)].concat();
        assert_eq!(Ota::parse(&data).unwrap().active, 0);
        let mut bad = data.clone();
        bad[SECTOR + 28] ^= 1;
        assert_eq!(Ota::parse(&bad).unwrap().seq, 4);
        let aborted = [entry(4, 2), entry(5, STATE_ABORTED)].concat();
        let ota = Ota::parse(&aborted).unwrap();
        assert_eq!((ota.active, ota.seq), (1, 4));
        // From there the next goes to slot 0 with seq 5, replacing the
        // aborted entry.
        let (at, sector) = ota.switch(&aborted, 0xE000);
        assert_eq!(at, 0xF000);
        assert_eq!(u32::from_le_bytes(sector[..4].try_into().unwrap()), 5);
    }
}
