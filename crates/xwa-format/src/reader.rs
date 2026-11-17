//! Archive reader and verifier.
//!
//! Reads records on demand with positional reads, the way the firmware
//! does, so opening even a very large archive costs a few small reads.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crate::codec::{ChunkCodec, Decoder};
use crate::content::{self, Article, Place, Ref};
use crate::error::corrupt;
use crate::normalize::{self, search_key_with};
use crate::records::{ArticleRecord, ChunkEntry, ImageRecord, article_flags, image_encoding};
use crate::superblock::{SectionEntry, Superblock};
use crate::title_index::{self, BlockSource, Hit, IndexReader, ReadError};
use crate::{
    BLOCK_SIZE, DAYS_IN_YEAR, Error, NO_ARTICLE, Result, TextCodec, VERSION_MAJOR, VERSION_MINOR,
    la2, math_glyphs, meta, required, section, varint,
};

/// Random-access bytes: a file or a buffer.
pub trait Source: Send + Sync {
    fn size(&self) -> io::Result<u64>;
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

impl Source for File {
    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    #[cfg(unix)]
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, offset)
    }

    #[cfg(windows)]
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Source for Vec<u8> {
    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }

    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start =
            usize::try_from(offset).map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let src = start
            .checked_add(buf.len())
            .and_then(|end| self.get(start..end))
            .ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// Sections every archive must have.
const REQUIRED_SECTIONS: [(u32, &str); 8] = [
    (section::META, "META"),
    (section::ARTICLES, "ARTICLES"),
    (section::TITLES, "TITLES"),
    (section::TINDEX_LEAF, "TINDEX_LEAF"),
    (section::TINDEX_INNER, "TINDEX_INNER"),
    (section::CHUNK_TABLE, "CHUNK_TABLE"),
    (section::BLOBS, "BLOBS"),
    (section::FOLD, "FOLD"),
];

pub fn section_name(kind: u32) -> &'static str {
    match kind {
        section::META => "META",
        section::ARTICLES => "ARTICLES",
        section::TITLES => "TITLES",
        section::TINDEX_LEAF => "TINDEX_LEAF",
        section::TINDEX_INNER => "TINDEX_INNER",
        section::CHUNK_TABLE => "CHUNK_TABLE",
        section::BLOBS => "BLOBS",
        section::IMAGE_TABLE => "IMAGE_TABLE",
        section::MATH_GLYPHS => "MATH_GLYPHS",
        section::ZSTD_DICT => "ZSTD_DICT",
        section::FONT => "FONT",
        section::WORD_INDEX => "WORD_INDEX",
        section::FOLD => "FOLD",
        section::DAYS => "DAYS",
        _ => "unknown",
    }
}

/// An open archive.
pub struct Archive<S: Source = File> {
    src: S,
    superblock: Superblock,
    meta: Vec<(String, String)>,
    codec: ChunkCodec,
    decoder: Mutex<Decoder>,
    articles: SectionEntry,
    titles: SectionEntry,
    leaves: SectionEntry,
    inner: SectionEntry,
    chunk_table: SectionEntry,
    blobs: SectionEntry,
    image_table: Option<SectionEntry>,
    glyph_count: Option<u32>,
    /// The FOLD table (spec 6.1), for folding what is searched for.
    fold: Vec<(char, Box<str>)>,
    /// Title index blocks read so far.
    index_reads: std::sync::atomic::AtomicU64,
}

impl Archive<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_source(File::open(path)?)
    }
}

