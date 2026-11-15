//! META section (spec 4): an ordered list of key/value strings.

use crate::error::corrupt;
use crate::{Result, varint};

pub fn encode(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint::put(&mut out, pairs.len() as u64);
    for (k, v) in pairs {
        for s in [k, v] {
            varint::put(&mut out, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
    }
    out
}

pub fn decode(buf: &[u8]) -> Result<Vec<(String, String)>> {
    let mut p = 0;
    let count = varint::get(buf, &mut p).ok_or_else(|| corrupt("META count"))?;
    let string = |p: &mut usize| -> Result<String> {
        let len = varint::get(buf, p).ok_or_else(|| corrupt("META string length"))? as usize;
        let bytes = buf
            .get(*p..*p + len)
            .ok_or_else(|| corrupt("META string truncated"))?;
        *p += len;
        String::from_utf8(bytes.to_vec()).map_err(|_| corrupt("META string is not UTF-8"))
    };
    let mut pairs = Vec::new();
    for _ in 0..count {
        let k = string(&mut p)?;
        let v = string(&mut p)?;
        pairs.push((k, v));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips() {
        let pairs = vec![
            ("title".to_string(), "Wikipédia".to_string()),
            ("empty".to_string(), String::new()),
        ];
        assert_eq!(super::decode(&super::encode(&pairs)).unwrap(), pairs);
        assert!(super::decode(&[5]).is_err());
    }
}
