//! Streaming archive writer.
//!
//! Content chunks and images go into BLOBS as they arrive, the article
//! records and titles list go to temporary files, and the title index is
//! sorted in temporary files in batches of bounded size. Memory use is about
//! a byte per article and 16 per image, plus a fixed allowance (64 MB by
//! default for sorting titles), whatever the archive's size.
//! Chunks are compressed on worker threads; the file layout depends only on
//! the order of calls, so the same input always produces the same bytes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::codec::{ChunkCodec, MAX_CHUNK, train_dictionary};
use crate::content::{self, Article, Ref};
use crate::font::FontStyle;
use crate::records::{
    ArticleRecord, ChunkEntry, ImageRecord, U24_MAX, U48_MAX, article_flags, image_encoding,
};
use crate::superblock::{SectionEntry, Superblock};
use crate::title_index::{self, LeafWriter, Leaves, TitleSorter};
use crate::{
    BLOCK_SIZE, DAYS_IN_YEAR, Error, NO_ARTICLE, Profile, Result, TextCodec, VERSION_MAJOR,
    VERSION_MINOR, meta, normalize, optional, required, section, varint,
};

/// BLOBS always starts right after the superblock.
const BLOBS_START: u64 = Superblock::SIZE as u64;

/// Image flag bits (spec 9.1).
pub mod image_flags {
    pub const HD_SOURCE: u8 = 1 << 0;
    pub const TRANSPARENCY: u8 = 1 << 1;
}

#[derive(Debug, Clone)]
pub struct WriterOptions {
    pub profile: Profile,
    /// Uncompressed chunk size: a power of two, at most [`MAX_CHUNK`].
    pub chunk_size: usize,
    /// libdeflate level for the compact profile (1-12).
    pub deflate_level: i32,
    /// zstd level for the full profile (1-22).
    pub zstd_level: i32,
    /// A dictionary trained in advance. Without one, the writer trains one
    /// on the first `dict_sample_bytes` of content (measured: as good as
    /// sampling the whole archive, within 0.2%).
    pub zstd_dict: Option<Vec<u8>>,
    pub dict_sample_bytes: usize,
    pub dict_size: usize,
    /// Compression threads; 0 means one per CPU.
    pub threads: usize,
    /// Memory for title index entries; more go to temporary files.
    pub title_memory: usize,
    /// Folder for temporary files; `None` means the system's.
    pub temp_dir: Option<PathBuf>,
}

impl WriterOptions {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            chunk_size: MAX_CHUNK,
            deflate_level: 12,
            zstd_level: 19,
            zstd_dict: None,
            dict_sample_bytes: 16 << 20,
            dict_size: 112_640,
            threads: 0,
            title_memory: 64 << 20,
            temp_dir: None,
        }
    }
}

/// What an image is, apart from its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub encoding: u8,
    pub width: u16,
    pub height: u16,
    pub kind: u8,
    pub flags: u8,
}

/// Figures about a finished archive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub file_size: u64,
    pub articles: u32,
    pub redirects: u64,
    pub images: u32,
    pub formulas: u64,
    pub chunks: u32,
    /// Article stream bytes before compression.
    pub text_raw: u64,
    /// Compressed chunk bytes, including their checksums.
    pub text_stored: u64,
    pub image_bytes: u64,
    pub dictionary_bytes: usize,
    pub title_index_bytes: u64,
}

/// A finished item waiting for its turn to be written to BLOBS.
enum Blob {
    Chunk { data: Vec<u8>, raw_len: usize },
    Image { id: u32, data: Vec<u8> },
}

type Job = (u64, Vec<u8>);
type Done = (u64, usize, Result<Vec<u8>>);

struct Workers {
    jobs: Option<SyncSender<Job>>,
    done: Receiver<Done>,
    handles: Vec<JoinHandle<()>>,
}