impl<S: Source> Archive<S> {
    /// Checks the superblock and loads META and the zstd dictionary.
    pub fn from_source(src: S) -> Result<Self> {
        let size = src.size()?;
        if size < Superblock::SIZE as u64 {
            return Err(Error::BadMagic);
        }
        let mut sb_bytes = vec![0u8; Superblock::SIZE];
        src.read_exact_at(&mut sb_bytes, 0)?;
        let sb = Superblock::decode(&sb_bytes)?;
        let draft_mismatch = VERSION_MAJOR == 0 && sb.version_minor != VERSION_MINOR;
        if sb.version_major != VERSION_MAJOR || draft_mismatch {
            let advice = if (sb.version_major, sb.version_minor) < (VERSION_MAJOR, VERSION_MINOR) {
                "it was made by an older xwa-tool; convert it again"
            } else {
                "it was made by a newer xwa-tool; update this one"
            };
            return Err(Error::Unsupported(format!(
                "format version {}.{} ({advice}); this reader supports {VERSION_MAJOR}.{VERSION_MINOR}",
                sb.version_major, sb.version_minor
            )));
        }
        let unknown = sb.required_features & !required::ALL_KNOWN;
        if unknown != 0 {
            return Err(Error::Unsupported(format!(
                "required features {unknown:#x}"
            )));
        }
        if sb.file_size != size {
            return Err(corrupt(format!(
                "file is {size} bytes but should be {} (incomplete copy?)",
                sb.file_size
            )));
        }
        if !(9..=15).contains(&sb.chunk_bits) {
            return Err(corrupt(format!(
                "chunk size 2^{} is not 2^9 to 2^15",
                sb.chunk_bits
            )));
        }
        if (sb.required_features & required::ZSTD_TEXT != 0) != (sb.text_codec == TextCodec::Zstd) {
            return Err(corrupt("zstd feature flag does not match the text codec"));
        }
        for (i, s) in sb.sections.iter().enumerate() {
            let end = s.offset.checked_add(s.length).filter(|&e| e <= size);
            if s.offset < Superblock::SIZE as u64
                || s.offset % BLOCK_SIZE as u64 != 0
                || end.is_none()
            {
                return Err(corrupt(format!(
                    "{} section out of bounds",
                    section_name(s.kind)
                )));
            }
            if sb.sections[..i].iter().any(|o| o.kind == s.kind) {
                return Err(corrupt(format!(
                    "duplicate {} section",
                    section_name(s.kind)
                )));
            }
        }
        for (kind, name) in REQUIRED_SECTIONS {
            if sb.section(kind).is_none() {
                return Err(corrupt(format!("missing {name} section")));
            }
        }
        let get = |kind| *sb.section(kind).unwrap();
        let (articles, chunk_table, leaves, inner) = (
            get(section::ARTICLES),
            get(section::CHUNK_TABLE),
            get(section::TINDEX_LEAF),
            get(section::TINDEX_INNER),
        );
        let image_table = sb.section(section::IMAGE_TABLE).copied();
        if let Some(days) = sb.section(section::DAYS)
            && days.length != DAYS_IN_YEAR as u64 * 4
        {
            return Err(corrupt(format!("DAYS section is {} bytes", days.length)));
        }
        let sized = [
            (articles, ArticleRecord::SIZE),
            (chunk_table, ChunkEntry::SIZE),
            (leaves, BLOCK_SIZE),
            (inner, BLOCK_SIZE),
        ];
        for (s, unit) in sized
            .into_iter()
            .chain(image_table.map(|s| (s, ImageRecord::SIZE)))
        {
            if s.length % unit as u64 != 0 {
                return Err(corrupt(format!(
                    "{} section length {} is not a multiple of {unit}",
                    section_name(s.kind),
                    s.length
                )));
            }
        }

        let small = |s: &SectionEntry, what: &str| -> Result<Vec<u8>> {
            if s.length > 64 << 20 {
                return Err(corrupt(format!("{what} section is {} bytes", s.length)));
            }
            let mut b = vec![0u8; s.length as usize];
            src.read_exact_at(&mut b, s.offset)?;
            if s.crc32 != 0 && crate::crc::crc32(&b) != s.crc32 {
                return Err(corrupt(format!("{what} section checksum mismatch")));
            }
            Ok(b)
        };
        let meta = meta::decode(&small(&get(section::META), "META")?)?;
        let fold = normalize::decode_fold_section(&small(&get(section::FOLD), "FOLD")?)
            .ok_or_else(|| corrupt("FOLD section is malformed"))?;
        let dict = sb
            .section(section::ZSTD_DICT)
            .map(|s| small(s, "ZSTD_DICT"))
            .transpose()?;
        if dict.is_some() && sb.text_codec != TextCodec::Zstd {
            return Err(corrupt("ZSTD_DICT present in a deflate archive"));
        }
        let glyph_count = match sb.section(section::MATH_GLYPHS) {
            Some(s) if s.length >= 4 => {
                let mut b = [0u8; 4];
                src.read_exact_at(&mut b, s.offset)?;
                Some(u16::from_le_bytes([b[2], b[3]]) as u32)
            }
            Some(_) => return Err(corrupt("MATH_GLYPHS section too short")),
            None => None,
        };
        let codec = ChunkCodec {
            codec: sb.text_codec,
            level: 0,
            dict,
        };
        let decoder = Mutex::new(codec.decoder()?);
        Ok(Self {
            src,
            meta,
            codec,
            decoder,
            articles,
            titles: get(section::TITLES),
            leaves,
            inner,
            chunk_table,
            blobs: get(section::BLOBS),
            image_table,
            glyph_count,
            fold,
            index_reads: Default::default(),
            superblock: sb,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }

    pub fn meta(&self) -> &[(String, String)] {
        &self.meta
    }

    pub fn meta_value(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn article_count(&self) -> u32 {
        (self.articles.length / ArticleRecord::SIZE as u64) as u32
    }

    pub fn chunk_count(&self) -> u32 {
        (self.chunk_table.length / ChunkEntry::SIZE as u64) as u32
    }

    pub fn image_count(&self) -> u32 {
        self.image_table
            .map_or(0, |s| (s.length / ImageRecord::SIZE as u64) as u32)
    }

    /// Glyphs in MATH_GLYPHS, if the archive has math.
    pub fn glyph_count(&self) -> Option<u32> {
        self.glyph_count
    }

    pub fn has_dictionary(&self) -> bool {
        self.codec.dict.is_some()
    }

    /// Uncompressed size of every chunk but the last (spec 7.1).
    pub fn chunk_size(&self) -> u32 {
        1 << self.superblock.chunk_bits
    }

    /// The article for each day of the year (spec 5.3), January 1 to
    /// December 31 with February 29, if the archive says.
    pub fn days(&self) -> Result<Option<Vec<Option<u32>>>> {
        let Some(s) = self.superblock.section(section::DAYS) else {
            return Ok(None);
        };
        let b = self.read_at(s.offset, DAYS_IN_YEAR * 4)?;
        Ok(Some(
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|d| u32::from_le_bytes(*d))
                .map(|id| (id != NO_ARTICLE).then_some(id))
                .collect(),
        ))
    }

    /// Title index blocks read since the archive was opened, as a measure
    /// of what searching costs a device.
    pub fn index_block_reads(&self) -> u64 {
        self.index_reads.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Entries in the FOLD table.
    pub fn fold_entries(&self) -> usize {
        self.fold.len()
    }

    /// The search key of `text`, folded through this archive's FOLD table
    /// as a reader on the device folds it (spec 6.1).
    pub fn search_key(&self, text: &str) -> String {
        search_key_with(text, &self.fold)
    }

    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut b = vec![0u8; len];
        self.src.read_exact_at(&mut b, offset)?;
        Ok(b)
    }

    fn record(&self, table: &SectionEntry, index: u64, size: usize, what: &str) -> Result<Vec<u8>> {
        if (index + 1) * size as u64 > table.length {
            return Err(Error::Invalid(format!("no {what} {index}")));
        }
        self.read_at(table.offset + index * size as u64, size)
    }

    pub fn article_record(&self, id: u32) -> Result<ArticleRecord> {
        ArticleRecord::decode(
            &self.record(&self.articles, id as u64, ArticleRecord::SIZE, "article")?,
            0,
        )
    }

    pub fn chunk_entry(&self, index: u32) -> Result<ChunkEntry> {
        ChunkEntry::decode(
            &self.record(&self.chunk_table, index as u64, ChunkEntry::SIZE, "chunk")?,
            0,
        )
    }

    pub fn image_record(&self, id: u32) -> Result<ImageRecord> {
        let table = self
            .image_table
            .ok_or_else(|| Error::Invalid(format!("no image {id}")))?;
        ImageRecord::decode(
            &self.record(&table, id as u64, ImageRecord::SIZE, "image")?,
            0,
        )
    }

    /// Display title of an article, from TITLES.
    pub fn title(&self, id: u32) -> Result<String> {
        let rec = self.article_record(id)?;
        let start = rec.title_ref as u64;
        let avail = self
            .titles
            .length
            .checked_sub(start)
            .ok_or_else(|| corrupt(format!("article {id}: title out of range")))?;
        let head = self.read_at(self.titles.offset + start, avail.min(10) as usize)?;
        let mut p = 0;
        let len = varint::get(&head, &mut p)
            .ok_or_else(|| corrupt(format!("article {id}: bad title length")))?;
        if len > avail - p as u64 {
            return Err(corrupt(format!("article {id}: title out of range")));
        }
        let bytes = self.read_at(self.titles.offset + start + p as u64, len as usize)?;
        String::from_utf8(bytes).map_err(|_| corrupt(format!("article {id}: title is not UTF-8")))
    }

    /// The stored bytes of a chunk, checked against its CRC-32.
    fn chunk_stored(&self, index: u32, entry: &ChunkEntry) -> Result<Vec<u8>> {
        let size = entry.stored_size as u64;
        if size < 4
            || entry
                .offset
                .checked_add(size)
                .is_none_or(|end| end > self.blobs.length)
        {
            return Err(corrupt(format!("chunk {index} out of bounds")));
        }
        // Every chunk but the last is full (spec 7.1).
        let full = self.chunk_size();
        let fits = if index + 1 < self.chunk_count() {
            entry.uncompressed_size == full
        } else {
            (1..=full).contains(&entry.uncompressed_size)
        };
        if !fits {
            return Err(corrupt(format!(
                "chunk {index} holds {} bytes; chunks are {full}",
                entry.uncompressed_size
            )));
        }
        let mut b = self.read_at(self.blobs.offset + entry.offset, size as usize)?;
        let crc = u32::from_le_bytes(b[b.len() - 4..].try_into().unwrap());
        b.truncate(b.len() - 4);
        if crate::crc::crc32(&b) != crc {
            return Err(corrupt(format!("chunk {index} checksum mismatch")));
        }
        Ok(b)
    }

    /// Decompresses a content chunk.
    pub fn read_chunk(&self, index: u32) -> Result<Vec<u8>> {
        let entry = self.chunk_entry(index)?;
        let stored = self.chunk_stored(index, &entry)?;
        let mut d = self.decoder.lock().unwrap_or_else(|e| e.into_inner());
        d.decompress(&stored, entry.uncompressed_size as usize)
            .map_err(|e| corrupt(format!("chunk {index}: {e}")))
    }

    fn cached_chunk<'c>(&self, index: u32, cache: &'c mut ChunkCache) -> Result<&'c [u8]> {
        if cache.index != Some(index) {
            cache.data = self.read_chunk(index)?;
            cache.index = Some(index);
            cache.reads += 1;
        }
        Ok(&cache.data)
    }

