//! Title index (spec section 6): front-coded leaf blocks under a static
//! B+tree of inner blocks, all 4096 bytes with a trailing CRC-32.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::normalize::{KeyChars, search_key};
use crate::{BLOCK_SIZE, varint};

const LEAF_HEADER: usize = 4;
const INNER_HEADER: usize = 6;
/// After an inner entry's key: `u32` child block, `u8` highest popularity.
const INNER_CHILD: usize = 5;
const PAYLOAD_END: usize = BLOCK_SIZE - 4;
pub const FLAG_REDIRECT: u8 = 1;
/// The entry leads to a named place in its article: a `u32` anchor hash
/// (spec 7.2) follows the flags.
pub const FLAG_ANCHOR: u8 = 2;

/// An inner-block child: its separator, its index within its own level, and
/// the highest popularity in it.
type ChildRef = (Vec<u8>, u32, u8);
type InnerBlock = Vec<ChildRef>;
/// An inner block as read: whether its children are inner blocks, and per
/// child its separator, block number and highest popularity.
type Children = (bool, Vec<(Vec<u8>, usize, u8)>);

/// One searchable title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleEntry {
    pub key: String,
    pub display: String,
    pub article: u32,
    pub flags: u8,
    /// The named place it leads to, for a redirect to a section.
    pub anchor: Option<u32>,
    /// Its article's popularity (spec 5.1).
    pub popularity: u8,
}

impl TitleEntry {
    pub fn new(display: impl Into<String>, article: u32, redirect: bool) -> Self {
        let display = display.into();
        Self {
            key: search_key(&display),
            display,
            article,
            flags: if redirect { FLAG_REDIRECT } else { 0 },
            anchor: None,
            popularity: 0,
        }
    }

    /// The same entry leading to the place with this anchor hash.
    pub fn with_anchor(mut self, anchor: Option<u32>) -> Self {
        self.anchor = anchor;
        self
    }

    /// The same entry with its article's popularity.
    pub fn with_popularity(mut self, popularity: u8) -> Self {
        self.popularity = popularity;
        self
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

    pub fn reader(&self) -> IndexReader<Slices<'_>> {
        IndexReader::new(Slices {
            leaves: &self.leaves,
            inner: &self.inner,
        })
    }
}

fn shared_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// What an inner block keeps of a leaf block's first key (spec 6.4): its
/// shortest prefix that sorts after `prev`, the last key of the leaf block
/// before it, or the whole key when the two are equal.
fn separator(prev: &[u8], key: &[u8]) -> Vec<u8> {
    let shared = shared_prefix(prev, key);
    key[..(shared + 1).min(key.len())].to_vec()
}

fn seal(block: &mut [u8]) {
    let crc = crate::crc::crc32(&block[..PAYLOAD_END]);
    block[PAYLOAD_END..].copy_from_slice(&crc.to_le_bytes());
}

/// The order entries are deduplicated in: by key, then article, then
/// preferring the article's own title, then the spelling with the most
/// accents, then the display bytes.
fn dedup_order(
    (a_key, a_article, a_flags, a_accents, a_disp): (&[u8], u32, u8, u16, &[u8]),
    (b_key, b_article, b_flags, b_accents, b_disp): (&[u8], u32, u8, u16, &[u8]),
) -> std::cmp::Ordering {
    a_key
        .cmp(b_key)
        .then(a_article.cmp(&b_article))
        .then((a_flags & FLAG_REDIRECT).cmp(&(b_flags & FLAG_REDIRECT)))
        .then(b_accents.cmp(&a_accents))
        .then(a_disp.cmp(b_disp))
}

fn accents(display: &str) -> u16 {
    display
        .chars()
        .filter(|c| !c.is_ascii())
        .count()
        .min(u16::MAX as usize) as u16
}

/// Sorts `entries` and builds the index in memory. Redirects whose key
/// equals the key of the title they point to should be dropped by the
/// caller beforehand. [`TitleSorter`] does the same for any number of
/// entries in a bounded amount of memory.
pub fn build(mut entries: Vec<TitleEntry>) -> BuiltIndex {
    // Keep one entry per (key, article): spellings that differ only in case or
    // accents ("Godel code", "Gödel code") are the same search hit. Prefer the
    // article's own title, then the spelling with accents, then the first.
    fn fields(e: &TitleEntry) -> (&[u8], u32, u8, u16, &[u8]) {
        (
            e.key.as_bytes(),
            e.article,
            e.flags,
            accents(&e.display),
            e.display.as_bytes(),
        )
    }
    entries.sort_by(|a, b| dedup_order(fields(a), fields(b)));
    entries.dedup_by(|later, kept| later.key == kept.key && later.article == kept.article);
    entries.sort_by(|a, b| {
        (a.key.as_bytes(), a.display.as_bytes()).cmp(&(b.key.as_bytes(), b.display.as_bytes()))
    });
    let mut leaves = LeafWriter::new(Vec::new());
    for e in &entries {
        leaves
            .push(
                e.key.as_bytes(),
                e.display.as_bytes(),
                e.article,
                e.flags,
                e.anchor,
                e.popularity,
            )
            .expect("titles fit in a block");
    }
    let done = leaves.finish().expect("writing to memory");
    let (inner, levels) = build_inner(done.separators, done.leaf_popularity);
    BuiltIndex {
        leaves: done.out,
        inner,
        entry_count: done.entry_count,
        levels,
    }
}

/// Writes leaf blocks as entries arrive in index order (sorted by key, then
/// display, one per key and article).
pub struct LeafWriter<W: Write> {
    out: W,
    block: Vec<u8>,
    used: usize,
    count: u16,
    prev_key: Vec<u8>,
    prev_disp: Vec<u8>,
    separators: Vec<Vec<u8>>,
    /// The highest popularity in each finished leaf, and in this one.
    leaf_popularity: Vec<u8>,
    popularity: u8,
    key_chars: KeyChars,
    scratch: Vec<u8>,
    entry_count: usize,
    redirect_count: usize,
    bytes: u64,
    crc32: u32,
}

