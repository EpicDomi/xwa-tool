//! Title index (spec section 6): front-coded leaf blocks under a static
//! B+tree of inner blocks, all 4096 bytes with a trailing CRC32C.

use crate::normalize::search_key;
use crate::{BLOCK_SIZE, varint};

const LEAF_HEADER: usize = 4;
const INNER_HEADER: usize = 6;
const PAYLOAD_END: usize = BLOCK_SIZE - 4;
pub const FLAG_REDIRECT: u8 = 1;

/// An inner-block child: its first key and its index within its own level.
type ChildRef = (Vec<u8>, u32);
type InnerBlock = Vec<ChildRef>;

/// One searchable title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleEntry {
    pub key: String,
    pub display: String,
    pub article: u32,
    pub flags: u8,
}

impl TitleEntry {
    pub fn new(display: impl Into<String>, article: u32, redirect: bool) -> Self {
        let display = display.into();
        Self {
            key: search_key(&display),
            display,
            article,
            flags: if redirect { FLAG_REDIRECT } else { 0 },
        }
    }
}

/// The two sections that make up a built index.
pub struct BuiltIndex {
    pub leaves: Vec<u8>,
    pub inner: Vec<u8>,
    pub entry_count: usize,
    pub levels: usize,
}

impl BuiltIndex {
    pub fn leaf_blocks(&self) -> usize {
        self.leaves.len() / BLOCK_SIZE
    }

    pub fn inner_blocks(&self) -> usize {
        self.inner.len() / BLOCK_SIZE
    }

    pub fn reader(&self) -> IndexReader<'_> {
        IndexReader {
            leaves: &self.leaves,
            inner: &self.inner,
        }
    }
}

fn shared_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn seal(block: &mut [u8]) {
    let crc = crc32c::crc32c(&block[..PAYLOAD_END]);
    block[PAYLOAD_END..].copy_from_slice(&crc.to_le_bytes());
}