impl Workers {
    fn start(codec: &ChunkCodec, threads: usize) -> Result<Self> {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(threads * 4);
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let mut encoder = codec.encoder()?;
            let (rx, tx) = (Arc::clone(&job_rx), done_tx.clone());
            handles.push(std::thread::spawn(move || {
                loop {
                    // Hold the lock only while taking a job, not while compressing it.
                    let job = rx.lock().map(|r| r.recv());
                    let Ok(Ok((seq, raw))) = job else { break };
                    let packed = encoder.compress(&raw);
                    if tx.send((seq, raw.len(), packed)).is_err() {
                        break;
                    }
                }
            }));
        }
        Ok(Self {
            jobs: Some(job_tx),
            done: done_rx,
            handles,
        })
    }
}

pub struct ArchiveWriter<W: Write + Seek> {
    out: W,
    opts: WriterOptions,
    codec: ChunkCodec,
    dict_decided: bool,
    threads: usize,
    workers: Option<Workers>,
    /// File position just past the last byte written.
    pos: u64,
    blobs_crc: u32,
    current: Vec<u8>,
    /// Chunks cut before the zstd dictionary exists.
    held: Vec<Vec<u8>>,
    held_bytes: usize,
    chunks_cut: u32,
    next_seq: u64,
    write_seq: u64,
    in_flight: usize,
    ready: BTreeMap<u64, Blob>,
    chunks: Vec<ChunkEntry>,
    /// The ARTICLES section, without popularity, which is set later.
    articles: Spool,
    popularity: Vec<u8>,
    /// Every code point the text uses, for the fallback FONT (spec 10).
    text_chars: normalize::CharSet,
    /// The code points articles draw bold, italic and bold italic.
    styled_chars: [normalize::CharSet; 3],
    /// The TITLES section.
    titles: Spool,
    title_sorter: TitleSorter,
    /// The highest redirect target and its title, checked at the end.
    max_redirect: Option<(u32, String)>,
    /// Encoded image records by id, all zeros until added; ids may arrive
    /// in any order.
    images: Vec<[u8; ImageRecord::SIZE]>,
    meta: Vec<(String, String)>,
    extra_sections: Vec<(u32, Vec<u8>)>,
    required: u32,
    optional: u32,
    summary: Summary,
}

