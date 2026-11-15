//! The 4096-byte superblock at the start of every archive (spec 3.1).

use crate::error::corrupt;
use crate::{Error, MAGIC, Profile, Result, TextCodec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionEntry {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub length: u64,
    /// CRC32C of the section bytes; 0 means not computed.
    pub crc32c: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superblock {
    pub version_major: u16,
    pub version_minor: u16,
    pub file_size: u64,
    pub required_features: u32,
    pub optional_features: u32,
    pub profile: Profile,
    pub text_codec: TextCodec,
    pub deflate_window_bits: u8,
    pub sections: Vec<SectionEntry>,
}

const SECTIONS_AT: usize = 40;
const SECTION_SIZE: usize = 32;
const CRC_AT: usize = Superblock::SIZE - 4;

impl Superblock {
    pub const SIZE: usize = 4096;
    pub const MAX_SECTIONS: usize = (CRC_AT - SECTIONS_AT) / SECTION_SIZE;

    pub fn section(&self, kind: u32) -> Option<&SectionEntry> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.sections.len() > Self::MAX_SECTIONS {
            return Err(Error::Invalid(format!(
                "{} sections; at most {}",
                self.sections.len(),
                Self::MAX_SECTIONS
            )));
        }
        let mut b = vec![0u8; Self::SIZE];
        b[0..8].copy_from_slice(&MAGIC);
        b[8..10].copy_from_slice(&self.version_major.to_le_bytes());
        b[10..12].copy_from_slice(&self.version_minor.to_le_bytes());
        b[12..16].copy_from_slice(&(Self::SIZE as u32).to_le_bytes());
        b[16..24].copy_from_slice(&self.file_size.to_le_bytes());
        b[24..28].copy_from_slice(&self.required_features.to_le_bytes());
        b[28..32].copy_from_slice(&self.optional_features.to_le_bytes());
        b[32] = self.profile.code();
        b[33] = self.text_codec.code();
        b[34] = self.deflate_window_bits;
        b[36..40].copy_from_slice(&(self.sections.len() as u32).to_le_bytes());
        for (i, s) in self.sections.iter().enumerate() {
            let o = SECTIONS_AT + i * SECTION_SIZE;
            b[o..o + 4].copy_from_slice(&s.kind.to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&s.flags.to_le_bytes());
            b[o + 8..o + 16].copy_from_slice(&s.offset.to_le_bytes());
            b[o + 16..o + 24].copy_from_slice(&s.length.to_le_bytes());
            b[o + 24..o + 28].copy_from_slice(&s.crc32c.to_le_bytes());
        }
        let crc = crc32c::crc32c(&b[..CRC_AT]);
        b[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        Ok(b)
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < Self::SIZE || b[0..8] != MAGIC {
            return Err(Error::BadMagic);
        }
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        if crc32c::crc32c(&b[..CRC_AT]) != u32_at(CRC_AT) {
            return Err(corrupt("superblock checksum mismatch"));
        }
        if u32_at(12) as usize != Self::SIZE {
            return Err(Error::Unsupported(format!(
                "superblock size {}",
                u32_at(12)
            )));
        }
        let count = u32_at(36) as usize;
        if count > Self::MAX_SECTIONS {
            return Err(corrupt(format!("{count} sections")));
        }
        let profile = Profile::from_code(b[32])
            .ok_or_else(|| Error::Unsupported(format!("profile {}", b[32])))?;
        let text_codec = TextCodec::from_code(b[33])
            .ok_or_else(|| Error::Unsupported(format!("text codec {}", b[33])))?;
        let sections = (0..count)
            .map(|i| {
                let o = SECTIONS_AT + i * SECTION_SIZE;
                SectionEntry {
                    kind: u32_at(o),
                    flags: u32_at(o + 4),
                    offset: u64_at(o + 8),
                    length: u64_at(o + 16),
                    crc32c: u32_at(o + 24),
                }
            })
            .collect();
        Ok(Self {
            version_major: u16_at(8),
            version_minor: u16_at(10),
            file_size: u64_at(16),
            required_features: u32_at(24),
            optional_features: u32_at(28),
            profile,
            text_codec,
            deflate_window_bits: b[34],
            sections,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_detects_damage() {
        let sb = Superblock {
            version_major: 0,
            version_minor: 1,
            file_size: 1 << 33,
            required_features: 0b101,
            optional_features: 0,
            profile: Profile::Full,
            text_codec: TextCodec::Zstd,
            deflate_window_bits: 15,
            sections: vec![SectionEntry {
                kind: 7,
                flags: 0,
                offset: 4096,
                length: 99,
                crc32c: 0xDEAD_BEEF,
            }],
        };
        let mut b = sb.encode().unwrap();
        assert_eq!(Superblock::decode(&b).unwrap(), sb);
        b[100] ^= 1;
        assert!(matches!(Superblock::decode(&b), Err(Error::Corrupt(_))));
        assert!(matches!(
            Superblock::decode(&[0u8; 4096]),
            Err(Error::BadMagic)
        ));
    }
}