/// What [`LeafWriter::finish`] hands back.
pub struct Leaves<W> {
    pub out: W,
    /// Each leaf block's [`separator`], for [`build_inner`].
    pub separators: Vec<Vec<u8>>,
    /// The highest popularity in each leaf block, for [`build_inner`].
    pub leaf_popularity: Vec<u8>,
    /// The characters the keys use, for the FOLD section.
    pub key_chars: KeyChars,
    pub entry_count: usize,
    /// Entries with [`FLAG_REDIRECT`].
    pub redirect_count: usize,
    /// Bytes written, a whole number of blocks.
    pub bytes: u64,
    /// CRC-32 of the bytes written.
    pub crc32: u32,
}

impl<W: Write> LeafWriter<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            block: vec![0u8; BLOCK_SIZE],
            used: 0,
            count: 0,
            prev_key: Vec::new(),
            prev_disp: Vec::new(),
            separators: Vec::new(),
            leaf_popularity: Vec::new(),
            popularity: 0,
            key_chars: KeyChars::default(),
            scratch: Vec::new(),
            entry_count: 0,
            redirect_count: 0,
            bytes: 0,
            crc32: 0,
        }
    }

    pub fn push(
        &mut self,
        key: &[u8],
        display: &[u8],
        article: u32,
        flags: u8,
        anchor: Option<u32>,
        popularity: u8,
    ) -> io::Result<()> {
        let flags = match anchor {
            Some(_) => flags | FLAG_ANCHOR,
            None => flags & !FLAG_ANCHOR,
        };
        loop {
            let (ks, ds) = if self.count == 0 {
                (0, 0)
            } else {
                (
                    shared_prefix(&self.prev_key, key),
                    shared_prefix(&self.prev_disp, display),
                )
            };
            let s = &mut self.scratch;
            s.clear();
            varint::put(s, ks as u64);
            varint::put(s, (key.len() - ks) as u64);
            s.extend_from_slice(&key[ks..]);
            varint::put(s, ds as u64);
            varint::put(s, (display.len() - ds) as u64);
            s.extend_from_slice(&display[ds..]);
            varint::put(s, article as u64);
            s.push(flags);
            s.push(popularity);
            if let Some(hash) = anchor {
                s.extend_from_slice(&hash.to_le_bytes());
            }
            if LEAF_HEADER + self.used + s.len() <= PAYLOAD_END {
                if self.count == 0 {
                    // Nothing sorts before the first leaf block.
                    self.separators.push(if self.entry_count == 0 {
                        Vec::new()
                    } else {
                        separator(&self.prev_key, key)
                    });
                }
                self.popularity = self.popularity.max(popularity);
                let at = LEAF_HEADER + self.used;
                self.block[at..at + s.len()].copy_from_slice(s);
                self.used += s.len();
                self.count += 1;
                break;
            }
            if self.count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a title is too long for the title index",
                ));
            }
            self.finish_leaf()?;
        }
        self.key_chars
            .insert_str(std::str::from_utf8(key).map_err(io::Error::other)?);
        self.prev_key.clear();
        self.prev_key.extend_from_slice(key);
        self.prev_disp.clear();
        self.prev_disp.extend_from_slice(display);
        self.entry_count += 1;
        if flags & FLAG_REDIRECT != 0 {
            self.redirect_count += 1;
        }
        Ok(())
    }

    fn finish_leaf(&mut self) -> io::Result<()> {
        self.block[0..2].copy_from_slice(&self.count.to_le_bytes());
        self.block[2..4].copy_from_slice(&(self.used as u16).to_le_bytes());
        seal(&mut self.block);
        self.out.write_all(&self.block)?;
        self.crc32 = crate::crc::crc32_append(self.crc32, &self.block);
        self.bytes += BLOCK_SIZE as u64;
        self.block.fill(0);
        (self.used, self.count) = (0, 0);
        self.leaf_popularity
            .push(std::mem::take(&mut self.popularity));
        Ok(())
    }

    /// Writes the last block. An empty index still has one, empty, leaf.
    pub fn finish(mut self) -> io::Result<Leaves<W>> {
        if self.count > 0 || self.bytes == 0 {
            self.finish_leaf()?;
            if self.separators.is_empty() {
                self.separators.push(Vec::new());
            }
        }
        Ok(Leaves {
            out: self.out,
            separators: self.separators,
            leaf_popularity: self.leaf_popularity,
            key_chars: self.key_chars,
            entry_count: self.entry_count,
            redirect_count: self.redirect_count,
            bytes: self.bytes,
            crc32: self.crc32,
        })
    }
}

