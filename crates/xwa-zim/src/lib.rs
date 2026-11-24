//! Read-only access to Kiwix ZIM archives.
//!
//! Only what the converter needs: the header, the MIME list, directory
//! entries (by index, in bulk, or by path), clusters and their blobs, and the
//! title listing. Reads are positional, so one [`Archive`] can be shared by
//! worker threads.

mod error;
mod io;

use std::fs::File;
use std::path::Path;

pub use error::{Error, Result};

const MAGIC: u32 = 72_173_914;
const NO_POSITION: u64 = u64::MAX;

/// Fixed-size header at the start of every ZIM file.
#[derive(Debug, Clone)]
pub struct Header {
    pub major_version: u16,
    pub minor_version: u16,
    pub uuid: [u8; 16],
    pub entry_count: u32,
    pub cluster_count: u32,
    pub path_ptr_pos: u64,
    pub title_ptr_pos: u64,
    pub cluster_ptr_pos: u64,
    pub mime_list_pos: u64,
    pub main_page: Option<u32>,
    pub checksum_pos: u64,
}

/// Where a directory entry's content lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Redirect(u32),
    Blob { cluster: u32, blob: u32 },
}

/// One directory entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub index: u32,
    pub namespace: u8,
    /// Index into [`Archive::mime_types`]; `None` for redirects.
    pub mime: Option<u16>,
    pub path: String,
    /// Title as stored; empty means "same as path".
    pub title: String,
    pub target: Target,
}

impl Entry {
    /// Title to display: the stored title, or the path when none is stored.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.path
        } else {
            &self.title
        }
    }
}

/// Cluster compression as recorded in the cluster's info byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Xz,
    Zstd,
}

/// Cheap facts about a cluster, read without decompressing it.
#[derive(Debug, Clone, Copy)]
pub struct ClusterInfo {
    pub compression: Compression,
    pub extended: bool,
    pub stored_size: u64,
    /// zstd frame window size declared in the frame header, if any.
    pub zstd_window: Option<u64>,
}

/// A decompressed cluster.
pub struct Cluster {
    data: Vec<u8>,
    offsets: Vec<u64>,
}

impl Cluster {
    pub fn blob_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn blob(&self, index: usize) -> Option<&[u8]> {
        let start = *self.offsets.get(index)? as usize;
        let end = *self.offsets.get(index + 1)? as usize;
        self.data.get(start..end)
    }

    pub fn decompressed_len(&self) -> usize {
        self.data.len()
    }

    /// Byte offset of a blob's end within the decompressed cluster, i.e. how
    /// much has to be decompressed to reach all of it.
    pub fn blob_end(&self, index: usize) -> Option<u64> {
        self.offsets.get(index + 1).copied()
    }
}

