# Offline wiki archive format, draft v0

Status: draft for review. The format is XWA (`.xwa` files); the converter
is xwa-tool. Everything here is open to change until the first reader and
writer agree on it.

## 1. Goals

1. Fast on the device: a title lookup costs 1-3 SD reads, opening an
   article 2 reads, following a link 1-2 reads, jumping to any section or
   table 1-2 reads. No B-tree walks for images, formulas or links.
2. Small: a front-coded title index (~28 B per entry measured), math as
   vectors (~93 B per formula measured), lossless 2-bit line art.
3. Device profiles: one container, two encodings. The X4 Pro (ESP32-S3,
   8 MB PSRAM) gets everything it can use; the X4/X3 (ESP32-C3, ~380 KB
   RAM) gets a compatible subset. Nothing in the container forces the X4 Pro
   down to C3 limits.
4. Settings-driven rendering: the archive stores enough (2-bit line art,
   high-resolution images, vector math) for the firmware to offer 1-bit or
   4-grey, normal or enlarged diagrams, shrink-to-fit and a zoom viewer
   without re-converting.
5. Robust: versioned, feature-flagged, checksummed, 64-bit offsets, no
   truncated keys, no contiguity requirement.

Non-goals for v0: full-text search, editing, multi-file archives, sources
other than Wikipedia (the content model is source-neutral, the extractor is
not yet).

### 1.1 Profiles

| | compact (X4/X3) | full (X4 Pro) |
|---|---|---|
| Text chunks | raw deflate (uzlib) | zstd + shared dictionary (-21% text) |
| Line-art images | LA2 + deflate | LA2 + zstd |
| Everything else | identical | identical |

The X4 Pro firmware reads both profiles; the X4/X3 firmware reads compact
only and refuses full archives with a clear message. The converter picks
the profile from the target device.

## 2. Conventions

- Little-endian. `u8/u16/u32/u64`, `i16/i32`, `varint` = unsigned LEB128,
  `svarint` = zigzag LEB128.
- `u48` offsets (6 bytes) in tables where space matters; 256 TB limit.
- Sections start on 4096-byte boundaries (8 SD sectors).
- Strings are UTF-8, length-prefixed with a varint unless stated otherwise.
- CRC32C (Castagnoli) for integrity checks.

## 3. File layout

```
offset 0      Superblock (4096 bytes)
4096...       Sections, each 4096-aligned, in any order
```

### 3.1 Superblock

| Offset | Type | Field |
|---|---|---|
| 0 | u8[8] | magic `XWA\0\r\n\x1a\n` |
| 8 | u16 | version_major (0 = draft) |
| 10 | u16 | version_minor |
| 12 | u32 | superblock_size (4096) |
| 16 | u64 | file_size |
| 24 | u32 | required_features (bitset, see 3.3) |
| 28 | u32 | optional_features (bitset) |
| 32 | u8 | profile: 0 = compact (X4/X3), 1 = full (X4 Pro) |
| 33 | u8 | text_codec: 0 = raw deflate, 1 = zstd |
| 34 | u8 | deflate_window_bits (9-15) |
| 35 | u8 | reserved |
| 36 | u32 | section_count |
| 40 | Section[section_count] | section directory, 32 bytes each |
| 4092 | u32 | CRC32C of bytes 0..4091 |

`Section` (32 bytes): `u32 type`, `u32 flags`, `u64 offset`, `u64 length`,
`u32 crc32c` (of the section bytes, 0 = not computed), `u32 reserved`.

`file_size` must equal the size of the file. A reader that finds a shorter
file reports an incomplete copy rather than reading past the end; copying
large files to SD cards fails this way often enough to matter.

### 3.2 Section types

| Type | Name | Required | Purpose |
|---|---|---|---|
| 1 | META | yes | Key/value metadata |
| 2 | ARTICLES | yes | Article table |
| 3 | TITLES | yes | Display titles blob |
| 4 | TINDEX_LEAF | yes | Front-coded title index leaf blocks |
| 5 | TINDEX_INNER | yes | Upper levels of the title index |
| 6 | CHUNK_TABLE | yes | Content chunk locations |
| 7 | BLOBS | yes | Compressed content chunks and encoded images |
| 8 | IMAGE_TABLE | if images | Image records |
| 9 | MATH_GLYPHS | if math | Glyph outline table |
| 10 | ZSTD_DICT | optional | Shared zstd dictionary (zstd archives only) |
| 11 | FONT | optional | Fallback font (a complete `.cpfont` v4 file) |
| 12 | WORD_INDEX | optional | Title-word search index (later) |

Readers ignore unknown section types.