    fn stream_with(&self, id: u32, cache: &mut ChunkCache) -> Result<Vec<u8>> {
        let rec = self.article_record(id)?;
        let bad = |msg: &str| corrupt(format!("article {id}: {msg}"));
        let first = self.cached_chunk(rec.first_chunk, cache)?;
        let off = rec.offset_in_chunk as usize;
        if off >= first.len() {
            return Err(bad("stream starts past the end of its chunk"));
        }
        let mut buf = first[off..].to_vec();
        let mut chunk = rec.first_chunk;
        let mut used = 1u32;
        let chunk_count = self.chunk_count();
        let mut next = |buf: &mut Vec<u8>, chunk: &mut u32, used: &mut u32| -> Result<()> {
            *chunk += 1;
            if *chunk >= chunk_count {
                return Err(bad("stream runs past the last chunk"));
            }
            buf.extend_from_slice(self.cached_chunk(*chunk, cache)?);
            *used += 1;
            Ok(())
        };
        let (size, prefix) = loop {
            let mut p = 0;
            match varint::get(&buf, &mut p) {
                Some(size) => break (size, p),
                None if buf.len() < 10 => next(&mut buf, &mut chunk, &mut used)?,
                None => return Err(bad("bad stream size")),
            }
        };
        let remaining_chunks = (chunk_count - rec.first_chunk) as u64;
        if size > remaining_chunks * self.chunk_size() as u64 {
            return Err(bad("stream size larger than the remaining chunks"));
        }
        let total = prefix + size as usize;
        while buf.len() < total {
            next(&mut buf, &mut chunk, &mut used)?;
        }
        if used != rec.chunk_count as u32 {
            return Err(bad(&format!(
                "stream spans {used} chunks, record says {}",
                rec.chunk_count
            )));
        }
        buf.truncate(total);
        buf.drain(..prefix);
        Ok(buf)
    }