/// The inner blocks over leaves with these separators and highest
/// popularities, and the number of levels including the leaves.
pub fn build_inner(separators: Vec<Vec<u8>>, leaf_popularity: Vec<u8>) -> (Vec<u8>, usize) {
    assert_eq!(separators.len(), leaf_popularity.len());
    // Inner levels, bottom-up. Each level lists (separator, local child
    // index, highest popularity below); a block's separator is its first
    // child's.
    let mut levels: Vec<Vec<InnerBlock>> = Vec::new();
    let mut children: Vec<ChildRef> = separators
        .into_iter()
        .zip(leaf_popularity)
        .enumerate()
        .map(|(i, (k, p))| (k, i as u32, p))
        .collect();
    while children.len() > 1 {
        let mut blocks: Vec<InnerBlock> = Vec::new();
        let mut cur: InnerBlock = Vec::new();
        let mut size = 0usize;
        for (key, child, popularity) in children {
            let ks = cur.last().map_or(0, |(p, _, _)| shared_prefix(p, &key));
            let need = varint::len(ks as u64) + varint::len((key.len() - ks) as u64) + key.len()
                - ks
                + INNER_CHILD;
            let need_first =
                varint::len(0) + varint::len(key.len() as u64) + key.len() + INNER_CHILD;
            if !cur.is_empty() && INNER_HEADER + size + need > PAYLOAD_END {
                blocks.push(std::mem::take(&mut cur));
                size = 0;
            }
            size += if cur.is_empty() { need_first } else { need };
            cur.push((key, child, popularity));
        }
        blocks.push(cur);
        children = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let top = b.iter().map(|(_, _, p)| *p).max().unwrap_or(0);
                (b[0].0.clone(), i as u32, top)
            })
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
            for (i, (key, child, popularity)) in blk.iter().enumerate() {
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
                s.push(*popularity);
                b[p..p + s.len()].copy_from_slice(&s);
                p += s.len();
                prev = key;
            }
            b[4..6].copy_from_slice(&((p - INNER_HEADER) as u16).to_le_bytes());
            seal(&mut b);
            inner.extend_from_slice(&b);
        }
    }
    (inner, levels.len() + 1)
}

/// Collects title entries for an index of any size in a bounded amount of
/// memory. Entries are kept in memory up to the budget; then they are
/// sorted and written to a temporary file as one run, and at the end the
/// runs are merged. The result is the same as [`build`]'s.
pub struct TitleSorter {
    budget: usize,
    dir: Option<PathBuf>,
    /// Records, back to back: u16 key length, u16 display length, u32
    /// article, u8 flags, u16 accents, u32 anchor (when the flags say), then
    /// the key and display bytes.
    buf: Vec<u8>,
    starts: Vec<u32>,
    runs: Vec<File>,
}

const REC_HEADER: usize = 15;

/// A record's fields, borrowed from a buffer.
#[derive(Clone, Copy)]
struct RecRef<'a> {
    key: &'a [u8],
    display: &'a [u8],
    article: u32,
    flags: u8,
    accents: u16,
    anchor: Option<u32>,
}

/// The anchor in a record header, if its flags say there is one.
fn rec_anchor(h: &[u8]) -> Option<u32> {
    (h[8] & FLAG_ANCHOR != 0).then(|| u32::from_le_bytes(h[11..15].try_into().unwrap()))
}

impl<'a> RecRef<'a> {
    fn parse(b: &'a [u8]) -> Self {
        let key_len = u16::from_le_bytes([b[0], b[1]]) as usize;
        let disp_len = u16::from_le_bytes([b[2], b[3]]) as usize;
        let key = &b[REC_HEADER..REC_HEADER + key_len];
        let display = &b[REC_HEADER + key_len..REC_HEADER + key_len + disp_len];
        Self {
            key,
            display,
            article: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            flags: b[8],
            accents: u16::from_le_bytes([b[9], b[10]]),
            anchor: rec_anchor(b),
        }
    }

    fn len(&self) -> usize {
        REC_HEADER + self.key.len() + self.display.len()
    }

    fn fields(&self) -> (&'a [u8], u32, u8, u16, &'a [u8]) {
        (
            self.key,
            self.article,
            self.flags,
            self.accents,
            self.display,
        )
    }

    fn to_owned(self) -> Rec {
        Rec {
            key: self.key.to_vec(),
            display: self.display.to_vec(),
            article: self.article,
            flags: self.flags,
            accents: self.accents,
            anchor: self.anchor,
        }
    }
}

/// A record read back from a run.
#[derive(Debug, PartialEq, Eq)]
struct Rec {
    key: Vec<u8>,
    display: Vec<u8>,
    article: u32,
    flags: u8,
    accents: u16,
    anchor: Option<u32>,
}

impl Rec {
    fn fields(&self) -> (&[u8], u32, u8, u16, &[u8]) {
        (
            &self.key,
            self.article,
            self.flags,
            self.accents,
            &self.display,
        )
    }

    fn read(r: &mut impl Read) -> io::Result<Option<Self>> {
        let mut h = [0u8; REC_HEADER];
        match r.read_exact(&mut h) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let key_len = u16::from_le_bytes([h[0], h[1]]) as usize;
        let disp_len = u16::from_le_bytes([h[2], h[3]]) as usize;
        let mut key = vec![0u8; key_len];
        r.read_exact(&mut key)?;
        let mut display = vec![0u8; disp_len];
        r.read_exact(&mut display)?;
        Ok(Some(Self {
            key,
            display,
            article: u32::from_le_bytes(h[4..8].try_into().unwrap()),
            flags: h[8],
            accents: u16::from_le_bytes([h[9], h[10]]),
            anchor: rec_anchor(&h),
        }))
    }
}

impl Ord for Rec {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        dedup_order(self.fields(), other.fields())
    }
}

impl PartialOrd for Rec {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TitleSorter {
    /// Holds up to about `budget` bytes of entries in memory; runs go to
    /// `dir`, or the system's temporary folder.
    pub fn new(budget: usize, dir: Option<PathBuf>) -> Self {
        Self {
            budget,
            dir,
            buf: Vec::new(),
            starts: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// Adds a title; `anchor` is the place a redirect to a section leads to.
    pub fn push(
        &mut self,
        display: &str,
        article: u32,
        redirect: bool,
        anchor: Option<u32>,
    ) -> io::Result<()> {
        let key = search_key(display);
        if key.len() > PAYLOAD_END || display.len() > PAYLOAD_END {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a title is too long for the title index",
            ));
        }
        let start = u32::try_from(self.buf.len())
            .map_err(|_| io::Error::other("title buffer over 4 GB"))?;
        self.starts.push(start);
        let b = &mut self.buf;
        b.extend_from_slice(&(key.len() as u16).to_le_bytes());
        b.extend_from_slice(&(display.len() as u16).to_le_bytes());
        b.extend_from_slice(&article.to_le_bytes());
        let flags = if redirect { FLAG_REDIRECT } else { 0 };
        b.push(flags | if anchor.is_some() { FLAG_ANCHOR } else { 0 });
        b.extend_from_slice(&accents(display).to_le_bytes());
        b.extend_from_slice(&anchor.unwrap_or(0).to_le_bytes());
        b.extend_from_slice(key.as_bytes());
        b.extend_from_slice(display.as_bytes());
        if self.buf.len() + self.starts.len() * 4 >= self.budget {
            self.spill()?;
        }
        Ok(())
    }