BLOBS holds content chunks and image data in whatever order the writer
produced them, so a converter can stream both into one file in a single
pass. CHUNK_TABLE and IMAGE_TABLE offsets are relative to the start of
BLOBS. xwa-tool writes BLOBS first, at offset 4096, and the tables after
it, but readers must not rely on any section order.

### 3.3 Feature flags

Required (a reader that lacks one must refuse the archive with a clear
message): bit 0 zstd text, bit 1 math glyph runs, bit 2 line-art images
(LA2), bit 3 JPEG images. Optional (safe to ignore): bit 0 fallback font,
bit 1 word index, bit 2 popularity scores, bit 3 HD images present.

## 4. META section

`varint count`, then `count` pairs of strings (key, value). Keys used:
`title`, `description`, `language` (ISO 639-3), `source.name`,
`source.flavour`, `source.date`, `source.uuid`, `source.scraper`,
`license`, `attribution`, `created` (RFC 3339), `converter` (name and
version), `profile`, `counts.articles`, `counts.redirects` (redirect
entries in the title index, after de-duplication), `counts.images`,
`counts.formulas`, `main_article` (decimal article id).

Wikipedia content is CC BY-SA: `attribution` must be shown somewhere in the
firmware UI (archive info screen).

## 5. Articles

### 5.1 Article table (ARTICLES)

Article id = index into this table (`u32`). Redirects are not articles;
they exist only in the title index. Record, 16 bytes:

| Field | Type | Meaning |
|---|---|---|
| first_chunk | u32 | Chunk holding the start of the article stream |
| offset_in_chunk | u16 | Byte offset of the stream in the decompressed chunk |
| chunk_count | u16 | Chunks the stream touches, `first_chunk` included |
| title_ref | u32 | Offset into TITLES |
| flags | u16 | infobox, tables, math, images, disambiguation, list |
| popularity | u8 | 0-255, log-scaled incoming link count (optional) |
| reserved | u8 | |

Random article: pick a uniform id in `[0, count)`. Lookup by id: one read.

### 5.2 Titles (TITLES)

Concatenated `varint length + UTF-8` display titles, addressed by
`title_ref`. Used for headers, history and link previews without
decompressing content.

## 6. Title index

### 6.1 Normalisation

`key = collapse_whitespace(casefold(strip_combining(NFKD(title))))`, with
`_` treated as a space. Search input is normalised the same way, so
"albert einstein", "Albert Einstein" and "Älbert Einstein" match.

### 6.2 Entries

One entry per article title and per redirect, at most one per (key,
article) pair: a redirect whose key equals its target's key is dropped
(4,187 of 71,542 in the mathematics ZIM), and spellings of the same key
pointing at the same article collapse to one ("Godel code" and "Gödel code"),
keeping the article's own title, then the accented spelling. Entries are
sorted by key bytes, then display title.

Entry encoding inside a leaf block:

```
varint key_shared      bytes shared with the previous key in this block
varint key_suffix_len, key suffix bytes
varint disp_shared     bytes shared with the previous display title
varint disp_suffix_len, display suffix bytes
varint article_id
u8     flags           bit0 redirect, bits1-7 reserved
```

The first entry of each block has `*_shared = 0`.

### 6.3 Leaf blocks (TINDEX_LEAF)

4096-byte blocks: `u16 entry_count`, `u16 used_bytes`, entries, zero
padding, `u32 crc32c` in the last 4 bytes. Measured with the Rust builder: 28.5
bytes per entry for the mathematics ZIM (104,216 entries, 2.99 MB, built in
0.4 s) and 23.8 for chemistry; about 0.6 GB for English Wikipedia's ~21.7 M
titles.

### 6.4 Inner levels (TINDEX_INNER)

A static B+tree over leaf blocks, built bottom-up. Each 4096-byte inner
block holds front-coded first keys of its children plus `u32` child block
numbers (about 270 children per block). The root is block 0. English
Wikipedia works out to ~155K leaf blocks under ~580 inner blocks (~2.4 MB):
the X4 Pro caches every inner block in PSRAM (1 SD read per lookup); the C3
caches only the root (up to 3 reads).

### 6.5 Prefix search

Descend to the first key `>= prefix`, then scan forward while keys start
with the prefix, collecting up to N results. Ranking by `popularity` among
the first M matches is optional.

## 7. Content

### 7.1 Chunks

Each article is one byte stream (7.2). Streams are concatenated in article
id order and cut into chunks of at most 32,768 bytes uncompressed, in both
profiles; a stream may start partway through one chunk and continue through
the following ones. Small articles share a chunk; large ones span several.
Each chunk is compressed independently:

- compact profile: raw deflate (window = `deflate_window_bits`, 15),
  decodable by the firmware's uzlib. xwa-tool compresses with libdeflate at
  level 12, 3.3% smaller than zlib's level 9 at about 6 MB/s per core
  (zopfli was only 0.6% smaller again, at a twentieth of the speed).
- full profile: one zstd frame per chunk, single-segment (content size in
  the header, so the decoder needs no window beyond the chunk itself),
  without checksum or dictionary id, compressed with the archive's
  ZSTD_DICT when there is one. xwa-tool trains the dictionary (110 KB) on
  the first 16 MB of chunks; sampling the whole archive instead measured
  only 0.2% better. Archives with too little text to train on have no
  ZSTD_DICT. Measured on the mathematics ZIM's text with 32 KB chunks:
  77.0 MB, against 98.4 MB for deflate at the same chunk size (-22%).

Why 32 KB: with the dictionary, 64 KB and 128 KB chunks would save only
1.0% and 1.7%, while every chunk read costs RAM on the C3 and decompression
time on both devices.

CHUNK_TABLE entry, 12 bytes: `u48 offset` (within BLOBS), `u24
stored_size`, `u24 uncompressed_size` (1-32,768). The stored bytes are the
compressed chunk followed by a `u32` CRC32C of the compressed bytes, so
corruption is caught before decompression.

To read from position `p` of an article stream, start at
(`first_chunk`, `offset_in_chunk`) and walk forward through the following
chunks' uncompressed sizes.

### 7.2 Article stream

At `offset_in_chunk` of `first_chunk`:

```
varint stream_size     bytes that follow this field
string title
varint body_size
varint toc_count, then per heading: u8 level, string text, varint pos
varint table_count, then per table: varint pos
varint infobox_pos_plus_one   (0 = no infobox)
body                   block stream (7.3), body_size bytes
tables                 table records (7.4), one per table
infobox                infobox record (7.4), if any
```

Every `pos` is a byte position measured from the first byte of the body, so
the header never depends on its own size. Jumping to a section or table
decompresses only the chunk that holds that position.

### 7.3 Block stream

A pre-order sequence of opcodes. Unknown opcodes carry a length so readers
can skip them.

Block opcodes:

| Op | Name | Payload |
|---|---|---|
| 0x01 | PARA | inline run (7.4) |
| 0x02 | HEADING | u8 level, inline run |
| 0x03 | LIST_START | u8 kind (bullet, number, definition) |
| 0x04 | LIST_ITEM | (blocks follow until next item or end) |
| 0x05 | LIST_END | |
| 0x06 | QUOTE_START / 0x07 QUOTE_END | |
| 0x08 | PRE | string (monospace, no wrapping) |
| 0x09 | RULE | |
| 0x0A | FIGURE | varint image_id, inline run (caption) |
| 0x0B | MATH_BLOCK | formula (8.1) |
| 0x0C | TABLE_REF | varint table index (into header table list) |
| 0x0D | INFOBOX_REF | |
| 0x0E | GALLERY | varint n, n × (image_id, inline run) |
| 0x7F | EXT | varint length, payload (skippable) |

An inline run is `varint length` followed by inline opcodes:

| Op | Name | Payload |
|---|---|---|
| 0x01-0x7F | TEXT | short text: op = length (1-127), then UTF-8 bytes |
| 0x80 | TEXT_LONG | varint length, UTF-8 |
| 0x81 | STYLE_PUSH | u8 style (bold, italic, sub, sup, mono, smallcaps) |
| 0x82 | STYLE_POP | |
| 0x83 | LINK_START | varint target article id, varint section index (0 = top) |
| 0x84 | LINK_END | |
| 0x85 | MATH | formula (8.1) |
| 0x86 | ICON | varint image_id (small inline image: flags, symbols) |
| 0x87 | BREAK | line break |
| 0x88 | NOTE_REF | varint footnote number (footnotes later) |

Links are resolved at conversion time: redirects are followed to the final
article id, and links to missing articles are emitted as plain text. The
device never searches to follow a link.

Text is stored in logical order. Right-to-left handling (pre-shaping as
today, or on-device bidi with the existing MiniBidi) is an open question.

### 7.4 Tables and infobox

Stored in the article's chunks after the body:

```
TABLE: varint rows, varint cols, u8 flags, varint cell_count,
       per cell (row-major, by top-left corner):
                 u8 cell_flags (bit0 header), varint rowspan, varint colspan,
                 inline run
INFOBOX: varint row_count, per row: u8 kind (header, image, label/value,
         full-width), payload (inline runs, image_id)
```

## 8. Math

### 8.1 Formula record

Used by MATH (inline) and MATH_BLOCK:

```
u8     flags         bit0 display style, bit1 has fallback image
svarint width, ascent, descent   (font units of MATH_GLYPHS)
varint item_count
items:
  tag 0 GLYPH     varint glyph_id, u8 scale_code, svarint dx, svarint dy
  tag 1 GLYPH_XY  varint glyph_id, i16 sx, i16 sy (8.8 fixed), svarint dx, svarint dy
  tag 2 RULE      svarint dx, svarint dy, varint w, varint h
[varint fallback_image_id]
```

`dx/dy` are relative to the previous item's position, in font units.
`scale_code` indexes a small table in MATH_GLYPHS (1.0, 0.707, 0.5 ...).
GLYPH_XY covers vertically stretched delimiter pieces. Measured on 316,891
real formulas: 99.86% are representable, 93 B mean, 59 B median.

Formulas are resolution-independent: the firmware chooses px/em per
setting (match text, shrink to fit, zoom viewer).

Vector math is not an image, so archives built without images still carry
it. A formula the math engine cannot lay out falls back to a fallback image
(when images are on) or to plain Unicode text in the content stream.

### 8.2 Glyph outlines (MATH_GLYPHS)

Required whenever an article contains a formula (feature bit 1).

```
u16 upem, u16 glyph_count, u8 scale_count, u16 scales[scale_count] (8.8)
u32 glyph_offset[glyph_count + 1]
per glyph: i16 xmin, ymin, xmax, ymax, then ops:
  0 MOVE x y | 1 LINE x y | 2 QUAD cx cy x y | 3 CUBIC c1x c1y c2x c2y x y
  4 CLOSE | 5 END          (i16 coordinates, font units, y up)
```

Glyphs come either from Wikimedia's MathJax SVGs (compiled) or from our own
math layout (LaTeX/MathML via a TeX-style engine). The firmware rasterises
them with a small integer rasteriser (prototype: 2.1 KB RV32 code, ≤ 8 KB
RAM, ~3 ms per inline formula on the C3).

## 9. Images

### 9.1 Image table (IMAGE_TABLE)

Image id = index. Record, 16 bytes: `u48 offset` (within BLOBS), `u24
size`, `u8 encoding`, `u16 width`, `u16 height`, `u8 kind` (photo,
line art, icon), `u8 flags` (HD source, has transparency).

### 9.2 Encodings

- `encoding 1` JPEG: baseline, 8-bit greyscale, decodable by JPEGDEC
  (which also decodes at 1/2, 1/4, 1/8 scale cheaply). For photos.
- `encoding 2` LA2: line art, 4 grey levels. Header `u16 w, u16 h, u8 bpp
  (1 or 2), u8 codec (0 raw deflate, 1 zstd)`, then rows packed MSB-first,
  compressed as one stream. Pixel values are ink levels: 0 is paper
  (white) and the maximum (1 or 3) is black. Deflate with a ≤ 4 KB window
  compresses within 1-3% of a 32 KB window (measured), so the C3 needs only
  a small buffer.

### 9.3 Resolution policy

One stored rendition per image, at the larger of what the zoom viewer can
use and what the inline "enlarged" setting needs, capped at 800 px on the
long side (landscape width) and at the source resolution (or the HD
download, if fetched). The firmware downsamples for inline display, so the
diagram-size setting and the zoom viewer need no extra data. 1-bit display
is derived from LA2 by threshold or patterns; 4-grey uses the levels as
stored.

## 10. Fallback font (FONT)

A complete `.cpfont` v4 file (the firmware's existing SD font format),
subset to the code points the archive needs that the built-in fonts do not
cover. Replaces the 16 MB FBF1 blob appended to every `.img` today. The
firmware needs an offset-based variant of its existing `.cpfont` loader.

## 11. Device I/O budget (expected)

| Operation | Reads |
|---|---|
| Search keystroke (prefix) | 1-3 (index) + 1 per extra leaf block |
| Open article | 1 (article record) + 1 (chunk table) + 1 (chunk) |
| Follow link | same as open article; no search |
| Jump to section/table | 1-2 (chunk table, chunk) |
| Show image | 1 (image record, cached per page) + 1 (data) |
| Draw formula | 0 (inline in content) + glyph outlines (cached per page) |

The firmware should map the archive file's extents at open time so raw
sector reads work without the "copy contiguously" step.

## 12. Open questions

1. RTL: keep pre-shaping in the converter or move bidi to the device.
2. Footnotes/references: include as NOTE_REF with a per-article notes block?
3. Compact-profile deflate window: 32 KB (as today) or smaller.
4. FAT32 cards: archives over 4 GB need exFAT; split archives are out of
   scope for v0.
5. Math source priority: our own layout first and Wikimedia SVGs as a
   fallback, or the other way round, during the transition.