/// Sorts `entries` and builds the index. Redirects whose key equals the key
/// of the title they point to should be dropped by the caller beforehand.
pub fn build(mut entries: Vec<TitleEntry>) -> BuiltIndex {
    // Keep one entry per (key, article): spellings that differ only in case or
    // accents ("Godel code", "Gödel code") are the same search hit. Prefer the
    // article's own title, then the spelling with accents, then the first.
    let preference = |e: &TitleEntry| {
        (
            e.flags & FLAG_REDIRECT,
            std::cmp::Reverse(e.display.chars().filter(|c| !c.is_ascii()).count()),
        )
    };
    entries.sort_by(|a, b| {
        (
            a.key.as_bytes(),
            a.article,
            preference(a),
            a.display.as_bytes(),
        )
            .cmp(&(
                b.key.as_bytes(),
                b.article,
                preference(b),
                b.display.as_bytes(),
            ))
    });
    entries.dedup_by(|later, kept| later.key == kept.key && later.article == kept.article);
    entries.sort_by(|a, b| {
        (a.key.as_bytes(), a.display.as_bytes()).cmp(&(b.key.as_bytes(), b.display.as_bytes()))
    });
    let entry_count = entries.len();

    // Leaves.
    let mut leaves = Vec::new();
    let mut first_keys: Vec<Vec<u8>> = Vec::new();
    let mut block = vec![0u8; BLOCK_SIZE];
    let (mut used, mut count) = (0usize, 0u16);
    let (mut prev_key, mut prev_disp): (&[u8], &[u8]) = (b"", b"");
    let mut scratch = Vec::new();
    for e in &entries {
        let (key, disp) = (e.key.as_bytes(), e.display.as_bytes());
        loop {
            let (ks, ds) = if count == 0 {
                (0, 0)
            } else {
                (shared_prefix(prev_key, key), shared_prefix(prev_disp, disp))
            };
            scratch.clear();
            varint::put(&mut scratch, ks as u64);
            varint::put(&mut scratch, (key.len() - ks) as u64);
            scratch.extend_from_slice(&key[ks..]);
            varint::put(&mut scratch, ds as u64);
            varint::put(&mut scratch, (disp.len() - ds) as u64);
            scratch.extend_from_slice(&disp[ds..]);
            varint::put(&mut scratch, e.article as u64);
            scratch.push(e.flags);
            if LEAF_HEADER + used + scratch.len() <= PAYLOAD_END {
                if count == 0 {
                    first_keys.push(key.to_vec());
                }
                block[LEAF_HEADER + used..LEAF_HEADER + used + scratch.len()]
                    .copy_from_slice(&scratch);
                used += scratch.len();
                count += 1;
                break;
            }
            assert!(count > 0, "title entry larger than a block");
            finish_leaf(&mut block, count, used, &mut leaves);
            (used, count) = (0, 0);
        }
        (prev_key, prev_disp) = (key, disp);
    }
    if count > 0 || leaves.is_empty() {
        finish_leaf(&mut block, count, used, &mut leaves);
        if first_keys.is_empty() {
            first_keys.push(Vec::new());
        }
    }

    // Inner levels, bottom-up. Each level lists (first key, local child index).
    let mut levels: Vec<Vec<InnerBlock>> = Vec::new();
    let mut children: Vec<ChildRef> = first_keys
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i as u32))
        .collect();
    while children.len() > 1 {
        let mut blocks: Vec<InnerBlock> = Vec::new();
        let mut cur: InnerBlock = Vec::new();
        let mut size = 0usize;
        for (key, child) in children {
            let ks = cur.last().map_or(0, |(p, _)| shared_prefix(p, &key));
            let need =
                varint::len(ks as u64) + varint::len((key.len() - ks) as u64) + key.len() - ks + 4;
            let need_first = varint::len(0) + varint::len(key.len() as u64) + key.len() + 4;
            if !cur.is_empty() && INNER_HEADER + size + need > PAYLOAD_END {
                blocks.push(std::mem::take(&mut cur));
                size = 0;
            }
            size += if cur.is_empty() { need_first } else { need };
            cur.push((key, child));
        }
        blocks.push(cur);
        children = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b[0].0.clone(), i as u32))
            .collect();
        levels.push(blocks);
    }

    // Lay inner blocks out top-down: root first, level 1 (parents of leaves) last.
    let mut offsets = vec![0u32; levels.len()];
    let mut total = 0u32;
    for lvl in (0..levels.len()).rev() {
        offsets[lvl] = total;
        total += levels[lvl].len() as u32;
    }
    let mut inner = Vec::with_capacity(total as usize * BLOCK_SIZE);
    for lvl in (0..levels.len()).rev() {
        for blk in &levels[lvl] {
            let mut b = vec![0u8; BLOCK_SIZE];
            b[0] = (lvl + 1) as u8;
            b[2..4].copy_from_slice(&(blk.len() as u16).to_le_bytes());
            let mut p = INNER_HEADER;
            let mut prev: &[u8] = b"";
            for (i, (key, child)) in blk.iter().enumerate() {
                let ks = if i == 0 { 0 } else { shared_prefix(prev, key) };
                let mut s = Vec::new();
                varint::put(&mut s, ks as u64);
                varint::put(&mut s, (key.len() - ks) as u64);
                s.extend_from_slice(&key[ks..]);
                let target = if lvl == 0 {
                    *child
                } else {
                    offsets[lvl - 1] + *child
                };
                s.extend_from_slice(&target.to_le_bytes());
                b[p..p + s.len()].copy_from_slice(&s);
                p += s.len();
                prev = key;
            }
            b[4..6].copy_from_slice(&((p - INNER_HEADER) as u16).to_le_bytes());
            seal(&mut b);
            inner.extend_from_slice(&b);
        }
    }
    BuiltIndex {
        leaves,
        inner,
        entry_count,
        levels: levels.len() + 1,
    }
}