    /// The article stream payload (spec 7.2) without its size prefix.
    pub fn article_stream(&self, id: u32) -> Result<Vec<u8>> {
        self.stream_with(id, &mut ChunkCache::default())
    }

    pub fn article(&self, id: u32) -> Result<Article> {
        content::decode_article(&self.article_stream(id)?)
            .map_err(|e| corrupt(format!("article {id}: {e}")))
    }

    /// The section of article `id` a named place is in (spec 7.2), if the
    /// article has that place.
    pub fn find_place(&self, id: u32, anchor: u32) -> Result<Option<u32>> {
        let stream = self.article_stream(id)?;
        let (header, _) =
            content::decode_header(&stream).map_err(|e| corrupt(format!("article {id}: {e}")))?;
        Ok(content::find_anchor(&header, anchor))
    }

    /// An image's record and encoded bytes.
    pub fn image(&self, id: u32) -> Result<(ImageRecord, Vec<u8>)> {
        let rec = self.image_record(id)?;
        if rec
            .offset
            .checked_add(rec.size as u64)
            .is_none_or(|end| end > self.blobs.length)
        {
            return Err(corrupt(format!("image {id} out of bounds")));
        }
        let data = self.read_at(self.blobs.offset + rec.offset, rec.size as usize)?;
        Ok((rec, data))
    }