impl ArchiveWriter<BufWriter<File>> {
    /// Creates (or truncates) the file at `path`.
    pub fn create(path: impl AsRef<Path>, opts: WriterOptions) -> Result<Self> {
        Self::new(BufWriter::with_capacity(1 << 20, File::create(path)?), opts)
    }
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub fn new(mut out: W, opts: WriterOptions) -> Result<Self> {
        // Every chunk but the last is full, so a reader finds the chunk
        // for a stream position by arithmetic (spec 7.1).
        if !opts.chunk_size.is_power_of_two() || !(512..=MAX_CHUNK).contains(&opts.chunk_size) {
            return Err(Error::Invalid(format!(
                "chunk size {} (a power of two, 512-{MAX_CHUNK})",
                opts.chunk_size
            )));
        }
        let codec = match opts.profile {
            Profile::Compact => ChunkCodec {
                codec: TextCodec::Deflate,
                level: opts.deflate_level,
                dict: None,
            },
            Profile::Full => ChunkCodec {
                codec: TextCodec::Zstd,
                level: opts.zstd_level,
                dict: opts.zstd_dict.clone(),
            },
        };
        codec.encoder()?; // reject bad levels now rather than on a worker
        let dict_decided = codec.codec == TextCodec::Deflate || codec.dict.is_some();
        let threads = match opts.threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        };
        let articles = Spool::new(opts.temp_dir.clone());
        let titles = Spool::new(opts.temp_dir.clone());
        let title_sorter = TitleSorter::new(opts.title_memory, opts.temp_dir.clone());
        // The superblock is written last; reserve its space now.
        out.seek(SeekFrom::Start(0))?;
        out.write_all(&[0u8; Superblock::SIZE])?;
        let mut required = 0;
        if codec.codec == TextCodec::Zstd {
            required |= required::ZSTD_TEXT;
        }
        Ok(Self {
            out,
            current: Vec::with_capacity(opts.chunk_size),
            opts,
            codec,
            dict_decided,
            threads,
            workers: None,
            pos: BLOBS_START,
            blobs_crc: 0,
            held: Vec::new(),
            held_bytes: 0,
            chunks_cut: 0,
            next_seq: 0,
            write_seq: 0,
            in_flight: 0,
            ready: BTreeMap::new(),
            chunks: Vec::new(),
            articles,
            popularity: Vec::new(),
            text_chars: normalize::CharSet::default(),
            styled_chars: Default::default(),
            titles,
            title_sorter,
            max_redirect: None,
            images: Vec::new(),
            meta: Vec::new(),
            extra_sections: Vec::new(),
            required,
            optional: 0,
            summary: Summary::default(),
        })
    }

    /// Sets a META value (spec 4). Counts and the profile are filled in by
    /// the writer.
    pub fn set_meta(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let (key, value) = (key.into(), value.into());
        match self.meta.iter_mut().find(|(k, _)| *k == key) {
            Some(pair) => pair.1 = value,
            None => self.meta.push((key, value)),
        }
    }

    /// Supplies the MATH_GLYPHS section (spec 8.2). Required when any
    /// article contains a formula.
    pub fn set_math_glyphs(&mut self, data: Vec<u8>) {
        self.set_extra(section::MATH_GLYPHS, data);
    }

    /// Every code point the archive's text uses so far: articles, the
    /// titles of redirects, and META's values (the archive's title and
    /// description).
    pub fn text_chars(&self) -> normalize::CharSet {
        let mut chars = self.text_chars.clone();
        for (_, v) in &self.meta {
            chars.insert_str(v);
        }
        chars
    }

    /// The code points articles so far draw in `style` (spec 10.1). Text
    /// drawn in another style may use them too; for regular, this is
    /// [`Self::text_chars`].
    pub fn styled_chars(&self, style: FontStyle) -> normalize::CharSet {
        match style {
            FontStyle::Regular => self.text_chars(),
            _ => self.styled_chars[style as usize - 1].clone(),
        }
    }

    /// Supplies the fallback FONT section (spec 10), or for another style
    /// FONT_BOLD, FONT_ITALIC or FONT_BOLD_ITALIC (10.1).
    pub fn set_font(&mut self, style: FontStyle, data: Vec<u8>) {
        if style == FontStyle::Regular {
            self.optional |= optional::FONT;
        }
        self.set_extra(style.section(), data);
    }

    /// Supplies the CHARS section (spec 10.2): the code points the font
    /// sections are for.
    pub fn set_font_chars(&mut self, chars: &crate::font::FontChars) {
        self.set_extra(section::CHARS, chars.encode());
    }

    /// Supplies the DAYS section (spec 5.3): the article for each day of
    /// the year, January 1 to December 31 with February 29, `None` where
    /// there is none.
    pub fn set_days(&mut self, days: &[Option<u32>; DAYS_IN_YEAR]) {
        let data = days
            .iter()
            .flat_map(|d| d.unwrap_or(NO_ARTICLE).to_le_bytes())
            .collect();
        self.set_extra(section::DAYS, data);
    }

    fn set_extra(&mut self, kind: u32, data: Vec<u8>) {
        self.extra_sections.retain(|(k, _)| *k != kind);
        self.extra_sections.push((kind, data));
    }

    /// Number of articles added so far, which is also the next article id.
    pub fn article_count(&self) -> u32 {
        self.popularity.len() as u32
    }

    /// One more than the highest image id added so far (the next id when
    /// images are added in order).
    pub fn image_count(&self) -> u32 {
        self.images.len() as u32
    }

    /// Adds article `id`, which must be the next id in sequence. `flags`
    /// adds source knowledge (disambiguation, list); the content flags are
    /// worked out here.
    pub fn add_article(&mut self, id: u32, article: &Article, flags: u16) -> Result<()> {
        if id != self.article_count() {
            return Err(Error::Invalid(format!(
                "article {id} added out of order; expected {}",
                self.article_count()
            )));
        }
        // Titles are tidy lines (spec 7.3) wherever they are stored.
        let tidied;
        let article = match content::tidy_line(&article.title) {
            std::borrow::Cow::Borrowed(_) => article,
            std::borrow::Cow::Owned(title) => {
                tidied = Article {
                    title,
                    ..article.clone()
                };
                &tidied
            }
        };
        let mut flags = flags;
        if article.infobox.is_some() {
            flags |= article_flags::INFOBOX;
        }
        if !article.tables.is_empty() {
            flags |= article_flags::TABLES;
        }
        let mut formulas = 0u64;
        content::walk_refs(article, &mut |r| match r {
            Ref::Formula(_) => {
                formulas += 1;
                flags |= article_flags::MATH;
            }
            Ref::Image(_) => flags |= article_flags::IMAGES,
            Ref::Link(..) | Ref::Table(_) | Ref::Infobox => {}
        });
        if formulas > 0 {
            self.required |= required::MATH;
            self.summary.formulas += formulas;
        }
        content::for_each_styled_text(article, &mut |t, style| {
            self.text_chars.insert_str(t);
            if style != FontStyle::Regular {
                self.styled_chars[style as usize - 1].insert_str(t);
            }
        });
        let stream = content::encode_article(article);
        self.summary.text_raw += stream.len() as u64;

        if self.current.len() >= self.opts.chunk_size {
            self.cut_chunk()?;
        }
        let first_chunk = self.chunks_cut;
        let offset_in_chunk = self.current.len() as u16;
        let mut rest = stream.as_slice();
        let mut chunk_count = 1u32;
        loop {
            let take = (self.opts.chunk_size - self.current.len()).min(rest.len());
            self.current.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if rest.is_empty() {
                break;
            }
            self.cut_chunk()?;
            chunk_count += 1;
        }
        let chunk_count = u16::try_from(chunk_count)
            .map_err(|_| Error::Invalid(format!("article {id} spans {chunk_count} chunks")))?;

        let title_ref = u32::try_from(self.titles.len)
            .map_err(|_| Error::Invalid("titles exceed 4 GB".into()))?;
        let mut len = Vec::with_capacity(4);
        varint::put(&mut len, article.title.len() as u64);
        self.titles.write(&len)?;
        self.titles.write(article.title.as_bytes())?;
        if !article.title.is_empty() {
            self.title_sorter.push(&article.title, id, false, None)?;
        }
        let record = ArticleRecord {
            first_chunk,
            offset_in_chunk,
            chunk_count,
            title_ref,
            flags,
            popularity: 0,
        };
        self.articles.write(&record.encode())?;
        self.popularity.push(0);
        Ok(())
    }

    /// Adds a searchable alternative title for article `target`, which may
    /// be added later; `anchor` is the hash of the place in it a redirect to
    /// a section leads to (spec 6.2). Redirects that normalise to their
    /// target's own key are dropped by the index.
    pub fn add_redirect(&mut self, title: &str, target: u32, anchor: Option<u32>) -> Result<()> {
        let title = content::tidy_line(title);
        let title = title.as_ref();
        if title.is_empty() {
            return Ok(());
        }
        self.text_chars.insert_str(title);
        if self
            .max_redirect
            .as_ref()
            .is_none_or(|(max, _)| target > *max)
        {
            self.max_redirect = Some((target, title.to_string()));
        }
        self.title_sorter.push(title, target, true, anchor)?;
        Ok(())
    }

    /// Sets the 0-255 popularity score of an article already added.
    pub fn set_popularity(&mut self, id: u32, popularity: u8) -> Result<()> {
        let rec = self
            .popularity
            .get_mut(id as usize)
            .ok_or_else(|| Error::Invalid(format!("no article {id}")))?;
        *rec = popularity;
        if popularity > 0 {
            self.optional |= optional::POPULARITY;
        }
        Ok(())
    }

    /// Makes room for `count` images in all, when the number is known in
    /// advance, so the image table takes no more memory than it needs.
    pub fn reserve_images(&mut self, count: u32) {
        let more = (count as usize).saturating_sub(self.images.len());
        self.images.reserve_exact(more);
    }

    /// Adds image `id` with its encoded bytes (spec 9.2). Ids may come in
    /// any order, but every id below the highest must be added before
    /// [`ArchiveWriter::finish`].
    pub fn add_image(&mut self, id: u32, info: ImageInfo, data: Vec<u8>) -> Result<()> {
        if self
            .images
            .get(id as usize)
            .is_some_and(|r| *r != [0; ImageRecord::SIZE])
        {
            return Err(Error::Invalid(format!("image {id} added twice")));
        }
        let size = u32::try_from(data.len())
            .ok()
            .filter(|&s| s <= U24_MAX && s > 0);
        let size =
            size.ok_or_else(|| Error::Invalid(format!("image {id} is {} bytes", data.len())))?;
        match info.encoding {
            image_encoding::JPEG => self.required |= required::JPEG,
            image_encoding::LINE_ART => self.required |= required::LINE_ART,
            other => {
                return Err(Error::Invalid(format!(
                    "image {id}: unknown encoding {other}"
                )));
            }
        }
        if info.flags & image_flags::HD_SOURCE != 0 {
            self.optional |= optional::HD_IMAGES;
        }
        if self.images.len() <= id as usize {
            self.images.resize(id as usize + 1, [0; ImageRecord::SIZE]);
        }
        self.images[id as usize] = ImageRecord {
            offset: 0,
            size,
            encoding: info.encoding,
            width: info.width,
            height: info.height,
            kind: info.kind,
            flags: info.flags,
        }
        .encode();
        self.summary.image_bytes += size as u64;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.ready.insert(seq, Blob::Image { id, data });
        // An image waits for the text chunks before it that are still being
        // compressed: collect those as they finish, and wait for them when
        // too many images are held, or every image would stay in memory
        // until `finish`.
        self.receive(false)?;
        while self.in_flight > 0 && self.ready.len() > self.max_waiting() {
            self.receive(true)?;
        }
        Ok(())
    }

    /// How many finished blobs may wait for an earlier one to be written.
    fn max_waiting(&self) -> usize {
        self.threads * 64
    }

    /// Blobs finished but waiting for an earlier one to be written.
    pub fn waiting(&self) -> usize {
        self.ready.len()
    }

    fn cut_chunk(&mut self) -> Result<()> {
        if self.current.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.current, Vec::with_capacity(self.opts.chunk_size));
        self.chunks_cut += 1;
        if self.dict_decided {
            return self.submit(chunk);
        }
        self.held_bytes += chunk.len();
        self.held.push(chunk);
        if self.held_bytes >= self.opts.dict_sample_bytes {
            self.decide_dictionary()?;
        }
        Ok(())
    }

    /// Trains the zstd dictionary on the held chunks (if there is enough
    /// text) and releases them for compression.
    fn decide_dictionary(&mut self) -> Result<()> {
        if self.codec.codec == TextCodec::Zstd && self.codec.dict.is_none() {
            self.codec.dict = train_dictionary(&self.held, self.opts.dict_size);
        }
        self.dict_decided = true;
        self.held_bytes = 0;
        for chunk in std::mem::take(&mut self.held) {
            self.submit(chunk)?;
        }
        Ok(())
    }

    fn submit(&mut self, chunk: Vec<u8>) -> Result<()> {
        if self.workers.is_none() {
            self.workers = Some(Workers::start(&self.codec, self.threads)?);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        // Bound memory: wait for results once enough chunks are in flight.
        while self.in_flight >= self.threads * 8 {
            self.receive(true)?;
        }
        let workers = self.workers.as_ref().unwrap();
        let jobs = workers.jobs.as_ref().unwrap();
        jobs.send((seq, chunk))
            .map_err(|_| Error::Invalid("compression threads stopped".into()))?;
        self.in_flight += 1;
        self.receive(false)
    }

    /// Collects finished chunks (waiting for at least one if `block`) and
    /// writes everything that is now in order.
    fn receive(&mut self, block: bool) -> Result<()> {
        if let Some(w) = &self.workers {
            let mut wait = block;
            loop {
                let done = if wait {
                    w.done
                        .recv()
                        .map_err(|_| Error::Invalid("compression threads stopped".into()))?
                } else {
                    match w.done.try_recv() {
                        Ok(d) => d,
                        Err(_) => break,
                    }
                };
                wait = false;
                let (seq, raw_len, packed) = done;
                self.in_flight -= 1;
                self.ready.insert(
                    seq,
                    Blob::Chunk {
                        data: packed?,
                        raw_len,
                    },
                );
            }
        }
        self.write_ready()
    }

    fn write_ready(&mut self) -> Result<()> {
        while let Some(blob) = self.ready.remove(&self.write_seq) {
            self.write_seq += 1;
            let offset = self.pos - BLOBS_START;
            if offset > U48_MAX {
                return Err(Error::Invalid("archive exceeds 256 TB".into()));
            }
            match blob {
                Blob::Chunk { data, raw_len } => {
                    let crc = crate::crc::crc32(&data);
                    let stored = data.len() + 4;
                    if stored > U24_MAX as usize {
                        return Err(Error::Invalid(format!(
                            "compressed chunk of {stored} bytes"
                        )));
                    }
                    self.write_blob(&data)?;
                    self.write_blob(&crc.to_le_bytes())?;
                    self.chunks.push(ChunkEntry {
                        offset,
                        stored_size: stored as u32,
                        uncompressed_size: raw_len as u32,
                    });
                    self.summary.text_stored += stored as u64;
                }
                Blob::Image { id, data } => {
                    self.write_blob(&data)?;
                    let rec = &mut self.images[id as usize];
                    let mut decoded = ImageRecord::decode(rec, 0)?;
                    decoded.offset = offset;
                    *rec = decoded.encode();
                }
            }
        }
        Ok(())
    }

    fn write_blob(&mut self, data: &[u8]) -> Result<()> {
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
        self.blobs_crc = crate::crc::crc32_append(self.blobs_crc, data);
        Ok(())
    }

    /// Pads to the next block boundary, where a section starts.
    fn start_section(&mut self) -> Result<u64> {
        let pad = self.pos.next_multiple_of(BLOCK_SIZE as u64) - self.pos;
        self.out.write_all(&vec![0u8; pad as usize])?;
        self.pos += pad;
        Ok(self.pos)
    }

    fn write_section(
        &mut self,
        sections: &mut Vec<SectionEntry>,
        kind: u32,
        data: &[u8],
    ) -> Result<()> {
        let offset = self.start_section()?;
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
        sections.push(SectionEntry {
            kind,
            flags: 0,
            offset,
            length: data.len() as u64,
            crc32: crate::crc::crc32(data),
        });
        Ok(())
    }

    /// Writes the tables and the superblock, and hands back the output. The
    /// archive is unusable until this returns successfully.
    pub fn finish(mut self) -> Result<(W, Summary)> {
        self.cut_chunk()?;
        if !self.dict_decided {
            self.decide_dictionary()?;
        }
        if let Some(w) = &mut self.workers {
            w.jobs = None; // lets the workers finish once the queue is empty
        }
        while self.in_flight > 0 {
            self.receive(true)?;
        }
        if let Some(w) = self.workers.take() {
            for h in w.handles {
                h.join()
                    .map_err(|_| Error::Invalid("compression thread panicked".into()))?;
            }
        }
        debug_assert!(self.ready.is_empty() && self.chunks.len() == self.chunks_cut as usize);

        let article_count = self.article_count();
        if let Some(missing) = self
            .images
            .iter()
            .position(|r| *r == [0; ImageRecord::SIZE])
        {
            return Err(Error::Invalid(format!("image {missing} was never added")));
        }
        if let Some((target, title)) = &self.max_redirect
            && *target >= article_count
        {
            return Err(Error::Invalid(format!(
                "redirect {title:?} points to missing article {target}"
            )));
        }
        if self.required & required::MATH != 0
            && !self
                .extra_sections
                .iter()
                .any(|(k, _)| *k == section::MATH_GLYPHS)
        {
            return Err(Error::Invalid(
                "articles contain formulas but no MATH_GLYPHS section was supplied".into(),
            ));
        }

        let mut sections = vec![SectionEntry {
            kind: section::BLOBS,
            flags: 0,
            offset: BLOBS_START,
            length: self.pos - BLOBS_START,
            crc32: self.blobs_crc,
        }];

        if let Some(dict) = self.codec.dict.clone() {
            self.write_section(&mut sections, section::ZSTD_DICT, &dict)?;
        }
        // The titles list, copied from its temporary file.
        let offset = self.start_section()?;
        let titles = std::mem::replace(&mut self.titles, Spool::new(None));
        let (length, crc32) = (titles.len, titles.crc32);
        titles.copy_to(&mut self.out)?;
        self.pos += length;
        sections.push(SectionEntry {
            kind: section::TITLES,
            flags: 0,
            offset,
            length,
            crc32,
        });
        // The title index: leaf blocks straight from the sorter into the
        // file, then the inner blocks above them.
        let offset = self.start_section()?;
        let sorter = std::mem::replace(&mut self.title_sorter, TitleSorter::new(0, None));
        let popularity = &self.popularity;
        let mut leaves = LeafWriter::new(&mut self.out);
        sorter.finish(|key, display, article, flags, anchor| {
            let score = popularity.get(article as usize).copied().unwrap_or(0);
            leaves.push(key, display, article, flags, anchor, score)
        })?;
        let Leaves {
            separators,
            leaf_popularity,
            key_chars,
            redirect_count,
            bytes: leaf_bytes,
            crc32,
            ..
        } = leaves.finish()?;
        self.pos += leaf_bytes;
        sections.push(SectionEntry {
            kind: section::TINDEX_LEAF,
            flags: 0,
            offset,
            length: leaf_bytes,
            crc32,
        });
        let (inner, _) = title_index::build_inner(separators, leaf_popularity);
        self.write_section(&mut sections, section::TINDEX_INNER, &inner)?;
        // How to fold what is typed as the keys were folded (spec 6.1).
        let fold = normalize::encode_fold_section(&key_chars);
        self.write_section(&mut sections, section::FOLD, &fold)?;
        let redirects = redirect_count as u64;

        let profile = match self.opts.profile {
            Profile::Compact => "compact",
            Profile::Full => "full",
        };
        let mut pairs = std::mem::take(&mut self.meta);
        let computed = [
            ("profile", profile.to_string()),
            ("counts.articles", article_count.to_string()),
            ("counts.redirects", redirects.to_string()),
            ("counts.images", self.images.len().to_string()),
            ("counts.formulas", self.summary.formulas.to_string()),
        ];
        pairs.retain(|(k, _)| !computed.iter().any(|(c, _)| c == k));
        pairs.extend(computed.into_iter().map(|(k, v)| (k.to_string(), v)));

        self.write_section(&mut sections, section::META, &meta::encode(&pairs))?;
        // Article records, with their popularity filled in on the way.
        let offset = self.start_section()?;
        let articles = std::mem::replace(&mut self.articles, Spool::new(None));
        let length = articles.len;
        let popularity = std::mem::take(&mut self.popularity);
        let crc32 = articles.copy_records_to(&mut self.out, ArticleRecord::SIZE, |i, rec| {
            rec[ArticleRecord::POPULARITY] = popularity[i];
        })?;
        self.pos += length;
        sections.push(SectionEntry {
            kind: section::ARTICLES,
            flags: 0,
            offset,
            length,
            crc32,
        });
        let chunks: Vec<u8> = self.chunks.iter().flat_map(|c| c.encode()).collect();
        self.write_section(&mut sections, section::CHUNK_TABLE, &chunks)?;
        if !self.images.is_empty() {
            let images: Vec<u8> = self.images.concat();
            self.write_section(&mut sections, section::IMAGE_TABLE, &images)?;
        }
        for (kind, data) in std::mem::take(&mut self.extra_sections) {
            if kind == section::DAYS {
                let missing = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b))
                    .find(|&id| id != NO_ARTICLE && id >= article_count);
                if let Some(id) = missing {
                    return Err(Error::Invalid(format!("DAYS names missing article {id}")));
                }
            }
            self.write_section(&mut sections, kind, &data)?;
        }

        let sb = Superblock {
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            file_size: self.pos,
            required_features: self.required,
            optional_features: self.optional,
            profile: self.opts.profile,
            text_codec: self.codec.codec,
            deflate_window_bits: 15,
            chunk_bits: self.opts.chunk_size.trailing_zeros() as u8,
            sections,
        };
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&sb.encode()?)?;
        self.out.flush()?;

        let mut summary = std::mem::take(&mut self.summary);
        summary.file_size = self.pos;
        summary.articles = article_count;
        summary.redirects = redirects;
        summary.images = self.images.len() as u32;
        summary.chunks = self.chunks.len() as u32;
        summary.dictionary_bytes = self.codec.dict.as_ref().map_or(0, Vec::len);
        summary.title_index_bytes = leaf_bytes + inner.len() as u64;
        Ok((self.out, summary))
    }
}