    /// The records in memory, sorted.
    fn sorted_starts(&mut self) -> Vec<u32> {
        let buf = &self.buf;
        let mut starts = std::mem::take(&mut self.starts);
        starts.sort_unstable_by(|&a, &b| {
            dedup_order(
                RecRef::parse(&buf[a as usize..]).fields(),
                RecRef::parse(&buf[b as usize..]).fields(),
            )
        });
        starts
    }

    fn spill(&mut self) -> io::Result<()> {
        if self.starts.is_empty() {
            return Ok(());
        }
        let starts = self.sorted_starts();
        let file = match &self.dir {
            Some(dir) => tempfile::tempfile_in(dir)?,
            None => tempfile::tempfile()?,
        };
        let mut w = BufWriter::with_capacity(1 << 20, file);
        for s in starts {
            let rec = &self.buf[s as usize..];
            w.write_all(&rec[..RecRef::parse(rec).len()])?;
        }
        let mut file = w.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        self.runs.push(file);
        self.buf.clear();
        Ok(())
    }

    /// Calls `f(key, display, article, flags, anchor)` for every entry in
    /// index order, one per (key, article), as [`build`] would store them.
    pub fn finish(
        mut self,
        mut f: impl FnMut(&[u8], &[u8], u32, u8, Option<u32>) -> io::Result<()>,
    ) -> io::Result<()> {
        // Records arrive sorted by key, article and preference. Each key's
        // records are gathered, each article's best spelling kept, and the
        // rest ordered by display.
        let mut group: Vec<Rec> = Vec::new();
        let mut flush = |group: &mut Vec<Rec>| -> io::Result<()> {
            group.dedup_by(|later, kept| later.article == kept.article);
            group.sort_by(|a, b| a.display.cmp(&b.display));
            for r in group.drain(..) {
                f(&r.key, &r.display, r.article, r.flags, r.anchor)?;
            }
            Ok(())
        };
        let mut take = |rec: Rec, group: &mut Vec<Rec>| -> io::Result<()> {
            if group.first().is_some_and(|g| g.key != rec.key) {
                flush(group)?;
            }
            group.push(rec);
            Ok(())
        };
        if self.runs.is_empty() {
            for s in self.sorted_starts() {
                take(
                    RecRef::parse(&self.buf[s as usize..]).to_owned(),
                    &mut group,
                )?;
            }
        } else {
            self.spill()?;
            let mut readers: Vec<BufReader<File>> = std::mem::take(&mut self.runs)
                .into_iter()
                .map(|f| BufReader::with_capacity(256 << 10, f))
                .collect();
            let mut heap = BinaryHeap::new();
            for (i, r) in readers.iter_mut().enumerate() {
                if let Some(rec) = Rec::read(r)? {
                    heap.push(Reverse((rec, i)));
                }
            }
            while let Some(Reverse((rec, i))) = heap.pop() {
                if let Some(next) = Rec::read(&mut readers[i])? {
                    heap.push(Reverse((next, i)));
                }
                take(rec, &mut group)?;
            }
        }
        flush(&mut group)
    }
}

/// Error from reading index blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    Checksum { inner: bool, block: usize },
    Malformed { inner: bool, block: usize },
    Io(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = |inner: &bool| if *inner { "inner" } else { "leaf" };
        match self {
            ReadError::Checksum { inner, block } => {
                write!(
                    f,
                    "title index {} block {block}: checksum mismatch",
                    level(inner)
                )
            }
            ReadError::Malformed { inner, block } => {
                write!(f, "title index {} block {block}: malformed", level(inner))
            }
            ReadError::Io(e) => write!(f, "title index: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Where index blocks come from: memory, a file, or a cache.
pub trait BlockSource {
    /// Number of 4096-byte blocks in the leaf (`inner == false`) or inner section.
    fn block_count(&self, inner: bool) -> usize;
    /// Block `index` of that section, exactly [`BLOCK_SIZE`] bytes.
    fn read_block(&self, inner: bool, index: usize) -> Result<Cow<'_, [u8]>, ReadError>;
}

/// Index sections held in memory.
#[derive(Clone, Copy)]
pub struct Slices<'a> {
    pub leaves: &'a [u8],
    pub inner: &'a [u8],
}

impl BlockSource for Slices<'_> {
    fn block_count(&self, inner: bool) -> usize {
        (if inner {
            self.inner.len()
        } else {
            self.leaves.len()
        }) / BLOCK_SIZE
    }

    fn read_block(&self, inner: bool, index: usize) -> Result<Cow<'_, [u8]>, ReadError> {
        let data = if inner { self.inner } else { self.leaves };
        data.get(index * BLOCK_SIZE..(index + 1) * BLOCK_SIZE)
            .map(Cow::Borrowed)
            .ok_or(ReadError::Malformed {
                inner,
                block: index,
            })
    }
}

/// One title as stored in a leaf block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub display: String,
    pub article: u32,
    pub flags: u8,
    /// Its article's popularity (spec 5.1).
    pub popularity: u8,
    /// The place in the article it leads to ([`FLAG_ANCHOR`]).
    pub anchor: Option<u32>,
}