    fn index(&self) -> IndexReader<FileBlocks<'_, S>> {
        IndexReader::new(FileBlocks { archive: self })
    }

    /// Up to `limit` titles starting with `prefix`, after folding, in index
    /// order.
    pub fn search(&self, prefix: &str, limit: usize) -> Result<Vec<Hit>> {
        self.index()
            .prefix_key(self.search_key(prefix).as_bytes(), limit)
            .map_err(index_error)
    }

    /// The `limit` most popular titles starting with `prefix`, after
    /// folding: most popular first, ties in index order (spec 6.5).
    pub fn search_popular(&self, prefix: &str, limit: usize) -> Result<Vec<Hit>> {
        self.index()
            .most_popular(self.search_key(prefix).as_bytes(), limit)
            .map_err(index_error)
    }

    /// Checks everything: checksums, every chunk, article, link, image and
    /// index entry. I/O errors stop the check; problems are collected.
    pub fn verify(&self, progress: &mut dyn FnMut(&str, u64, u64)) -> Result<VerifyReport> {
        let mut r = VerifyReport::default();
        self.verify_sections(&mut r, progress)?;
        let popularity = self.verify_articles(&mut r, progress)?;
        self.verify_images(&mut r, progress)?;
        self.verify_index(&mut r, &popularity, progress)?;
        self.verify_days(&mut r)?;
        self.verify_counts(&mut r);
        Ok(r)
    }

    /// The MATH_GLYPHS table, if the archive has one.
    pub fn math_glyphs(&self) -> Result<Option<math_glyphs::GlyphTable>> {
        let Some(s) = self.superblock.section(section::MATH_GLYPHS) else {
            return Ok(None);
        };
        let len = usize::try_from(s.length).map_err(|_| corrupt("MATH_GLYPHS too large"))?;
        let bytes = self.read_at(s.offset, len)?;
        Ok(Some(math_glyphs::decode(&bytes)?))
    }

    fn verify_sections(
        &self,
        r: &mut VerifyReport,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<()> {
        match self.math_glyphs() {
            Ok(Some(t)) if t.upem == 0 || t.scales.is_empty() => {
                r.problem("MATH_GLYPHS has no size table or em size")
            }
            Ok(_) => {}
            Err(Error::Io(e)) => return Err(Error::Io(e)),
            Err(e) => r.problem(describe(&e)),
        }
        let mut sections: Vec<&SectionEntry> = self.superblock.sections.iter().collect();
        sections.sort_by_key(|s| s.offset);
        for w in sections.windows(2) {
            if w[0].offset + w[0].length > w[1].offset {
                r.problem(format!(
                    "{} and {} sections overlap",
                    section_name(w[0].kind),
                    section_name(w[1].kind)
                ));
            }
        }
        let total: u64 = sections
            .iter()
            .filter(|s| s.crc32 != 0)
            .map(|s| s.length)
            .sum();
        let mut done = 0;
        let mut buf = vec![0u8; 1 << 20];
        for s in sections {
            if s.crc32 == 0 {
                continue;
            }
            let mut crc = 0u32;
            let mut pos = 0;
            while pos < s.length {
                let n = (s.length - pos).min(buf.len() as u64) as usize;
                self.src.read_exact_at(&mut buf[..n], s.offset + pos)?;
                crc = crate::crc::crc32_append(crc, &buf[..n]);
                pos += n as u64;
                done += n as u64;
                progress("checksums", done, total);
            }
            if crc != s.crc32 {
                r.problem(format!(
                    "{} section checksum mismatch",
                    section_name(s.kind)
                ));
            }
        }
        Ok(())
    }

    /// Checks every article; returns each one's popularity, `None` where
    /// its record could not be read.
    fn verify_articles(
        &self,
        r: &mut VerifyReport,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<Vec<Option<u8>>> {
        let (count, images, chunks) =
            (self.article_count(), self.image_count(), self.chunk_count());
        let mut touched = vec![false; chunks as usize];
        let mut cache = ChunkCache::default();
        // Every article's named places (spec 7.2), as anchor hashes, to
        // check links to them once all are known.
        let mut places: Vec<u32> = Vec::new();
        let mut places_at: Vec<u64> = Vec::with_capacity(count as usize + 1);
        let mut anchor_links: Vec<(u32, u32)> = Vec::new();
        let mut popularity = vec![None; count as usize];
        for id in 0..count {
            places_at.push(places.len() as u64);
            if id % 256 == 0 {
                progress("articles", id as u64, count as u64);
            }
            let rec = match self.article_record(id) {
                Ok(rec) => rec,
                Err(e) => {
                    r.problem(describe(&e));
                    continue;
                }
            };
            popularity[id as usize] = Some(rec.popularity);
            // The record claims these chunks even if they turn out to be damaged.
            let end = (rec.first_chunk as usize + rec.chunk_count as usize).min(touched.len());
            if let Some(span) = touched.get_mut(rec.first_chunk as usize..end) {
                span.iter_mut().for_each(|t| *t = true);
            }
            let stream = match self.stream_with(id, &mut cache) {
                Ok(s) => s,
                Err(Error::Io(e)) => return Err(Error::Io(e)),
                Err(e) => {
                    r.problem(describe(&e));
                    continue;
                }
            };
            r.text_bytes += stream.len() as u64;
            let article = match content::decode_article(&stream) {
                Ok(a) => a,
                Err(e) => {
                    r.problem(format!("article {id}: {e}"));
                    continue;
                }
            };
            for p in content::text_problems(&article).into_iter().take(3) {
                r.problem(format!("article {id}: {p}"));
            }
            let (header, body_start) = content::decode_header(&stream)?;
            for e in &header.toc {
                if stream.get(body_start + e.pos as usize) != Some(&0x02) {
                    r.problem(format!(
                        "article {id}: contents entry {:?} does not point at a heading",
                        e.text
                    ));
                }
            }
            let sections = header.toc.len() as u32;
            for a in &header.anchors {
                if a.section > sections {
                    r.problem(format!(
                        "article {id}: a named place is in section {}, of {sections}",
                        a.section
                    ));
                }
            }
            places.extend(header.anchors.iter().map(|a| a.hash));
            r.listed_places += header.anchors.len() as u64;
            let mut seen = std::collections::HashSet::new();
            for e in &header.toc {
                if !seen.insert(content::heading_anchor_hash(&e.text)) {
                    r.repeated_headings += 1;
                }
            }
            places.extend(
                header
                    .toc
                    .iter()
                    .map(|e| content::heading_anchor_hash(&e.text)),
            );
            match self.title(id) {
                Ok(t) if t == article.title => {}
                Ok(t) => r.problem(format!(
                    "article {id}: TITLES says {t:?}, stream says {:?}",
                    article.title
                )),
                Err(e) => r.problem(describe(&e)),
            }
            let mut flags = 0u16;
            if article.infobox.is_some() {
                flags |= article_flags::INFOBOX;
            }
            if !article.tables.is_empty() {
                flags |= article_flags::TABLES;
            }
            let mut problems = Vec::new();
            content::walk_refs(&article, &mut |x| match x {
                Ref::Link(target, place) => {
                    r.links += 1;
                    if target >= count {
                        problems.push(format!("links to missing article {target}"));
                    }
                    match place {
                        Place::Anchor(hash) => anchor_links.push((target, hash)),
                        Place::Section(n) if target == id && n > sections => {
                            problems.push(format!("links to its section {n}, of {sections}"))
                        }
                        Place::Section(_) => {}
                    }
                }
                Ref::Image(img) => {
                    flags |= article_flags::IMAGES;
                    if img >= images {
                        problems.push(format!("uses missing image {img}"));
                    }
                }
                Ref::Formula(f) => {
                    flags |= article_flags::MATH;
                    r.formulas += 1;
                    let glyphs = f.items.iter().filter_map(|i| match i {
                        content::MathItem::Glyph { glyph, .. }
                        | content::MathItem::GlyphXY { glyph, .. } => Some(*glyph),
                        content::MathItem::Rule { .. } | content::MathItem::Path { .. } => None,
                    });
                    match self.glyph_count {
                        Some(n) => {
                            if let Some(g) = glyphs.into_iter().find(|&g| g >= n) {
                                problems.push(format!("formula uses missing glyph {g}"));
                            }
                        }
                        None => {
                            problems.push("formula in an archive without MATH_GLYPHS".to_string())
                        }
                    }
                }
                Ref::Table(t) => {
                    if t as usize >= article.tables.len() {
                        problems.push(format!("refers to missing table {t}"));
                    }
                }
                Ref::Infobox => {
                    if article.infobox.is_none() {
                        problems.push("refers to a missing infobox".to_string());
                    }
                }
            });
            const CONTENT_FLAGS: u16 = article_flags::INFOBOX
                | article_flags::TABLES
                | article_flags::MATH
                | article_flags::IMAGES;
            if rec.flags & CONTENT_FLAGS != flags {
                problems.push(format!(
                    "flags {:#x} do not match its content ({flags:#x})",
                    rec.flags & CONTENT_FLAGS
                ));
            }
            problems.dedup();
            for p in problems {
                r.problem(format!("article {id}: {p}"));
            }
        }
        progress("articles", count as u64, count as u64);
        places_at.push(places.len() as u64);
        r.anchor_links = anchor_links.len() as u64;
        r.anchor_links_found = anchor_links
            .iter()
            .filter(|(target, hash)| {
                let t = *target as usize;
                t + 1 < places_at.len()
                    && places[places_at[t] as usize..places_at[t + 1] as usize].contains(hash)
            })
            .count() as u64;
        r.articles = count;
        r.chunks = chunks;
        r.chunk_reads = cache.reads;
        for (i, _) in touched.iter().enumerate().filter(|(_, t)| !**t) {
            match self.read_chunk(i as u32) {
                Ok(_) => r.problem(format!("chunk {i} is not used by any article")),
                Err(Error::Io(e)) => return Err(Error::Io(e)),
                Err(e) => r.problem(format!("unused chunk {i}: {}", describe(&e))),
            }
        }
        if r.formulas > 0 && self.superblock.required_features & required::MATH == 0 {
            r.problem("archive has formulas but does not declare the math feature");
        }
        Ok(popularity)
    }

    fn verify_images(
        &self,
        r: &mut VerifyReport,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<()> {
        let count = self.image_count();
        let features = self.superblock.required_features;
        for id in 0..count {
            if id % 256 == 0 {
                progress("images", id as u64, count as u64);
            }
            let (rec, data) = match self.image(id) {
                Ok(x) => x,
                Err(Error::Io(e)) => return Err(Error::Io(e)),
                Err(e) => {
                    r.problem(describe(&e));
                    continue;
                }
            };
            r.image_bytes += data.len() as u64;
            let check = match rec.encoding {
                image_encoding::JPEG if features & required::JPEG == 0 => {
                    Err("JPEG feature not declared".to_string())
                }
                image_encoding::JPEG => check_jpeg(&data),
                image_encoding::LINE_ART if features & required::LINE_ART == 0 => {
                    Err("line-art feature not declared".to_string())
                }
                image_encoding::LINE_ART => check_la2(&data, &rec),
                other => Err(format!("unknown encoding {other}")),
            };
            if let Err(e) = check {
                r.problem(format!("image {id}: {e}"));
            }
        }
        progress("images", count as u64, count as u64);
        r.images = count;
        Ok(())
    }

    fn verify_index(
        &self,
        r: &mut VerifyReport,
        popularity: &[Option<u8>],
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<()> {
        let count = self.article_count();
        let mut titled = vec![false; count as usize];
        let mut prev: Option<(Vec<u8>, String)> = None;
        let mut sample = Vec::new();
        let mut problems = Vec::new();
        let leaf_blocks = self.leaves.length / BLOCK_SIZE as u64;
        let mut leaves = vec![title_index::LeafSummary::default(); leaf_blocks as usize];
        let index = self.index();
        let mut n = 0u64;
        let scan = index.for_each_in_leaf(|leaf, key, hit| {
            n += 1;
            if n.is_multiple_of(4096) {
                progress("title index", n, n.max(leaf_blocks * 100));
            }
            let folded = self.search_key(&hit.display);
            if folded.as_bytes() != key {
                problems.push(format!(
                    "title index: key of {:?} is not its folded title",
                    hit.display
                ));
            } else if self.search_key(&folded) != folded {
                // Typing a key must find it.
                problems.push(format!(
                    "title index: key of {:?} does not fold to itself",
                    hit.display
                ));
            }
            if let Some((pk, pd)) = &prev
                && (pk.as_slice(), pd.as_str()) > (key, hit.display.as_str())
            {
                problems.push(format!("title index: {:?} is out of order", hit.display));
            }
            leaves[leaf].add(key, hit.popularity);
            if hit.article >= count {
                problems.push(format!(
                    "title index: {:?} points to missing article {}",
                    hit.display, hit.article
                ));
            } else {
                if hit.flags & title_index::FLAG_REDIRECT == 0 {
                    titled[hit.article as usize] = true;
                }
                if let Some(p) = popularity[hit.article as usize]
                    && p != hit.popularity
                {
                    problems.push(format!(
                        "title index: {:?} has popularity {}, its article {p}",
                        hit.display, hit.popularity
                    ));
                }
            }
            if hit.flags & !(title_index::FLAG_REDIRECT | title_index::FLAG_ANCHOR) != 0 {
                problems.push(format!(
                    "title index: {:?} has reserved flags set",
                    hit.display
                ));
            }
            if hit.flags & title_index::FLAG_REDIRECT != 0 {
                r.redirects += 1;
            }
            if n % 997 == 1 {
                sample.push((hit.display.clone(), hit.article));
            }
            prev = Some((key.to_vec(), hit.display));
        });
        if let Err(e) = scan {
            match index_error(e) {
                Error::Io(e) => return Err(Error::Io(e)),
                e => r.problem(describe(&e)),
            }
            return Ok(());
        }
        r.title_entries = n;
        for p in problems {
            r.problem(p);
        }
        match index.check_inner(&leaves) {
            Ok(found) => found.into_iter().for_each(|p| r.problem(p)),
            Err(e) => r.problem(describe(&index_error(e))),
        }
        for id in 0..count {
            if !titled[id as usize] {
                match self.title(id) {
                    Ok(t) if t.is_empty() => {}
                    Ok(t) => {
                        r.problem(format!("article {id} ({t:?}) cannot be found by its title"))
                    }
                    Err(e) => r.problem(describe(&e)),
                }
            }
        }
        // Exercise the inner levels with lookups spread over the index, and
        // the popularity search where every match is known.
        const LIMIT: usize = 1000;
        for (display, article) in sample {
            let key = self.search_key(&display);
            match index.prefix_key(key.as_bytes(), LIMIT) {
                Ok(hits) if hits.len() < LIMIT && !hits.iter().any(|h| h.article == article) => r
                    .problem(format!(
                        "title index: searching {display:?} does not find it"
                    )),
                Ok(mut hits) if hits.len() < LIMIT => {
                    // Every match is known: the most popular are these, each
                    // article once.
                    hits.sort_by_key(|h| std::cmp::Reverse(h.popularity));
                    let mut seen = std::collections::HashSet::new();
                    hits.retain(|h| seen.insert(h.article));
                    match index.most_popular(key.as_bytes(), hits.len()) {
                        Ok(top) if top == hits => {}
                        Ok(_) => r.problem(format!(
                            "title index: the most popular titles starting with {display:?} are wrong"
                        )),
                        Err(e) => r.problem(describe(&index_error(e))),
                    }
                }
                Ok(_) => {}
                Err(e) => r.problem(describe(&index_error(e))),
            }
        }
        progress("title index", n, n);
        Ok(())
    }

    fn verify_days(&self, r: &mut VerifyReport) -> Result<()> {
        let count = self.article_count();
        for (day, id) in self.days()?.unwrap_or_default().into_iter().enumerate() {
            if let Some(id) = id
                && id >= count
            {
                r.problem(format!("DAYS: day {} names missing article {id}", day + 1));
            }
        }
        Ok(())
    }

    fn verify_counts(&self, r: &mut VerifyReport) {
        let expected = [
            ("counts.articles", r.articles as u64),
            ("counts.images", r.images as u64),
            ("counts.formulas", r.formulas),
            ("counts.redirects", r.redirects),
        ];
        for (key, actual) in expected {
            match self.meta_value(key).map(str::parse::<u64>) {
                Some(Ok(v)) if v == actual => {}
                Some(_) => r.problem(format!(
                    "META {key} = {:?}, archive has {actual}",
                    self.meta_value(key).unwrap()
                )),
                None => r.problem(format!("META has no {key}")),
            }
        }
    }
}

#[derive(Default)]
struct ChunkCache {
    index: Option<u32>,
    data: Vec<u8>,
    reads: u64,
}

struct FileBlocks<'a, S: Source> {
    archive: &'a Archive<S>,
}

impl<S: Source> BlockSource for FileBlocks<'_, S> {
    fn block_count(&self, inner: bool) -> usize {
        let s = if inner {
            &self.archive.inner
        } else {
            &self.archive.leaves
        };
        (s.length / BLOCK_SIZE as u64) as usize
    }

    fn read_block(
        &self,
        inner: bool,
        index: usize,
    ) -> std::result::Result<Cow<'_, [u8]>, ReadError> {
        if index >= self.block_count(inner) {
            return Err(ReadError::Malformed {
                inner,
                block: index,
            });
        }
        let s = if inner {
            &self.archive.inner
        } else {
            &self.archive.leaves
        };
        let mut b = vec![0u8; BLOCK_SIZE];
        self.archive
            .src
            .read_exact_at(&mut b, s.offset + (index * BLOCK_SIZE) as u64)
            .map_err(|e| ReadError::Io(e.to_string()))?;
        self.archive
            .index_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Cow::Owned(b))
    }
}