/// Bytes for a section that grows while the archive is written but goes
/// after BLOBS, kept in a temporary file until then.
struct Spool {
    dir: Option<PathBuf>,
    file: Option<BufWriter<File>>,
    len: u64,
    crc32: u32,
}

impl Spool {
    fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            file: None,
            len: 0,
            crc32: 0,
        }
    }

    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let file = match &mut self.file {
            Some(f) => f,
            None => {
                let f = match &self.dir {
                    Some(dir) => tempfile::tempfile_in(dir)?,
                    None => tempfile::tempfile()?,
                };
                self.file.insert(BufWriter::with_capacity(1 << 20, f))
            }
        };
        file.write_all(data)?;
        self.len += data.len() as u64;
        self.crc32 = crate::crc::crc32_append(self.crc32, data);
        Ok(())
    }

    /// Copies fixed-size records to `out`, letting `patch` change each
    /// one (by index) on the way, and returns the CRC-32 of what it wrote.
    fn copy_records_to(
        self,
        out: &mut impl Write,
        size: usize,
        mut patch: impl FnMut(usize, &mut [u8]),
    ) -> io::Result<u32> {
        let mut crc = 0;
        if let Some(w) = self.file {
            let mut f = w.into_inner().map_err(|e| e.into_error())?;
            f.seek(SeekFrom::Start(0))?;
            let mut buf = vec![0u8; size * 4096];
            let (mut index, mut left) = (0, self.len);
            while left > 0 {
                let n = buf.len().min(left as usize);
                f.read_exact(&mut buf[..n])?;
                for rec in buf[..n].chunks_exact_mut(size) {
                    patch(index, rec);
                    index += 1;
                }
                crc = crate::crc::crc32_append(crc, &buf[..n]);
                out.write_all(&buf[..n])?;
                left -= n as u64;
            }
        }
        Ok(crc)
    }

    fn copy_to(self, out: &mut impl Write) -> io::Result<()> {
        if let Some(w) = self.file {
            let mut f = w.into_inner().map_err(|e| e.into_error())?;
            f.seek(SeekFrom::Start(0))?;
            let copied = io::copy(&mut f, out)?;
            if copied != self.len {
                return Err(io::Error::other("a temporary file came back short"));
            }
        }
        Ok(())
    }
}