/// A leaf block's first and last keys and highest popularity.
#[derive(Debug, Clone, Default)]
pub struct LeafSummary {
    pub first: Vec<u8>,
    pub last: Vec<u8>,
    pub top: u8,
    pub entries: usize,
}

impl LeafSummary {
    /// Takes in the block's next entry.
    pub fn add(&mut self, key: &[u8], popularity: u8) {
        if self.entries == 0 {
            self.first = key.to_vec();
        }
        self.last.clear();
        self.last.extend_from_slice(key);
        self.top = self.top.max(popularity);
        self.entries += 1;
    }
}

/// Prefix search and full scans over an index.
pub struct IndexReader<S> {
    src: S,
}

impl<S: BlockSource> IndexReader<S> {
    pub fn new(src: S) -> Self {
        Self { src }
    }

    fn block(&self, inner: bool, index: usize) -> Result<Cow<'_, [u8]>, ReadError> {
        let b = self.src.read_block(inner, index)?;
        if b.len() != BLOCK_SIZE {
            return Err(ReadError::Malformed {
                inner,
                block: index,
            });
        }
        let stored = u32::from_le_bytes(b[PAYLOAD_END..].try_into().unwrap());
        if crate::crc::crc32(&b[..PAYLOAD_END]) != stored {
            return Err(ReadError::Checksum {
                inner,
                block: index,
            });
        }
        Ok(b)
    }

    /// The leaf block to start a search for keys `>= key` at: under each
    /// inner block, the last child whose first key is less than `key`, or
    /// the first child. (Not "at most": entries with one key can continue
    /// from one leaf block into the next, whose first key is then theirs.)
    fn descend(&self, key: &[u8]) -> Result<usize, ReadError> {
        let inner_count = self.src.block_count(true);
        if inner_count == 0 {
            return Ok(0);
        }
        let mut index = 0usize;
        // Each step goes one level down; a well-formed tree is far shallower than this.
        for _ in 0..32 {
            let b = self.block(true, index)?;
            let bad = ReadError::Malformed {
                inner: true,
                block: index,
            };
            let level = b[0];
            let count = u16::from_le_bytes([b[2], b[3]]) as usize;
            let mut p = INNER_HEADER;
            let mut prev: Vec<u8> = Vec::new();
            let mut chosen = None;
            for _ in 0..count {
                let ks = varint::get(&b, &mut p).ok_or(bad.clone())? as usize;
                let sl = varint::get(&b, &mut p).ok_or(bad.clone())? as usize;
                let suffix = b
                    .get(p..p.saturating_add(sl))
                    .filter(|_| ks <= prev.len())
                    .ok_or(bad.clone())?;
                p += sl;
                prev.truncate(ks);
                prev.extend_from_slice(suffix);
                let child = b.get(p..p + 4).ok_or(bad.clone())?;
                let child = u32::from_le_bytes(child.try_into().unwrap());
                p += INNER_CHILD;
                if prev.as_slice() < key || chosen.is_none() {
                    chosen = Some(child);
                } else {
                    break;
                }
            }
            let child = chosen.ok_or(bad.clone())? as usize;
            if level == 1 {
                return Ok(child);
            }
            if level == 0 || child >= inner_count {
                return Err(bad);
            }
            index = child;
        }
        Err(ReadError::Malformed {
            inner: true,
            block: index,
        })
    }

    /// Calls `f(key, hit)` for every entry of leaf block `leaf`, in order,
    /// until `f` returns `false`. Returns whether the scan should continue.
    fn scan_leaf(
        &self,
        leaf: usize,
        f: &mut impl FnMut(&[u8], Hit) -> bool,
    ) -> Result<bool, ReadError> {
        let b = self.block(false, leaf)?;
        let bad = || ReadError::Malformed {
            inner: false,
            block: leaf,
        };
        let count = u16::from_le_bytes([b[0], b[1]]) as usize;
        let used = u16::from_le_bytes([b[2], b[3]]) as usize;
        if LEAF_HEADER + used > PAYLOAD_END {
            return Err(bad());
        }
        let payload = &b[..LEAF_HEADER + used];
        let mut p = LEAF_HEADER;
        let (mut key, mut disp): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        let field = |prev: &mut Vec<u8>, p: &mut usize| -> Result<(), ReadError> {
            let shared = varint::get(payload, p).ok_or_else(bad)? as usize;
            let len = varint::get(payload, p).ok_or_else(bad)? as usize;
            let s = payload
                .get(*p..p.saturating_add(len))
                .filter(|_| shared <= prev.len())
                .ok_or_else(bad)?;
            *p += len;
            prev.truncate(shared);
            prev.extend_from_slice(s);
            Ok(())
        };
        for _ in 0..count {
            field(&mut key, &mut p)?;
            field(&mut disp, &mut p)?;
            let article = varint::get(payload, &mut p)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(bad)?;
            let flags = *payload.get(p).ok_or_else(bad)?;
            let popularity = *payload.get(p + 1).ok_or_else(bad)?;
            p += 2;
            let anchor = if flags & FLAG_ANCHOR != 0 {
                let b = payload.get(p..p + 4).ok_or_else(bad)?;
                p += 4;
                Some(u32::from_le_bytes(b.try_into().unwrap()))
            } else {
                None
            };
            let display = String::from_utf8(disp.clone()).map_err(|_| bad())?;
            if !f(
                &key,
                Hit {
                    display,
                    article,
                    flags,
                    popularity,
                    anchor,
                },
            ) {
                return Ok(false);
            }
        }
        if p != payload.len() {
            return Err(bad());
        }
        Ok(true)
    }

    /// Up to `limit` titles whose search key starts with `search_key(prefix)`.
    pub fn prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Hit>, ReadError> {
        self.prefix_key(search_key(prefix).as_bytes(), limit)
    }

    /// Up to `limit` titles whose key starts with `want`, a search key
    /// already folded, in index order.
    pub fn prefix_key(&self, want: &[u8], limit: usize) -> Result<Vec<Hit>, ReadError> {
        let mut hits = Vec::new();
        if limit == 0 {
            return Ok(hits);
        }
        let leaf_count = self.src.block_count(false);
        let mut leaf = self.descend(want)?;
        let mut done = false;
        while leaf < leaf_count && !done {
            let more = self.scan_leaf(leaf, &mut |key, hit| {
                if key < want {
                    return true;
                }
                if !key.starts_with(want) {
                    done = true;
                    return false;
                }
                hits.push(hit);
                if hits.len() == limit {
                    done = true;
                }
                !done
            })?;
            done |= !more;
            leaf += 1;
        }
        Ok(hits)
    }

    /// The children of inner block `index`: whether they are inner blocks,
    /// and per child its first key, block number and highest popularity.
    fn children(&self, index: usize) -> Result<Children, ReadError> {
        let b = self.block(true, index)?;
        let bad = ReadError::Malformed {
            inner: true,
            block: index,
        };
        let level = b[0];
        let count = u16::from_le_bytes([b[2], b[3]]) as usize;
        let mut p = INNER_HEADER;
        let mut prev: Vec<u8> = Vec::new();
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let ks = varint::get(&b, &mut p).ok_or(bad.clone())? as usize;
            let sl = varint::get(&b, &mut p).ok_or(bad.clone())? as usize;
            let suffix = b
                .get(p..p.saturating_add(sl))
                .filter(|_| ks <= prev.len())
                .ok_or(bad.clone())?;
            p += sl;
            prev.truncate(ks);
            prev.extend_from_slice(suffix);
            let child = b.get(p..p + INNER_CHILD).ok_or(bad.clone())?;
            let block = u32::from_le_bytes(child[..4].try_into().unwrap()) as usize;
            out.push((prev.clone(), block, child[4]));
            p += INNER_CHILD;
        }
        if level == 0 {
            return Err(bad);
        }
        Ok((level > 1, out))
    }

    /// The `limit` most popular articles with a title whose key starts with
    /// `want`, a search key already folded (spec 6.5): one entry for each,
    /// its first match in index order; most popular first, and of equal
    /// popularity the earlier in index order. A best-first walk: a subtree
    /// is opened only while its highest popularity could still make the
    /// list, so a short prefix costs a few blocks however many titles start
    /// with it.
    pub fn most_popular(&self, want: &[u8], limit: usize) -> Result<Vec<Hit>, ReadError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Keys starting with `want` sort from `want` up to (not including)
        // `want` with its last byte one higher; none of UTF-8's bytes is 0xFF.
        let upper: Option<Vec<u8>> = want.split_last().map(|(last, head)| {
            let mut u = head.to_vec();
            u.push(last + 1);
            u
        });
        // A child's entries have keys from its separator up to the next
        // child's separator, that one included (entries with one key can
        // continue into the next child).
        let overlaps = |lo: &[u8], hi: Option<&[u8]>| {
            upper.as_deref().is_none_or(|u| lo < u) && hi.is_none_or(|h| h >= want)
        };
        // Subtrees to open, highest popularity first, then lowest separator:
        // (popularity, separator, inner, block, next child's separator).
        type Open = (u8, Reverse<Option<Vec<u8>>>, bool, usize, Option<Vec<u8>>);
        let mut open: BinaryHeap<Open> = BinaryHeap::new();
        let has_inner = self.src.block_count(true) > 0;
        open.push((u8::MAX, Reverse(None), has_inner, 0, None));
        // An entry's place in index order: leaf block, entry in it.
        type At = (usize, usize);
        // The best so far, one entry per article, worst first: by
        // popularity, then index order.
        let mut best: BTreeSet<(u8, Reverse<At>)> = BTreeSet::new();
        let mut kept: HashMap<At, (Vec<u8>, Hit)> = HashMap::new();
        let mut of_article: HashMap<u32, At> = HashMap::new();
        // Each block is opened once; a damaged tree could lead back.
        let mut opened: HashSet<(bool, usize)> = HashSet::new();
        while let Some((top, Reverse(lo), inner, block, hi)) = open.pop() {
            if best.len() == limit {
                let &(low, Reverse(at)) = best.first().unwrap();
                // Nothing left can beat the worst kept: less popular, or as
                // popular but later in the index (its separator is past the
                // worst kept's key).
                let later = || lo.as_deref().is_some_and(|lo| lo > kept[&at].0.as_slice());
                if top < low || (top == low && later()) {
                    break;
                }
            }
            if !opened.insert((inner, block)) {
                return Err(ReadError::Malformed { inner, block });
            }
            if inner {
                let (to_inner, kids) = self.children(block)?;
                for (i, (first, child, pop)) in kids.iter().enumerate() {
                    let next = kids.get(i + 1).map(|(k, _, _)| k.clone()).or(hi.clone());
                    if overlaps(first, next.as_deref()) {
                        open.push((*pop, Reverse(Some(first.clone())), to_inner, *child, next));
                    }
                }
                continue;
            }
            let mut n = 0;
            self.scan_leaf(block, &mut |key, hit| {
                let at = (block, n);
                n += 1;
                if !key.starts_with(want) {
                    return upper.as_deref().is_none_or(|u| key < u);
                }
                // An article counts once, by its first entry in index order.
                if let Some(&other) = of_article.get(&hit.article) {
                    if other < at {
                        return true;
                    }
                    if let Some((_, old)) = kept.remove(&other) {
                        best.remove(&(old.popularity, Reverse(other)));
                    }
                }
                of_article.insert(hit.article, at);
                best.insert((hit.popularity, Reverse(at)));
                kept.insert(at, (key.to_vec(), hit));
                if best.len() > limit {
                    let (_, Reverse(gone)) = best.pop_first().unwrap();
                    let (_, hit) = kept.remove(&gone).unwrap();
                    of_article.remove(&hit.article);
                }
                true
            })?;
        }
        Ok(best
            .into_iter()
            .rev()
            .map(|(_, Reverse(at))| kept.remove(&at).unwrap().1)
            .collect())
    }

    /// What [`Self::check_inner`] needs to know of each leaf block.
    pub fn leaf_summaries(&self) -> Result<Vec<LeafSummary>, ReadError> {
        let mut leaves = vec![LeafSummary::default(); self.src.block_count(false)];
        self.for_each_in_leaf(|leaf, key, hit| leaves[leaf].add(key, hit.popularity))?;
        Ok(leaves)
    }

    /// Problems with the inner blocks (spec 6.4): every leaf block under
    /// them once and in order, each separator after the leaf block before
    /// and at most the first key of its own, each block's separator its
    /// first child's, and each highest popularity the highest below it.
    pub fn check_inner(&self, leaves: &[LeafSummary]) -> Result<Vec<String>, ReadError> {
        let mut problems = Vec::new();
        if self.src.block_count(true) == 0 {
            if leaves.len() > 1 {
                problems.push("title index: leaf blocks without inner blocks".to_string());
            }
            return Ok(problems);
        }
        let mut next_leaf = 0;
        self.check_subtree(0, leaves, &mut next_leaf, &mut problems, 0)?;
        if next_leaf != leaves.len() {
            problems.push(format!(
                "title index: the inner blocks lead to {next_leaf} of {} leaf blocks",
                leaves.len()
            ));
        }
        Ok(problems)
    }

    /// Checks the subtree under inner block `block`; returns its first
    /// separator and highest popularity.
    fn check_subtree(
        &self,
        block: usize,
        leaves: &[LeafSummary],
        next_leaf: &mut usize,
        problems: &mut Vec<String>,
        depth: usize,
    ) -> Result<(Vec<u8>, u8), ReadError> {
        let bad = ReadError::Malformed { inner: true, block };
        if depth > 32 {
            return Err(bad);
        }
        let (to_inner, kids) = self.children(block)?;
        let mut top = 0u8;
        let problem = |problems: &mut Vec<String>, what: String| {
            problems.push(format!("title index: inner block {block}: {what}"));
        };
        for (sep, child, stored) in &kids {
            let actual = if to_inner {
                let (first, top) =
                    self.check_subtree(*child, leaves, next_leaf, problems, depth + 1)?;
                if first != *sep {
                    problem(problems, "a separator is not its child's".to_string());
                }
                top
            } else {
                let leaf = leaves.get(*child).ok_or(bad.clone())?;
                if *child != *next_leaf {
                    problem(
                        problems,
                        format!("leads to leaf block {child}, not {next_leaf}"),
                    );
                }
                *next_leaf = child + 1;
                let after_previous = match child.checked_sub(1).and_then(|p| leaves.get(p)) {
                    None => true,
                    Some(prev) => *sep > prev.last || (*sep == prev.last && *sep == leaf.first),
                };
                if !after_previous || *sep > leaf.first {
                    problem(
                        problems,
                        format!("the separator of leaf block {child} is out of place"),
                    );
                }
                leaf.top
            };
            if actual != *stored {
                problem(
                    problems,
                    format!("gives popularity {stored} for a child whose highest is {actual}"),
                );
            }
            top = top.max(actual);
        }
        let first = kids.first().map(|k| k.0.clone()).ok_or(bad)?;
        Ok((first, top))
    }

    /// Calls `f(key, hit)` for every entry in index order.
    pub fn for_each(&self, mut f: impl FnMut(&[u8], Hit)) -> Result<(), ReadError> {
        self.for_each_in_leaf(|_, key, hit| f(key, hit))
    }

    /// Calls `f(leaf block, key, hit)` for every entry in index order.
    pub fn for_each_in_leaf(&self, mut f: impl FnMut(usize, &[u8], Hit)) -> Result<(), ReadError> {
        for leaf in 0..self.src.block_count(false) {
            self.scan_leaf(leaf, &mut |key, hit| {
                f(leaf, key, hit);
                true
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up popularity for an article.
    fn pop(article: u32) -> u8 {
        (article.wrapping_mul(2_654_435_761) >> 24) as u8
    }

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
                // Some redirects lead to a section.
                TitleEntry::new(t, i, i % 3 == 0)
                    .with_anchor((i % 15 == 0).then(|| i.wrapping_mul(2_654_435_761)))
                    .with_popularity(pop(i))
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
    fn full_scan_is_sorted_and_complete() {
        let idx = build(synthetic(20_000));
        let mut seen = Vec::new();
        idx.reader()
            .for_each(|key, hit| seen.push((key.to_vec(), hit.display.into_bytes())))
            .unwrap();
        assert_eq!(seen.len(), idx.entry_count);
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn detects_corruption() {
        let idx = build(synthetic(5_000));
        let mut leaves = idx.leaves.clone();
        leaves[100] ^= 0xFF;
        let r = IndexReader::new(Slices {
            leaves: &leaves,
            inner: &idx.inner,
        });
        assert!(matches!(
            r.prefix("", usize::MAX),
            Err(ReadError::Checksum { .. })
        ));
    }

    #[test]
    fn a_key_shared_across_leaf_blocks_is_found_in_full() {
        // One title for many articles: more entries with one key than a
        // leaf block holds, so they continue from block to block.
        let mut entries = synthetic(3_000);
        entries.extend((0..2_000).map(|i| TitleEntry::new("Mercury", 10_000 + i, false)));
        let idx = build(entries);
        let r = idx.reader();
        for q in ["Mercury", "mercur", "MERCURY"] {
            assert_eq!(r.prefix(q, usize::MAX).unwrap().len(), 2_000, "{q}");
        }
        assert_eq!(r.most_popular(b"mercury", 3000).unwrap().len(), 2_000);
    }

    /// Counts the blocks read through it.
    struct Counting<'a> {
        inner: Slices<'a>,
        reads: std::cell::Cell<usize>,
    }

    impl BlockSource for Counting<'_> {
        fn block_count(&self, inner: bool) -> usize {
            self.inner.block_count(inner)
        }

        fn read_block(&self, inner: bool, index: usize) -> Result<Cow<'_, [u8]>, ReadError> {
            self.reads.set(self.reads.get() + 1);
            self.inner.read_block(inner, index)
        }
    }

    #[test]
    fn most_popular_matches_brute_force_and_opens_few_blocks() {
        // Several titles for most articles, as redirects make them.
        let entries: Vec<TitleEntry> = synthetic(80_000)
            .into_iter()
            .map(|mut e| {
                e.article %= 24_000;
                e.popularity = pop(e.article);
                e
            })
            .collect();
        let idx = build(entries);
        assert!(idx.levels >= 3, "test should exercise inner levels");
        let r = IndexReader::new(Counting {
            inner: Slices {
                leaves: &idx.leaves,
                inner: &idx.inner,
            },
            reads: Default::default(),
        });
        assert!(
            r.check_inner(&r.leaf_summaries().unwrap())
                .unwrap()
                .is_empty()
        );
        let all = r.prefix("", usize::MAX).unwrap();
        for prefix in [
            "",
            "a",
            "al",
            "einstein1",
            "x5",
            "zola",
            "emile 1",
            "nothing",
        ] {
            let want = search_key(prefix);
            for k in [1, 5, 20] {
                r.src.reads.set(0);
                let got = r.most_popular(want.as_bytes(), k).unwrap();
                let reads = r.src.reads.get();
                let matches: Vec<Hit> = all
                    .iter()
                    .filter(|h| search_key(&h.display).starts_with(&want))
                    .cloned()
                    .collect();
                assert_eq!(got, top_articles(matches, k), "{prefix:?} top {k}");
                // A handful of blocks, of the hundreds that hold matches
                // for the shortest prefixes.
                assert!(reads <= 4 + 2 * k, "{prefix:?} top {k}: {reads} blocks");
            }
        }
        r.src.reads.set(0);
        r.most_popular(b"", 10).unwrap();
        eprintln!(
            "top 10 of {} titles in {} leaf and {} inner blocks: {} blocks read",
            all.len(),
            idx.leaf_blocks(),
            idx.inner_blocks(),
            r.src.reads.get()
        );
    }

    /// What [`IndexReader::most_popular`] should give for these matches, in
    /// index order: most popular first, ties in index order, each article
    /// once.
    fn top_articles(mut matches: Vec<Hit>, k: usize) -> Vec<Hit> {
        matches.sort_by_key(|h| std::cmp::Reverse(h.popularity));
        let mut seen = std::collections::HashSet::new();
        matches.retain(|h| seen.insert(h.article));
        matches.truncate(k);
        matches
    }

    #[test]
    fn a_wrong_popularity_in_an_inner_block_is_found() {
        let mut idx = build(synthetic(20_000));
        // Lower the first child's highest popularity in the root.
        let b = &mut idx.inner[..BLOCK_SIZE];
        let mut p = INNER_HEADER;
        let _ = varint::get(b, &mut p);
        let len = varint::get(b, &mut p).unwrap() as usize;
        p += len + 4;
        b[p] = b[p].saturating_sub(1);
        seal(b);
        let r = idx.reader();
        assert_eq!(
            r.check_inner(&r.leaf_summaries().unwrap()).unwrap().len(),
            1
        );
    }

    /// Builds through a [`TitleSorter`] and a [`LeafWriter`] instead.
    fn sorted_build(entries: &[TitleEntry], budget: usize) -> (Vec<u8>, Vec<u8>, usize, usize) {
        let mut sorter = TitleSorter::new(budget, None);
        for e in entries {
            sorter
                .push(
                    &e.display,
                    e.article,
                    e.flags & FLAG_REDIRECT != 0,
                    e.anchor,
                )
                .unwrap();
        }
        let mut leaves = LeafWriter::new(Vec::new());
        sorter
            .finish(|k, d, a, f, anchor| leaves.push(k, d, a, f, anchor, pop(a)))
            .unwrap();
        let done = leaves.finish().unwrap();
        assert_eq!(done.bytes as usize, done.out.len());
        assert_eq!(done.crc32, crate::crc::crc32(&done.out));
        let (inner, _) = build_inner(done.separators, done.leaf_popularity);
        (done.out, inner, done.entry_count, done.redirect_count)
    }

    #[test]
    fn sorter_matches_build_in_memory_and_in_runs() {
        let mut entries = synthetic(30_000);
        // Spellings of one title that differ in case or accents, for one article.
        for t in ["Godel code", "GODEL CODE", "Gödel code"] {
            entries.push(TitleEntry::new(t, 5, true).with_popularity(pop(5)));
        }
        let idx = build(entries.clone());
        let redirects = idx
            .reader()
            .prefix("", usize::MAX)
            .unwrap()
            .iter()
            .filter(|h| h.flags & FLAG_REDIRECT != 0)
            .count();
        // All in memory, then spread over many runs.
        for budget in [usize::MAX, 256 << 10, 48 << 10] {
            let (leaves, inner, count, redirect_count) = sorted_build(&entries, budget);
            assert_eq!(count, idx.entry_count, "budget {budget}");
            assert_eq!(redirect_count, redirects, "budget {budget}");
            assert!(
                leaves == idx.leaves && inner == idx.inner,
                "budget {budget}"
            );
        }
        let hits = idx.reader().prefix("godel code", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display, "Gödel code");
        assert_eq!(sorted_build(&[], 16 << 10).0, build(Vec::new()).leaves);
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