/// An open ZIM archive.
pub struct Archive {
    file: File,
    header: Header,
    mime_types: Vec<String>,
    cluster_ptrs: Vec<u64>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let mut raw = [0u8; 80];
        io::read_exact_at(&file, &mut raw, 0)?;
        let u16_at = |o: usize| u16::from_le_bytes(raw[o..o + 2].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(raw[o..o + 8].try_into().unwrap());
        if u32_at(0) != MAGIC {
            return Err(Error::BadMagic);
        }
        let main_page = u32_at(64);
        let header = Header {
            major_version: u16_at(4),
            minor_version: u16_at(6),
            uuid: raw[8..24].try_into().unwrap(),
            entry_count: u32_at(24),
            cluster_count: u32_at(28),
            path_ptr_pos: u64_at(32),
            title_ptr_pos: u64_at(40),
            cluster_ptr_pos: u64_at(48),
            mime_list_pos: u64_at(56),
            main_page: (main_page != u32::MAX).then_some(main_page),
            checksum_pos: u64_at(72),
        };
        if header.major_version != 5 && header.major_version != 6 {
            return Err(Error::Unsupported(format!(
                "ZIM major version {}",
                header.major_version
            )));
        }
        let mime_types = read_mime_list(&file, header.mime_list_pos)?;
        let mut ptrs = vec![0u8; header.cluster_count as usize * 8];
        io::read_exact_at(&file, &mut ptrs, header.cluster_ptr_pos)?;
        let cluster_ptrs = ptrs
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        Ok(Self {
            file,
            header,
            mime_types,
            cluster_ptrs,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    pub fn entry_count(&self) -> u32 {
        self.header.entry_count
    }

    pub fn cluster_count(&self) -> u32 {
        self.header.cluster_count
    }

    pub fn mime_type(&self, entry: &Entry) -> Option<&str> {
        entry
            .mime
            .and_then(|m| self.mime_types.get(m as usize))
            .map(String::as_str)
    }

    fn dirent_offset(&self, index: u32) -> Result<u64> {
        if index >= self.header.entry_count {
            return Err(Error::Corrupt(format!("entry index {index} out of range")));
        }
        let mut b = [0u8; 8];
        io::read_exact_at(
            &self.file,
            &mut b,
            self.header.path_ptr_pos + index as u64 * 8,
        )?;
        Ok(u64::from_le_bytes(b))
    }

    /// Reads one directory entry by its index in path order.
    pub fn entry(&self, index: u32) -> Result<Entry> {
        let offset = self.dirent_offset(index)?;
        let mut buf = vec![0u8; 512];
        loop {
            let n = io::read_at_most(&self.file, &mut buf, offset)?;
            if let Some(entry) = parse_dirent(&buf[..n], index)? {
                return Ok(entry);
            }
            if n < buf.len() {
                return Err(Error::Corrupt(format!("truncated directory entry {index}")));
            }
            buf.resize(buf.len() * 4, 0);
        }
    }

    /// Visits every entry in path order, reading the directory in large
    /// sequential batches. Much faster than calling [`Archive::entry`] in a loop.
    pub fn for_each_entry(&self, mut f: impl FnMut(Entry) -> Result<()>) -> Result<()> {
        const PTR_BATCH: u32 = 65_536;
        const WINDOW: usize = 1 << 20;
        let mut window = Vec::with_capacity(WINDOW);
        let mut window_start = 0u64;
        let mut ptrs = vec![0u8; PTR_BATCH as usize * 8];
        let mut first = 0u32;
        while first < self.header.entry_count {
            let count = PTR_BATCH.min(self.header.entry_count - first);
            let bytes = &mut ptrs[..count as usize * 8];
            io::read_exact_at(
                &self.file,
                bytes,
                self.header.path_ptr_pos + first as u64 * 8,
            )?;
            for (i, chunk) in bytes.as_chunks::<8>().0.iter().enumerate() {
                let index = first + i as u32;
                let offset = u64::from_le_bytes(*chunk);
                let parsed =
                    if offset >= window_start && offset < window_start + window.len() as u64 {
                        parse_dirent(&window[(offset - window_start) as usize..], index)?
                    } else {
                        None
                    };
                let entry = match parsed {
                    Some(e) => e,
                    None => {
                        window.resize(WINDOW, 0);
                        let n = io::read_at_most(&self.file, &mut window, offset)?;
                        window.truncate(n);
                        window_start = offset;
                        match parse_dirent(&window, index)? {
                            Some(e) => e,
                            None => self.entry(index)?,
                        }
                    }
                };
                f(entry)?;
            }
            first += count;
        }
        Ok(())
    }

    /// Finds an entry by namespace and path (binary search in path order).
    pub fn find(&self, namespace: u8, path: &str) -> Result<Option<Entry>> {
        let (mut lo, mut hi) = (0u32, self.header.entry_count);
        let key = (namespace, path.as_bytes());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let e = self.entry(mid)?;
            match (e.namespace, e.path.as_bytes()).cmp(&key) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Ok(Some(e)),
            }
        }
        Ok(None)
    }

    /// Follows redirects (bounded) to the entry that holds content.
    pub fn resolve(&self, mut entry: Entry) -> Result<Entry> {
        for _ in 0..32 {
            match entry.target {
                Target::Blob { .. } => return Ok(entry),
                Target::Redirect(to) => entry = self.entry(to)?,
            }
        }
        Err(Error::Corrupt("redirect loop".into()))
    }

    fn cluster_span(&self, cluster: u32) -> Result<(u64, u64)> {
        let c = cluster as usize;
        let start = *self
            .cluster_ptrs
            .get(c)
            .ok_or_else(|| Error::Corrupt(format!("cluster {cluster} out of range")))?;
        let end = match self.cluster_ptrs.get(c + 1) {
            Some(&next) => next,
            None if self.header.checksum_pos != NO_POSITION => self.header.checksum_pos,
            None => self.file.metadata()?.len(),
        };
        if end <= start {
            return Err(Error::Corrupt(format!("cluster {cluster} has no data")));
        }
        Ok((start, end))
    }

    /// Where each cluster starts in the file, in cluster order.
    pub fn cluster_positions(&self) -> &[u64] {
        &self.cluster_ptrs
    }

    /// Reads a cluster's info byte and, for zstd, its frame header.
    pub fn cluster_info(&self, cluster: u32) -> Result<ClusterInfo> {
        let (start, end) = self.cluster_span(cluster)?;
        let mut head = [0u8; 16];
        let n = io::read_at_most(&self.file, &mut head, start)?;
        let compression = match head[0] & 0x0F {
            0 | 1 => Compression::None,
            4 => Compression::Xz,
            5 => Compression::Zstd,
            other => return Err(Error::Unsupported(format!("cluster compression {other}"))),
        };
        let zstd_window = (compression == Compression::Zstd && n >= 7)
            .then(|| zstd_window(&head[1..n]))
            .flatten();
        Ok(ClusterInfo {
            compression,
            extended: head[0] & 0x10 != 0,
            stored_size: end - start - 1,
            zstd_window,
        })
    }

    /// Reads and decompresses a whole cluster.
    pub fn cluster(&self, cluster: u32) -> Result<Cluster> {
        let (start, end) = self.cluster_span(cluster)?;
        let mut stored = vec![0u8; (end - start) as usize];
        io::read_exact_at(&self.file, &mut stored, start)?;
        let info = stored[0];
        let data = match info & 0x0F {
            0 | 1 => stored.split_off(1),
            5 => {
                let mut out = Vec::with_capacity(4 << 20);
                let mut dec = zstd::stream::Decoder::new(&stored[1..])?;
                std::io::copy(&mut dec, &mut out)?;
                out
            }
            4 => {
                let mut out = Vec::with_capacity(4 << 20);
                lzma_rs::xz_decompress(&mut &stored[1..], &mut out)
                    .map_err(|e| Error::Corrupt(format!("xz cluster {cluster}: {e}")))?;
                out
            }
            other => return Err(Error::Unsupported(format!("cluster compression {other}"))),
        };
        let width = if info & 0x10 != 0 { 8 } else { 4 };
        let read_off = |i: usize| -> Option<u64> {
            let b = data.get(i * width..i * width + width)?;
            Some(if width == 8 {
                u64::from_le_bytes(b.try_into().ok()?)
            } else {
                u32::from_le_bytes(b.try_into().ok()?) as u64
            })
        };
        let first =
            read_off(0).ok_or_else(|| Error::Corrupt(format!("cluster {cluster} too short")))?;
        let count = (first as usize) / width;
        let offsets: Vec<u64> = (0..count)
            .map(read_off)
            .collect::<Option<_>>()
            .ok_or_else(|| Error::Corrupt(format!("cluster {cluster} offset table truncated")))?;
        if offsets.windows(2).any(|w| w[1] < w[0])
            || offsets.last().is_some_and(|&l| l as usize > data.len())
        {
            return Err(Error::Corrupt(format!(
                "cluster {cluster} offsets out of order"
            )));
        }
        Ok(Cluster { data, offsets })
    }

    /// Returns the content of a non-redirect entry.
    pub fn content(&self, entry: &Entry) -> Result<Vec<u8>> {
        match entry.target {
            Target::Redirect(_) => Err(Error::Corrupt(format!(
                "entry {} is a redirect",
                entry.index
            ))),
            Target::Blob { cluster, blob } => self
                .cluster(cluster)?
                .blob(blob as usize)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| Error::Corrupt(format!("blob {blob} missing in cluster {cluster}"))),
        }
    }

