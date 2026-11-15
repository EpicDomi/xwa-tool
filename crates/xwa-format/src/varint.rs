//! LEB128 varints (unsigned) and zigzag-encoded signed varints.

pub fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn put_signed(out: &mut Vec<u8>, v: i64) {
    put(out, ((v << 1) ^ (v >> 63)) as u64);
}

pub fn len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Reads a varint at `*pos`, advancing it. `None` on truncation or overflow.
pub fn get(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

pub fn get_signed(buf: &[u8], pos: &mut usize) -> Option<i64> {
    let u = get(buf, pos)?;
    Some(((u >> 1) as i64) ^ -((u & 1) as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            u32::MAX as u64,
            u64::MAX,
        ] {
            let mut b = Vec::new();
            put(&mut b, v);
            assert_eq!(b.len(), len(v));
            let mut p = 0;
            assert_eq!(get(&b, &mut p), Some(v));
            assert_eq!(p, b.len());
        }
        for v in [0i64, -1, 1, -64, 64, i32::MIN as i64, i64::MAX, i64::MIN] {
            let mut b = Vec::new();
            put_signed(&mut b, v);
            let mut p = 0;
            assert_eq!(get_signed(&b, &mut p), Some(v));
        }
    }

    #[test]
    fn rejects_truncated() {
        let mut p = 0;
        assert_eq!(get(&[0x80, 0x80], &mut p), None);
    }
}