fn finish_leaf(block: &mut [u8], count: u16, used: usize, leaves: &mut Vec<u8>) {
    block[0..2].copy_from_slice(&count.to_le_bytes());
    block[2..4].copy_from_slice(&(used as u16).to_le_bytes());
    seal(block);
    leaves.extend_from_slice(block);
    block.iter_mut().for_each(|b| *b = 0);
}

/// Error from reading index blocks.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    Checksum { inner: bool, block: usize },
    Malformed { inner: bool, block: usize },
}

/// Prefix search over index blocks held in memory (or a cache of them).
pub struct IndexReader<'a> {
    pub leaves: &'a [u8],
    pub inner: &'a [u8],
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub display: String,
    pub article: u32,
    pub flags: u8,
}

impl IndexReader<'_> {
    fn block(data: &[u8], index: usize, inner: bool) -> Result<&[u8], ReadError> {
        let b = data
            .get(index * BLOCK_SIZE..(index + 1) * BLOCK_SIZE)
            .ok_or(ReadError::Malformed {
                inner,
                block: index,
            })?;
        let stored = u32::from_le_bytes(b[PAYLOAD_END..].try_into().unwrap());
        if crc32c::crc32c(&b[..PAYLOAD_END]) != stored {
            return Err(ReadError::Checksum {
                inner,
                block: index,
            });
        }
        Ok(b)
    }

    /// Leaf block that holds the first key `>= key` (or the block before it).
    fn descend(&self, key: &[u8]) -> Result<usize, ReadError> {
        if self.inner.is_empty() {
            return Ok(0);
        }
        let mut index = 0usize;
        loop {
            let b = Self::block(self.inner, index, true)?;
            let level = b[0];
            let count = u16::from_le_bytes([b[2], b[3]]) as usize;
            let mut p = INNER_HEADER;
            let mut prev: Vec<u8> = Vec::new();
            let mut chosen = None;
            for _ in 0..count {
                let bad = ReadError::Malformed {
                    inner: true,
                    block: index,
                };
                let ks = varint::get(b, &mut p).ok_or(bad)? as usize;
                let sl = varint::get(b, &mut p).ok_or(ReadError::Malformed {
                    inner: true,
                    block: index,
                })? as usize;
                let suffix = b.get(p..p + sl).ok_or(ReadError::Malformed {
                    inner: true,
                    block: index,
                })?;
                p += sl;
                prev.truncate(ks);
                prev.extend_from_slice(suffix);
                let child = u32::from_le_bytes(
                    b.get(p..p + 4)
                        .ok_or(ReadError::Malformed {
                            inner: true,
                            block: index,
                        })?
                        .try_into()
                        .unwrap(),
                );
                p += 4;
                if prev.as_slice() <= key || chosen.is_none() {
                    chosen = Some(child);
                } else {
                    break;
                }
            }
            let child = chosen.ok_or(ReadError::Malformed {
                inner: true,
                block: index,
            })? as usize;
            if level == 1 {
                return Ok(child);
            }
            index = child;
        }
    }

    /// Up to `limit` titles whose search key starts with `search_key(prefix)`.
    pub fn prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Hit>, ReadError> {
        let want = search_key(prefix).into_bytes();
        let mut leaf = self.descend(&want)?;
        let mut hits = Vec::new();
        let leaf_count = self.leaves.len() / BLOCK_SIZE;
        while leaf < leaf_count && hits.len() < limit {
            let b = Self::block(self.leaves, leaf, false)?;
            let count = u16::from_le_bytes([b[0], b[1]]) as usize;
            let mut p = LEAF_HEADER;
            let (mut key, mut disp): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
            for _ in 0..count {
                let bad = || ReadError::Malformed {
                    inner: false,
                    block: leaf,
                };
                let field = |prev: &mut Vec<u8>, p: &mut usize| -> Result<(), ReadError> {
                    let shared = varint::get(b, p).ok_or_else(bad)? as usize;
                    let len = varint::get(b, p).ok_or_else(bad)? as usize;
                    let s = b.get(*p..*p + len).ok_or_else(bad)?;
                    *p += len;
                    prev.truncate(shared);
                    prev.extend_from_slice(s);
                    Ok(())
                };
                field(&mut key, &mut p)?;
                field(&mut disp, &mut p)?;
                let article = varint::get(b, &mut p).ok_or_else(bad)? as u32;
                let flags = *b.get(p).ok_or_else(bad)?;
                p += 1;
                if key.as_slice() < want.as_slice() {
                    continue;
                }
                if !key.starts_with(&want) {
                    return Ok(hits);
                }
                hits.push(Hit {
                    display: String::from_utf8_lossy(&disp).into_owned(),
                    article,
                    flags,
                });
                if hits.len() == limit {
                    break;
                }
            }
            leaf += 1;
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(n: u32) -> Vec<TitleEntry> {
        let words = [
            "Albert",
            "Einstein",
            "Émile",
            "Zola",
            "Straße",
            "list",
            "of",
            "theorem",
            "Pythagorean",
            "Ångström",
            "x",
        ];
        let mut seed = 0x9E37_79B9u32;
        (0..n)
            .map(|i| {
                let mut t = String::new();
                for _ in 0..1 + i % 4 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    if !t.is_empty() {
                        t.push(' ');
                    }
                    t.push_str(words[(seed % words.len() as u32) as usize]);
                    t.push_str(&(seed % 997).to_string());
                }
                TitleEntry::new(t, i, i % 3 == 0)
            })
            .collect()
    }

    #[test]
    fn prefix_search_matches_brute_force() {
        let entries = synthetic(40_000);
        let idx = build(entries.clone());
        assert!(idx.inner_blocks() > 0, "test should exercise inner levels");
        let r = idx.reader();
        let mut sorted = entries.clone();
        sorted.sort_by(|a, b| {
            (a.key.as_bytes(), a.display.as_bytes()).cmp(&(b.key.as_bytes(), b.display.as_bytes()))
        });
        let mut seen = std::collections::HashSet::new();
        sorted.retain(|e| seen.insert((e.key.clone(), e.article)));
        for prefix in [
            "albert", "EMILE z", "strasse1", "angstrom", "x9", "pyth", "zzz", "", "list of",
        ] {
            let want: Vec<_> = sorted
                .iter()
                .filter(|e| e.key.starts_with(&search_key(prefix)))
                .take(25)
                .map(|e| (e.key.clone(), e.article))
                .collect();
            let got: Vec<_> = r
                .prefix(prefix, 25)
                .unwrap()
                .into_iter()
                .map(|h| (search_key(&h.display), h.article))
                .collect();
            assert_eq!(got, want, "prefix {prefix:?}");
        }
        for e in sorted.iter().step_by(97) {
            let hits = r.prefix(&e.display, 1000).unwrap();
            assert!(
                hits.iter().any(|h| h.article == e.article),
                "{:?} not found",
                e.display
            );
        }
    }

    #[test]
    fn detects_corruption() {
        let idx = build(synthetic(5_000));
        let mut leaves = idx.leaves.clone();
        leaves[100] ^= 0xFF;
        let r = IndexReader {
            leaves: &leaves,
            inner: &idx.inner,
        };
        assert!(matches!(
            r.prefix("", usize::MAX),
            Err(ReadError::Checksum { .. })
        ));
    }

    #[test]
    fn empty_and_single_block() {
        assert!(
            build(Vec::new())
                .reader()
                .prefix("a", 10)
                .unwrap()
                .is_empty()
        );
        let idx = build(vec![TitleEntry::new("Only", 7, false)]);
        assert_eq!(idx.inner_blocks(), 0);
        assert_eq!(idx.reader().prefix("on", 10).unwrap()[0].article, 7);
    }
}