    /// Entry indices of front articles in title order (`X/listing/titleOrdered/v1`).
    pub fn title_listing(&self) -> Result<Option<Vec<u32>>> {
        let Some(entry) = self.find(b'X', "listing/titleOrdered/v1")? else {
            return Ok(None);
        };
        let bytes = self.content(&entry)?;
        Ok(Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect(),
        ))
    }

    /// A metadata value (`M/<name>`), if present.
    pub fn metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.find(b'M', name)? {
            Some(e) => Ok(Some(self.content(&self.resolve(e)?)?)),
            None => Ok(None),
        }
    }

    /// Whether the ZIM has its articles' pictures: false for Kiwix's
    /// "nopic" and "mini" files, which say so in their tags
    /// (`_pictures:no`) or their flavour. True when they say nothing.
    pub fn has_pictures(&self) -> bool {
        let text = |name: &str| {
            self.metadata(name)
                .ok()
                .flatten()
                .map(|v| String::from_utf8_lossy(&v).to_lowercase())
                .unwrap_or_default()
        };
        let tagged_none = text("Tags").split(';').any(|t| t.trim() == "_pictures:no");
        let flavour = text("Flavour");
        !(tagged_none || matches!(flavour.trim(), "nopic" | "mini"))
    }
}

fn read_mime_list(file: &File, pos: u64) -> Result<Vec<String>> {
    let mut buf = vec![0u8; 4096];
    let n = io::read_at_most(file, &mut buf, pos)?;
    let mut list = Vec::new();
    let mut start = 0;
    for i in 0..n {
        if buf[i] == 0 {
            if i == start {
                return Ok(list);
            }
            list.push(String::from_utf8_lossy(&buf[start..i]).into_owned());
            start = i + 1;
        }
    }
    Err(Error::Corrupt(
        "MIME list not terminated within 4 KiB".into(),
    ))
}

