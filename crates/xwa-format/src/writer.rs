//! Streaming archive writer.
//!
//! Content chunks and images go into BLOBS as they arrive, so memory use is
//! bounded by the tables (about 40 bytes per article) rather than the
//! archive size. Chunks are compressed on worker threads; the file layout
//! depends only on the order of calls, so the same input always produces
//! the same bytes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::codec::{ChunkCodec, MAX_CHUNK, train_dictionary};
use crate::content::{self, Article, Ref};
use crate::records::{
    ArticleRecord, ChunkEntry, ImageRecord, U24_MAX, U48_MAX, article_flags, image_encoding,
};
use crate::superblock::{SectionEntry, Superblock};
use crate::title_index::{self, TitleEntry};
use crate::{
    BLOCK_SIZE, Error, Profile, Result, TextCodec, VERSION_MAJOR, VERSION_MINOR, meta, optional,
    required, section, varint,
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
    /// Uncompressed chunk size, at most [`MAX_CHUNK`].
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
    articles: Vec<ArticleRecord>,
    titles: Vec<u8>,
    title_entries: Vec<TitleEntry>,
    images: Vec<ImageRecord>,
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
        if opts.chunk_size == 0 || opts.chunk_size > MAX_CHUNK {
            return Err(Error::Invalid(format!(
                "chunk size {} (1-{MAX_CHUNK})",
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
            articles: Vec::new(),
            titles: Vec::new(),
            title_entries: Vec::new(),
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

    /// Supplies the fallback FONT section (spec 10).
    pub fn set_font(&mut self, data: Vec<u8>) {
        self.optional |= optional::FONT;
        self.set_extra(section::FONT, data);
    }

    fn set_extra(&mut self, kind: u32, data: Vec<u8>) {
        self.extra_sections.retain(|(k, _)| *k != kind);
        self.extra_sections.push((kind, data));
    }

    /// Number of articles added so far, which is also the next article id.
    pub fn article_count(&self) -> u32 {
        self.articles.len() as u32
    }

    /// Number of images added so far, which is also the next image id.
    pub fn image_count(&self) -> u32 {
        self.images.len() as u32
    }

    /// Adds article `id`, which must be the next id in sequence. `flags`
    /// adds source knowledge (disambiguation, list); the content flags are
    /// worked out here.
    pub fn add_article(&mut self, id: u32, article: &Article, flags: u16) -> Result<()> {
        if id as usize != self.articles.len() {
            return Err(Error::Invalid(format!(
                "article {id} added out of order; expected {}",
                self.articles.len()
            )));
        }
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
            Ref::Link(_) | Ref::Table(_) | Ref::Infobox => {}
        });
        if formulas > 0 {
            self.required |= required::MATH;
            self.summary.formulas += formulas;
        }
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

        let title_ref = u32::try_from(self.titles.len())
            .map_err(|_| Error::Invalid("titles exceed 4 GB".into()))?;
        varint::put(&mut self.titles, article.title.len() as u64);
        self.titles.extend_from_slice(article.title.as_bytes());
        if !article.title.is_empty() {
            self.title_entries
                .push(TitleEntry::new(article.title.clone(), id, false));
        }
        self.articles.push(ArticleRecord {
            first_chunk,
            offset_in_chunk,
            chunk_count,
            title_ref,
            flags,
            popularity: 0,
        });
        Ok(())
    }

    /// Adds a searchable alternative title for article `target`. Redirects
    /// that normalise to their target's own key are dropped by the index.
    pub fn add_redirect(&mut self, title: &str, target: u32) {
        if !title.is_empty() {
            self.title_entries
                .push(TitleEntry::new(title, target, true));
        }
    }

    /// Sets the 0-255 popularity score of an article already added.
    pub fn set_popularity(&mut self, id: u32, popularity: u8) -> Result<()> {
        let rec = self
            .articles
            .get_mut(id as usize)
            .ok_or_else(|| Error::Invalid(format!("no article {id}")))?;
        rec.popularity = popularity;
        if popularity > 0 {
            self.optional |= optional::POPULARITY;
        }
        Ok(())
    }

    /// Adds image `id`, which must be the next id in sequence, with its
    /// encoded bytes (spec 9.2).
    pub fn add_image(&mut self, id: u32, info: ImageInfo, data: Vec<u8>) -> Result<()> {
        if id as usize != self.images.len() {
            return Err(Error::Invalid(format!(
                "image {id} added out of order; expected {}",
                self.images.len()
            )));
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
        self.images.push(ImageRecord {
            offset: 0,
            size,
            encoding: info.encoding,
            width: info.width,
            height: info.height,
            kind: info.kind,
            flags: info.flags,
        });
        self.summary.image_bytes += size as u64;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.ready.insert(seq, Blob::Image { id, data });
        self.write_ready()
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
                    let crc = crc32c::crc32c(&data);
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
                    self.images[id as usize].offset = offset;
                }
            }
        }
        Ok(())
    }

    fn write_blob(&mut self, data: &[u8]) -> Result<()> {
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
        self.blobs_crc = crc32c::crc32c_append(self.blobs_crc, data);
        Ok(())
    }

    fn write_section(
        &mut self,
        sections: &mut Vec<SectionEntry>,
        kind: u32,
        data: &[u8],
    ) -> Result<()> {
        let pad = self.pos.next_multiple_of(BLOCK_SIZE as u64) - self.pos;
        self.out.write_all(&vec![0u8; pad as usize])?;
        self.pos += pad;
        sections.push(SectionEntry {
            kind,
            flags: 0,
            offset: self.pos,
            length: data.len() as u64,
            crc32c: crc32c::crc32c(data),
        });
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
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

        let article_count = self.articles.len() as u32;
        if let Some(e) = self
            .title_entries
            .iter()
            .find(|e| e.article >= article_count)
        {
            return Err(Error::Invalid(format!(
                "redirect {:?} points to missing article {}",
                e.display, e.article
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
            crc32c: self.blobs_crc,
        }];

        let titled = self
            .title_entries
            .iter()
            .filter(|e| e.flags & title_index::FLAG_REDIRECT == 0)
            .count();
        let index = title_index::build(std::mem::take(&mut self.title_entries));
        let redirects = (index.entry_count - titled) as u64;

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

        if let Some(dict) = self.codec.dict.clone() {
            self.write_section(&mut sections, section::ZSTD_DICT, &dict)?;
        }
        self.write_section(&mut sections, section::META, &meta::encode(&pairs))?;
        let articles: Vec<u8> = self.articles.iter().flat_map(|a| a.encode()).collect();
        self.write_section(&mut sections, section::ARTICLES, &articles)?;
        let titles = std::mem::take(&mut self.titles);
        self.write_section(&mut sections, section::TITLES, &titles)?;
        self.write_section(&mut sections, section::TINDEX_INNER, &index.inner)?;
        self.write_section(&mut sections, section::TINDEX_LEAF, &index.leaves)?;
        let chunks: Vec<u8> = self.chunks.iter().flat_map(|c| c.encode()).collect();
        self.write_section(&mut sections, section::CHUNK_TABLE, &chunks)?;
        if !self.images.is_empty() {
            let images: Vec<u8> = self.images.iter().flat_map(|i| i.encode()).collect();
            self.write_section(&mut sections, section::IMAGE_TABLE, &images)?;
        }
        for (kind, data) in std::mem::take(&mut self.extra_sections) {
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
        summary.title_index_bytes = (index.leaves.len() + index.inner.len()) as u64;
        Ok((self.out, summary))
    }
}