/// Replaces sections of the finished archive at `path`, in place: each
/// `(kind, Some(bytes))` is written after the end of the file and takes
/// the place of any section of that kind, and `(kind, None)` removes it.
/// Old bytes stay where they are, no longer referred to, and the
/// superblock is written last, so until then the archive is as it was.
/// Optional feature bit 0 follows whether a FONT section is left.
pub fn patch_sections(path: &Path, changes: &[(u32, Option<Vec<u8>>)]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut head = vec![0u8; Superblock::SIZE];
    file.read_exact(&mut head)?;
    let mut sb = Superblock::decode(&head)?;
    let mut end = file.seek(SeekFrom::End(0))?;
    for (kind, data) in changes {
        sb.sections.retain(|s| s.kind != *kind);
        let Some(data) = data else { continue };
        let offset = end.next_multiple_of(BLOCK_SIZE as u64);
        file.seek(SeekFrom::Start(end))?;
        file.write_all(&vec![0u8; (offset - end) as usize])?;
        file.write_all(data)?;
        end = offset + data.len() as u64;
        sb.sections.push(SectionEntry {
            kind: *kind,
            flags: 0,
            offset,
            length: data.len() as u64,
            crc32: crate::crc::crc32(data),
        });
    }
    if sb.section(section::FONT).is_some() {
        sb.optional_features |= optional::FONT;
    } else {
        sb.optional_features &= !optional::FONT;
    }
    sb.file_size = end;
    let head = sb.encode()?;
    file.sync_data()?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    file.sync_all()?;
    Ok(())
}