/// Parses a directory entry from `buf`; `Ok(None)` means `buf` was too short.
fn parse_dirent(buf: &[u8], index: u32) -> Result<Option<Entry>> {
    if buf.len() < 12 {
        return Ok(None);
    }
    let mime = u16::from_le_bytes([buf[0], buf[1]]);
    let param_len = buf[2] as usize;
    let namespace = buf[3];
    let u32_at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let (mime, target, mut p) = if mime == 0xFFFF {
        (None, Target::Redirect(u32_at(8)), 12)
    } else if mime == 0xFFFE || mime == 0xFFFD {
        return Err(Error::Unsupported(format!(
            "linktarget/deleted entry {index}"
        )));
    } else {
        if buf.len() < 16 {
            return Ok(None);
        }
        (
            Some(mime),
            Target::Blob {
                cluster: u32_at(8),
                blob: u32_at(12),
            },
            16,
        )
    };
    let cstr = |p: &mut usize| -> Option<String> {
        let rel = buf.get(*p..)?.iter().position(|&b| b == 0)?;
        let s = String::from_utf8_lossy(&buf[*p..*p + rel]).into_owned();
        *p += rel + 1;
        Some(s)
    };
    let Some(path) = cstr(&mut p) else {
        return Ok(None);
    };
    let Some(title) = cstr(&mut p) else {
        return Ok(None);
    };
    if buf.len() < p + param_len {
        return Ok(None);
    }
    Ok(Some(Entry {
        index,
        namespace,
        mime,
        path,
        title,
        target,
    }))
}

/// Window size declared by a zstd frame header (`frame` starts at the magic).
fn zstd_window(frame: &[u8]) -> Option<u64> {
    if frame.len() < 6 || frame[..4] != [0x28, 0xB5, 0x2F, 0xFD] {
        return None;
    }
    let fhd = frame[4];
    let single_segment = fhd & 0x20 != 0;
    if !single_segment {
        let wd = frame[5];
        let base = 1u64 << (10 + (wd >> 3));
        return Some(base + (base / 8) * (wd & 7) as u64);
    }
    let did_len = [0usize, 1, 2, 4][(fhd & 3) as usize];
    let fcs_len = [1usize, 2, 4, 8][(fhd >> 6) as usize];
    let start = 5 + did_len;
    let bytes = frame.get(start..start + fcs_len)?;
    let mut v = 0u64;
    for (i, b) in bytes.iter().enumerate() {
        v |= (*b as u64) << (8 * i);
    }
    Some(if fcs_len == 2 { v + 256 } else { v })
}