/// An error as a problem line: the message without the error-kind prefix.
fn describe(e: &Error) -> String {
    match e {
        Error::Corrupt(m) | Error::Unsupported(m) | Error::Invalid(m) => m.clone(),
        other => other.to_string(),
    }
}

fn index_error(e: ReadError) -> Error {
    match e {
        ReadError::Io(msg) => Error::Io(io::Error::other(msg)),
        other => corrupt(other.to_string()),
    }
}

fn check_jpeg(data: &[u8]) -> std::result::Result<(), String> {
    if !data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err("not a JPEG".into());
    }
    if !data.ends_with(&[0xFF, 0xD9]) {
        return Err("JPEG is truncated".into());
    }
    Ok(())
}

fn check_la2(data: &[u8], rec: &ImageRecord) -> std::result::Result<(), String> {
    let img = la2::decode(data).map_err(|e| describe(&e))?;
    let h = img.header;
    if (h.width, h.height) != (rec.width, rec.height) {
        return Err(format!(
            "LA2 is {}x{}, record says {}x{}",
            h.width, h.height, rec.width, rec.height
        ));
    }
    Ok(())
}

/// What [`Archive::verify`] found.
#[derive(Debug, Default, Clone)]
pub struct VerifyReport {
    pub articles: u32,
    pub chunks: u32,
    /// Chunks decompressed while reading every article in order.
    pub chunk_reads: u64,
    pub images: u32,
    pub title_entries: u64,
    pub redirects: u64,
    pub formulas: u64,
    pub links: u64,
    /// Links to named places in articles (spec 7.2), and how many of those
    /// places exist; the others open their article at the top.
    pub anchor_links: u64,
    pub anchor_links_found: u64,
    /// Named places the articles list (spec 7.2), and of those, the ones
    /// for headings whose text an earlier heading has.
    pub listed_places: u64,
    pub repeated_headings: u64,
    pub text_bytes: u64,
    pub image_bytes: u64,
    /// The first [`VerifyReport::MAX_LISTED`] problems.
    pub problems: Vec<String>,
    pub problem_count: u64,
}

impl VerifyReport {
    pub const MAX_LISTED: usize = 200;

    fn problem(&mut self, msg: impl Into<String>) {
        self.problem_count += 1;
        if self.problems.len() < Self::MAX_LISTED {
            self.problems.push(msg.into());
        }
    }

    pub fn ok(&self) -> bool {
        self.problem_count == 0
    }
}
