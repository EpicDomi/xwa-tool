//! The checksum the format uses everywhere (spec 2): CRC-32 as in zlib,
//! PNG and the ESP32's ROM (reflected polynomial 0xEDB88320, initial value
//! and final XOR 0xFFFFFFFF).

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// The CRC-32 of what `crc` was the CRC-32 of, followed by `data`.
pub fn crc32_append(crc: u32, data: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new_with_initial(crc);
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_zlibs_crc32_and_appends() {
        // The check value of CRC-32/ISO-HDLC, as zlib computes it.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_append(crc32(b"12345"), b"6789"), crc32(b"123456789"));
        assert_eq!(crc32_append(0, b"123456789"), crc32(b"123456789"));
    }
}
