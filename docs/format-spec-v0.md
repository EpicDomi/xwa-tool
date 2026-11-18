# Offline wiki archive format, draft v0

Status: draft for review. The format is XWA (`.xwa` files); the converter
is xwa-tool. This text describes version 0.4. Everything here is open to
change until the first reader and writer agree on it.

## 1. Goals

1. Fast on the device: a title lookup costs 1-3 SD reads, opening an
   article 2 reads, following a link 1-2 reads, jumping to any section or
   table 1-2 reads. No B-tree walks for images, formulas or links.
2. Small: a front-coded title index (~26-30 B per entry measured), math as
   vectors (~82 B per formula measured), lossless 2-bit line art.
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
- Checksums are CRC-32 as in zlib, PNG and the ESP32's ROM (polynomial
  0x04C11DB7, reflected; initial value and final XOR 0xFFFFFFFF; the
  CRC-32 of the ASCII bytes `123456789` is 0xCBF43926).
- Reserved fields and flag bits are 0; readers ignore them.

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
| 10 | u16 | version_minor (4 = this text) |
| 12 | u32 | superblock_size (4096) |
| 16 | u64 | file_size |
| 24 | u32 | required_features (bitset, see 3.3) |
| 28 | u32 | optional_features (bitset) |
| 32 | u8 | profile: 0 = compact (X4/X3), 1 = full (X4 Pro) |
| 33 | u8 | text_codec: 0 = raw deflate, 1 = zstd |
| 34 | u8 | deflate_window_bits (9-15) |
| 35 | u8 | chunk_bits: every content chunk but the last holds 2^chunk_bits bytes (9-15; xwa-tool writes 15) |
| 36 | u32 | section_count |
| 40 | Section[section_count] | section directory, 32 bytes each |
| 4092 | u32 | CRC-32 of bytes 0..4091 |

`Section` (32 bytes): `u32 type`, `u32 flags` (reserved), `u64 offset`,
`u64 length`, `u32 crc32` (of the section bytes, 0 = not computed), `u32
reserved`. A type appears at most once.

`file_size` must equal the size of the file. A reader that finds a shorter
file reports an incomplete copy rather than reading past the end; copying
large files to SD cards fails this way often enough to matter.

While `version_major` is 0, every minor version may change the layout:
readers accept only the minor version they were written for, and tell the
user to convert the archive again when it is older. Version 0.2 added named
places (7.2): anchor links (7.3) and anchors on title index entries (6.2).
Version 0.3, from the firmware reader's review, made search keys fold
through a table the archive carries (6.1, FOLD), put popularity in the
title index (6.2, 6.4) for a most-popular-first search (6.5), shortened the
inner blocks' keys (6.4), fixed the chunk size (7.1), made heading anchors
stateless (7.2), gave icons a size (7.3) and formula records a length
(8.1), wrote down what readers may rely on in text (7.3), added DAYS (5.3),
and changed the checksums from CRC32C to CRC-32. Version 0.4 made FONT
outline glyphs that one rasteriser draws at any size (10), and gave tables
their caption (7.4).

### 3.2 Section types

| Type | Name | Required | Purpose |
|---|---|---|---|
| 1 | META | yes | Key/value metadata |
| 2 | ARTICLES | yes | Article table |
| 3 | TITLES | yes | Display titles blob |
| 4 | TINDEX_LEAF | yes | Front-coded title index leaf blocks |
| 5 | TINDEX_INNER | yes | Upper levels of the title index (empty over one leaf block) |
| 6 | CHUNK_TABLE | yes | Content chunk locations |
| 7 | BLOBS | yes | Compressed content chunks and encoded images |
| 8 | IMAGE_TABLE | if images | Image records |
| 9 | MATH_GLYPHS | if math | Glyph outline table |
| 10 | ZSTD_DICT | optional | Shared zstd dictionary (zstd archives only) |
| 11 | FONT | optional | Outline glyphs for what the firmware's fonts lack (10) |
| 12 | WORD_INDEX | optional | Title-word search index (later) |
| 13 | FOLD | yes | How to fold search input (6.1) |
| 14 | DAYS | optional | The article for each day of the year (5.3) |

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

### 3.4 Where archives go

Archives are files ending in `.xwa` in the `wiki` folder at the top of the
SD card (`/wiki/`); a card can hold several, under any names. Readers list
that folder; FAT32 and exFAT match its name in any case.

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
| flags | u16 | bit 0 infobox, 1 tables, 2 math, 3 images, 4 disambiguation, 5 list |
| popularity | u8 | 0-255, how many articles link to it, log-scaled |
| reserved | u8 | |

xwa-tool sets `popularity` to round(13 × log2(1 + n)), at most 255, for n
links from other articles: 1 link gives 13, 1,000 give 130. Optional
feature bit 2 says the scores were computed; without it they are all 0.
Each title index entry carries its article's score too (6.2).

Random article: pick a uniform id in `[0, count)`. Lookup by id: one read.

### 5.2 Titles (TITLES)

Concatenated `varint length + UTF-8` display titles, addressed by
`title_ref`. Used for headers, history and link previews without
decompressing content. Titles are tidy lines (7.3).

### 5.3 Days (DAYS)

For "On this day": 366 `u32` article ids, one per day from January 1 to
December 31 with February 29 (the days of a leap year, in order), or
0xFFFFFFFF where the archive has no article for that day. Outside leap
years, readers skip February 29 (entry 59).

xwa-tool finds each day's article by the title Wikidata gives it in the
archive's Wikipedia ("January 1", "1er janvier", "1. Januar", "1月1日"):
`cargo xtask days` collects the titles of 262 Wikipedias from Wikidata into
a table the converter carries. Archives of other wikis, or of Wikipedia
selections without the day articles, have no DAYS section.

## 6. Title index

### 6.1 Search keys (FOLD)

A title's search key is the title folded one code point at a time, so a
reader folds what the user types exactly as the converter folded the
titles, with a table from the archive rather than Unicode data of its own:

1. ASCII: `A`-`Z` become `a`-`z`; `_`, tab, line feed, vertical tab, form
   feed, carriage return and space become a space; the rest stay.
2. Hangul syllables (U+AC00-D7A3) become their conjoining jamo, as Unicode
   decomposes them: for s = c − 0xAC00, U+1100 + s / 588, then U+1161 +
   (s % 588) / 28, then U+11A7 + s % 28 unless s % 28 is 0.
3. Every other code point is looked up in the FOLD section: found, it
   becomes the fold given there, which may be empty; not found, it stays.

Then each run of spaces becomes one space, and spaces at either end go.
"albert einstein", "Albert_Einstein", "ALBERT EINSTEIN" and "Älbert
Einstein" share a key. Keys are compared as bytes. A key folds to itself,
so typing a title's key always finds it.

xwa-tool works the table out from Unicode's data (through Rust's
unicode-normalization and caseless crates); since the table travels in the
archive, readers never depend on which Unicode version that was. Each code
point is decomposed (NFKD),
stripped, case-folded (full case folding), decomposed and stripped again,
with white space and `_` made spaces, until nothing changes. Stripping
leaves out what accents and other diacritics add to letters (U+0300-036F,
U+1AB0-1AFF, U+1DC0-1DFF, U+20D0-20FF, U+FE20-FE2F), Hebrew points and
cantillation, Arabic vowel signs and Quranic marks, variation selectors,
and invisible format characters (soft hyphen, zero-width spaces and
joiners, direction marks, tags). Marks that are letters stay: Indic vowel
signs, kana voicing marks, Thai tone marks, so `कि` and `क`, `が` and `か`,
`ไก่` and `ไก` have different keys.

FOLD: `u32 count`, `u32 reserved`, then `count` 8-byte records in code
point order, then a string pool. A record is `u32 code_point`, `u8 length`
(of the fold in UTF-8 bytes) and 3 bytes: the fold itself, padded with
zeros, when it is at most 3 bytes long, else the `u24` offset of the fold
in the pool. Records never straddle a 4096-byte block, so the X4 can
binary-search the section a block at a time, and 82% of folds need no pool
read. No record is for ASCII or a Hangul syllable. An archive lists only
the folds its keys can use: those made of spaces and characters that occur
in its keys (a character whose fold has any other cannot match a key,
folded or not), and every empty fold. Measured: 2,710 to 4,404 records
(21.7 to 36.9 KB) in the English archives tried, 5,732 (49.6 KB) in Swedish
Wikipedia's; the whole table is 7,775 records in 68.8 KB.

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
u8     flags           bit0 redirect, bit1 anchor follows, bits2-7 reserved
u8     popularity      its article's popularity (5.1), for ranking (6.5)
u32    anchor          only with bit1: the named place (7.2) a redirect to
                       a section leads to
```

The first entry of each block has `*_shared = 0`. Choosing an entry with
an anchor opens its article at that place, found as for an anchor link
(7.2); when the article has no such place, it opens at the top. Display
titles are tidy lines (7.3).

### 6.3 Leaf blocks (TINDEX_LEAF)

4096-byte blocks: `u16 entry_count`, `u16 used_bytes`, entries, zero
padding, `u32 crc32` in the last 4 bytes. An entry always fits in one block
(MediaWiki titles are at most 255 bytes), but entries with the same key may
continue from one block into the next. Measured: 29.9 bytes per title in
the mathematics archive (103,101 titles, 3.1 MB), 25.6 in chemistry, 25.9
in Swedish Wikipedia's (4.48 M titles, 116 MB) and 13.9 for Simple
Wiktionary's one-word titles; about 0.6 GB for English Wikipedia's ~21.7 M
titles.

### 6.4 Inner levels (TINDEX_INNER)

A static B+tree over the leaf blocks, built bottom-up. The root is inner
block 0; an index of a single leaf block has no inner blocks. An inner
block is `u8 level` (1: its children are leaf blocks; 2 and up: inner
blocks), `u8 reserved`, `u16 child_count`, `u16 used_bytes`, then per
child, in key order:

```
varint sep_shared      bytes shared with the previous child's separator
varint sep_suffix_len, separator suffix bytes
u32    child           leaf block number (level 1), else inner block number
u8     popularity      the highest popularity of any entry under the child
```

then zero padding and `u32 crc32` in the last 4 bytes. The first child has
`sep_shared = 0`.

A child's separator is a byte string `s` with `last < s ≤ first`, where
`first` is the child's first key and `last` the key just before it, or
`s = first` when the two keys are equal (a key continuing from one leaf
block into the next). xwa-tool writes the shortest: `first` up to and
including its first byte that differs from `last`, and an empty separator
for the first leaf block. An inner block's separator in its parent is its
first child's. Measured: 14-15 bytes per child (24-28 with whole keys), so
a full block holds about 280 children. Swedish Wikipedia's 28,344 leaf
blocks sit under 101 inner blocks (414 KB; 162 with whole keys), and
English Wikipedia's ~150 K would need about 540 (2.2 MB): the X4 Pro can
keep every inner block in PSRAM (one SD read per lookup), and the C3 keeps
the root (2-3 reads).

### 6.5 Search

Both searches start from the search key of what was typed (6.1). The keys
that start with a prefix `p` sort from `p` up to, not including, `p` with
its last byte increased by one (UTF-8 has no 0xFF bytes).

**In index order.** Descend from the root: in each inner block, to the last
child whose separator is less than `p`, or to the first child when none
is. In the leaf block, skip keys less than `p`, then collect entries while
their keys start with `p`, going on into the following leaf blocks. (Going
to the last child whose separator is at most `p` would miss entries with
key `p` at the end of the block before.)

**Most popular first.** The `k` most popular articles with an entry whose
key starts with `p`: each article once, by its first such entry in index
order; most popular first, and of equal popularity in index order. A
best-first walk finds them without reading every match. Keep the subtrees
still to open ordered by their highest popularity (highest first), then
their separator (lowest first), starting with the root. Open the next one:
an inner block adds those of its children whose keys can start with `p`
(a child's keys run from its separator up to the next child's separator,
that one included; for the last child of a block, the next separator after
its block in the parent, and none for the last of all); a leaf block offers
its entries that start with `p`.
Stop when `k` articles are kept and the next subtree's popularity is less
than that of the least popular one kept, or equal to it with a separator
past that entry's key. Measured, for the 8 most popular: the median
one-letter prefix reads 9 blocks in the chemistry and mathematics archives
and 15.5 in Swedish Wikipedia's (at most 26), two letters 4-5 and 11 (at
most 21), where a listing in index order reads 3. Most of the extra reads
are leaf blocks holding redirects to articles already kept, which carry
their article's popularity and so cannot be passed over; a reader may stop
early and show what it has.

## 7. Content

### 7.1 Chunks

Each article is one byte stream (7.2). Streams are concatenated in article
id order and cut into chunks of exactly 2^`chunk_bits` bytes uncompressed
(32,768 in xwa-tool's archives, in both profiles), all but the last, which
holds the rest (at least 1 byte); a stream may start partway through one
chunk and continue through the following ones. Small articles share a
chunk; large ones span several. Each chunk is compressed independently:

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
stored_size`, `u24 uncompressed_size` (2^`chunk_bits`, or for the last chunk
1 to 2^`chunk_bits`; a check). The stored bytes are the compressed chunk
followed by a `u32` CRC-32 of the compressed bytes, so corruption is caught
before decompression.

Byte `p` of an article stream (counting its `stream_size` field from 0) is
in chunk `first_chunk + ((offset_in_chunk + p) >> chunk_bits)`, at offset
`(offset_in_chunk + p) & (2^chunk_bits − 1)` in it.

### 7.2 Article stream

At `offset_in_chunk` of `first_chunk`:

```
varint stream_size     bytes that follow this field
string title
varint body_size
varint toc_count, then per heading: u8 level, string text, varint pos
varint table_count, then per table: varint pos
varint infobox_pos_plus_one   (0 = no infobox)
varint anchor_count, then per anchor: u32 hash, varint section
body                   block stream (7.3), body_size bytes
tables                 table records (7.4), one per table
infobox                infobox record (7.4), if any
```

Every `pos` is a byte position measured from the first byte of the body, so
the header never depends on its own size. Jumping to a section or table
decompresses only the chunk that holds that position.

Sections are numbered by their contents entries: 1 is the first, and 0 is
the top of the article.

**Named places.** Links may lead to a named place in another article (the
`#Early_life` of a URL), and so may redirects to a section. A place is
known by its anchor hash: 32-bit FNV-1a (offset basis `0x811C9DC5`, prime
`0x01000193`) over the UTF-8 bytes of its name, with each space byte taken
as `_`. An article's places are:

1. its anchor list, from the header above: each `hash` with the section
   it is in;
2. each contents entry's own anchor, by rule: the entry's text with the
   spaces at either end removed and each run of spaces inside replaced by
   one `_` (each entry on its own: two entries with the same text get the
   same name).

To find a place, look through the anchor list first, then the contents
entries' anchors in order, and take the first match; with none, use the
top. The converter lists only what the rule does not give: headings whose
anchor on Wikipedia differs from the rule's (among them the second and
later of headings with the same text, which Wikipedia names `History_2`,
`History_3`), named places inside sections (template anchors, table rows,
old-style anchors kept for old links), and nothing in the lead, which is
the top anyway. Measured: 91.5% of the 14,639 links to
places in other articles in the chemistry ZIM find their place, and 87% of
55,760 in mathematics; nearly all the rest are links to sections Wikipedia
has since renamed or removed. Listing the anchors of repeated headings
costs about 0.01% of the text (462 in Simple Wiktionary, 1,961 in
Wikivoyage Europe).

### 7.3 Block stream

A pre-order sequence of opcodes. EXT opcodes carry a length so that readers
can skip what they do not know; any other unknown opcode is an error.
Lists and quotes nest at most 64 deep, and at most 64 styles and links are
open at once in an inline run.

Block opcodes:

| Op | Name | Payload |
|---|---|---|
| 0x01 | PARA | inline run |
| 0x02 | HEADING | u8 level, 1-6 as HTML's h1-h6 (sections are 2), inline run |
| 0x03 | LIST_START | u8 kind: 0 bullet, 1 numbered, 2 definition |
| 0x04 | LIST_ITEM | (blocks follow until the next item or the end) |
| 0x05 | LIST_END | |
| 0x06 | QUOTE_START / 0x07 QUOTE_END | |
| 0x08 | PRE | string (preformatted text, monospace, no wrapping) |
| 0x09 | RULE | |
| 0x0A | FIGURE | varint image_id, inline run (caption) |
| 0x0B | MATH_BLOCK | formula (8.1) |
| 0x0C | TABLE_REF | varint table index (into the header's table list) |
| 0x0D | INFOBOX_REF | |
| 0x0E | GALLERY | varint n, n × (varint image_id, inline run) |
| 0x7F | BLOCK_EXT | varint length, payload (skippable) |

An inline run is `varint length` followed by inline opcodes:

| Op | Name | Payload |
|---|---|---|
| 0x01-0x7F | TEXT | short text: op = length (1-127), then UTF-8 bytes |
| 0x80 | TEXT_LONG | varint length, UTF-8 |
| 0x81 | STYLE_PUSH | u8 style: 0 bold, 1 italic, 2 subscript, 3 superscript, 4 monospace, 5 small caps |
| 0x82 | STYLE_POP | |
| 0x83 | LINK_START | varint target article id, varint section index (0 = top) |
| 0x84 | LINK_END | |
| 0x85 | MATH | formula (8.1) |
| 0x86 | ICON | varint image_id, u8 height (an image in a line of text: flags, symbols) |
| 0x87 | BREAK | line break |
| 0x88 | NOTE_REF | varint footnote number (footnotes later) |
| 0x89 | LINK_ANCHOR | varint target article id, u32 anchor hash (7.2); ends with LINK_END |
| 0xFF | INLINE_EXT | varint length, payload (skippable) |

Links are resolved at conversion time: redirects are followed to the final
article id, and links to missing articles are emitted as plain text. The
device never searches to follow a link. A link to a section of its own
article is a LINK_START with the section number; a link to a named place in
another article, or through a redirect to a section, is a LINK_ANCHOR,
whose section the reader finds in the target's header when it opens it
(7.2).

An ICON's `height` is the height to draw it at, in sixteenths of the size
of body text (the size paragraphs are drawn at, whatever the size of the
text around the icon); its width follows from the image's proportions.
xwa-tool takes it from the page: the image's height in CSS pixels over the
page's body text size (16 px in Wikipedia's ZIMs, so a 15 px flag has
height 15).

**Text.** What readers may rely on, which `xwa-tool verify` checks:

- All text is UTF-8 in NFC. A TEXT or TEXT_LONG op is never empty and
  holds whole characters.
- Inline runs hold no control characters (U+0000-001F, U+007F-009F), so
  no tabs or newlines: BREAK is the only line break. The only white space
  is the space (U+0020) and the no-break spaces (U+00A0, U+2007, U+202F),
  which must not be broken at.
- Across a whole run, through styles and links: never two spaces in a
  row, no space at the start or end or next to a BREAK, and no BREAK at
  the start or end or twice in a row.
- Titles, contents entries and display titles in the index are tidy lines:
  the same, with no breaks.
- PRE text separates lines with a line feed (U+000A) alone and holds no
  other control characters (tabs are expanded with spaces to the next
  multiple of 8 characters), and has no empty lines or trailing spaces at
  either end.
- Text is in logical order, as typed. Readers run the Unicode bidirectional
  algorithm on each line they lay out (the firmware has MiniBidi) and shape
  Arabic script themselves; line breaking happens on the device, so the
  converter cannot reorder or shape. Bidi control characters (U+200E,
  U+200F, U+202A-202E, U+2066-2069) are kept.

### 7.4 Tables and infobox

Stored in the article's chunks after the body:

```
TABLE: varint rows, varint cols, u8 flags (reserved), varint cell_count,
       per cell (row-major, by top-left corner):
                 u8 cell_flags (bit0 header), varint rowspan, varint colspan,
                 inline run
       inline run (the caption; empty when the table has none)
INFOBOX: varint row_count, per row: u8 kind, then by kind:
         0 header: inline run
         1 image: varint image_id, inline run (caption)
         2 field: inline run (label), inline run (value)
         3 full width: inline run
```

The caption belongs to the table, not to the text around it, so a reader
that shows tables in a viewer of their own can title them there, and show
the table in the text as a line ("Table: caption") that opens it.

## 8. Math

Formulas are vector drawings: glyph outlines placed by a TeX-quality
layout, so they stay sharp at any size and the firmware can match the text
size, shrink a wide formula to fit, or zoom without re-converting.

xwa-tool lays out each formula's LaTeX source (the `alttext` Wikipedia
keeps next to the MathML) with KaTeX's layout rules, through katex-native
(a Rust port), and KaTeX's fonts. It does not use Wikimedia's math SVGs.
Measured: 99.97% of the mathematics ZIM's 642,219 formulas are drawn, and
90% of the chemistry ZIM's (mhchem's `\ce` is not supported). A formula
that cannot be laid out becomes text in the content stream, with
superscript and subscript styles and italic variables (a² + b² = c²),
which suits chemical formulas well.

Formulas are not images, so archives built without pictures still carry
them. In the mathematics ZIM they add 10.8 MB to the X4 Pro archive (60.8
MB with formulas as text, 71.6 MB drawn); the old `.img` spent 536 MB on
formula images.

### 8.1 Formula record

Used by MATH (inline) and MATH_BLOCK:

```
varint byte_length   of the rest of the record
u8     flags         bit0 display (a block formula), bit1 has fallback image
svarint width, ascent, descent   (MATH_GLYPHS units)
[varint fallback_image_id]       with flag bit1
items, up to the end of the record:
  tag 0 GLYPH     varint glyph_id, u8 scale_code, svarint dx, svarint dy
  tag 1 GLYPH_XY  varint glyph_id, i16 sx, i16 sy (8.8 fixed), svarint dx, svarint dy
  tag 2 RULE      svarint dx, svarint dy, varint w, varint h
  tag 3 PATH      svarint dx, svarint dy, varint command_count, commands:
                    u8 0 MOVE | 1 LINE | 2 QUAD | 3 CUBIC | 4 CLOSE,
                    followed by 1, 1, 2, 3 or 0 points; each point is
                    svarint x, y as a delta from the point written before
                    it (the first from 0, 0)
```

The length lets a reader skip a formula without reading its items: when
paginating, or when it holds an item it does not know, drawing the
fallback image or a placeholder in its box instead. It costs next to nothing:
items run to the end of the record, so no item count is stored (with all
of version 0.3's changes, the mathematics archive's compressed text is
0.16% larger than in 0.2).

Units are MATH_GLYPHS font units (1000 per em in xwa-tool's archives),
with y pointing up. The formula's origin is the left end of its baseline;
its box runs from x = 0 to `width` and from y = −`descent` to `ascent`.
Each item's position is the previous item's position plus (`dx`, `dy`);
the first item's is relative to the origin.

- GLYPH: the glyph's origin at the position, scaled by
  `scales[scale_code]` from MATH_GLYPHS. GLYPH_XY scales by `sx`, `sy`
  instead, for sizes outside the table.
- RULE: a filled rectangle (fraction bar, overline) with its bottom-left
  corner at the position.
- PATH: a filled outline (radical sign, stretchy brace or arrow), nonzero
  winding, with coordinates relative to the position. KaTeX draws such
  shapes hundreds of ems long and clips them to their box; xwa-tool clips
  them before writing, so devices never need to clip.

Measured on the mathematics ZIM: 82 B per formula on average (median 33,
90th percentile 196), 9.8 items, a PATH in 8% of formulas.
`xwa_format::math_svg` in xwa-tool is a reference renderer: it draws a
formula record as SVG.

### 8.2 Glyph outlines (MATH_GLYPHS)

Required whenever an article contains a formula (feature bit 1).

```
u16 upem, u16 glyph_count, u8 scale_count, u16 scales[scale_count] (8.8)
u32 glyph_offset[glyph_count + 1]   (from the start of the glyph data,
                                     which follows this table)
per glyph: i16 xmin, ymin, xmax, ymax, then commands:
  0 MOVE x y | 1 LINE x y | 2 QUAD cx cy x y | 3 CUBIC c1x c1y c2x c2y x y
  4 CLOSE | 5 END          (i16 coordinates, font units, y up)
```

xwa-tool writes `upem` 1000 and the scales 1.0, 0.7 and 0.5 (KaTeX's
text, script and scriptscript sizes, which cover all but 62 of 1.4 million
glyph uses in the mathematics ZIM). It stores only the glyphs the
archive's formulas use, numbered in order of first use: 995 glyphs, 333 KB
for the mathematics ZIM. The outlines come from KaTeX's fonts (SIL Open
Font License 1.1); META `license.math_font` says so.

The firmware rasterises outlines with a small integer rasteriser
(prototype: 2.1 KB RV32 code, ≤ 8 KB RAM, ~3 ms per inline formula on the
C3).

## 9. Images

### 9.1 Image table (IMAGE_TABLE)

Image id = index. Record, 16 bytes: `u48 offset` (within BLOBS), `u24
size`, `u8 encoding`, `u16 width`, `u16 height`, `u8 kind` (0 photo, 1
line art, 2 icon), `u8 flags` (bit 0 HD source, bit 1 the source had
transparency, now flattened onto white).

Each image is stored once, however many articles use it. Articles refer
to images with `Figure`, `Gallery`, infobox image rows and inline `ICON`s;
an icon is a small image meant to sit in a line of text (flags, symbols).

### 9.2 Encodings

- `encoding 1` JPEG: baseline, 8-bit greyscale, decodable by JPEGDEC
  (which also decodes at 1/2, 1/4, 1/8 scale cheaply). For photos.
- `encoding 2` LA2: line art, 4 grey levels. Header `u16 w, u16 h, u8 bpp
  (1 or 2), u8 codec (0 raw deflate, 1 zstd)`, then rows packed MSB-first,
  compressed as one stream. Pixel values are ink levels: 0 is paper
  (white) and the maximum (1 or 3) is black; `bpp` is 1 when the image has
  no greys. Compact archives use deflate with back-references of at most
  4096 bytes, so the C3 decodes with a 4 KB window (measured: within 1-3%
  of a 32 KB window); full archives use zstd.

xwa-tool stores diagrams drawn from vector graphics (`data-file-type=
"drawing"`) and icons as LA2, and photos as JPEG (quality 75 by default).
Bitmaps that are really diagrams (PNG reaction schemes, plots) are found
by their pixels: when at least 90% sit within 8 of an ink level they are
stored as LA2 too. On the chemistry ZIM that makes the pictures 24%
smaller than JPEG and keeps lines and labels crisp; shaded renderings and
photos score lower and stay JPEG, which the device can dither.

### 9.3 Resolution policy

One stored rendition per image, at the larger of what the zoom viewer can
use and what the inline "enlarged" setting needs, capped at 800 px on the
long side (landscape width) and at the source resolution (or the HD
download, if fetched). ZIM thumbnails are small (chemistry ZIM: 110, 250
and 507 px wide at the 10th, 50th and 90th percentiles), so most are
stored at their source size. The firmware downsamples for inline display, so the
diagram-size setting and the zoom viewer need no extra data. 1-bit display
is derived from LA2 by threshold or patterns; 4-grey uses the levels as
stored.

HD downloads (optional, when converting): for diagrams drawn from SVG,
xwa-tool fetches Wikimedia's rendering at the smallest standard thumbnail
width that gives at least 800 px on the long side, and stores it in place
of the ZIM's picture with flag bit 0 set; the archive then has optional
feature bit 3.

## 10. Fallback font (FONT)

Outline glyphs for the characters an archive's text uses that the
firmware's built-in reader fonts cannot draw, in the MATH_GLYPHS outline
encoding (8.2), so the rasteriser that draws formulas draws them too, at
any size and with anti-aliasing. A reader draws a character from FONT when
its own font lacks it, so a reader font from the SD card with more
coverage is still used where it has the glyph.

```
u16 upem, i16 ascender, i16 descender, u16 line_gap
u32 range_count, then per range (sorted, not overlapping):
    u32 first_code_point, u32 last_code_point, u32 first_glyph
    (glyph = first_glyph + code point - first_code_point)
u32 glyph_count
u16 advance[glyph_count]            (font units)
u32 glyph_offset[glyph_count + 1]   (from the start of the glyph data)
glyph data: per glyph i16 xmin, ymin, xmax, ymax, then MOVE, LINE, QUAD,
            CUBIC and CLOSE commands and END, as MATH_GLYPHS (8.2)
```

xwa-tool writes `upem` 1000, ascender 1069, descender −293 and line gap 0
(Noto Sans's), scales every glyph to 1000 units per em, and numbers the
glyphs in code point order, one range per run of consecutive code points.
A glyph is drawn at the size of the text around it; one that draws nothing
but takes space (U+3000) has an empty outline. There is no hinting, and
regular weight only: bold and italic text uses the same glyphs.

**What it holds.** Every code point the text uses (titles, contents
entries, body, captions, tables, the infobox, preformatted text, display
titles in the title index, and META's title and description), except the
firmware's built-in coverage below and characters that draw nothing
(control characters and Unicode's default-ignorable code points: joiners,
direction marks, variation selectors, tags). For the Arabic letters among
them it adds their presentation forms (U+FB50-FDFF and U+FE70-FEFF that
decompose to the letter) and the Lam-Alef ligatures (U+FEF5-FEFC), which
the firmware's shaper turns Arabic into (7.3). Glyphs are per code point:
combining marks are drawn on their base by the reader, and scripts that
need complex shaping (Indic conjuncts, Khmer, Myanmar) are drawn letter by
letter.

The built-in coverage, the code points both of the firmware's reader
fonts (Bookerly and Noto Sans, 10-18 pt) draw:

```
0000, 000D, 0020-007E, 00A0-017F, 01A0-01A1, 01AF-01B0, 01C4-01F5,
01F8-021F, 0300-0304, 0306-030C, 030F, 0311-0315, 031B, 0323-0329,
032D-032E, 0330-0331, 0342, 0344-0345, 0400-045F, 0462-0463, 0472-0475,
048A-04FF, 1EA0-1EF9, 2000-2030, 2032-203C, 203E, 2043-2044,
2047-2049, 2053, 2057, 205F-2064, 2070, 2074-2079, 207F-2089,
20A3-20A4, 20A7, 20AC, 2212, FB00-FB06, FFFD
```

**Where the glyphs come from.** The Noto fonts (SIL Open Font License
1.1; META `license.font` says so), as Google Fonts serves them, looked in
in this order: Noto Sans (Latin, Greek, Cyrillic, IPA, Devanagari), each
script's own Noto font (Arabic, Hebrew, Armenian, Georgian, the Indic and
Southeast Asian scripts, Ethiopic, and 30 more), Noto Sans Math, Noto Sans
Symbols and Symbols 2, the CJK fonts, and Noto Emoji (monochrome). The CJK
fonts go in the order the archive's language wants: Japanese takes JP
first, Korean KR, Cantonese HK, and everything else SC, then TC, HK, JP
and KR; a character in a CJK block (CJK punctuation, kana, Hangul, Han,
full-width forms) looks in them before any other. `cargo xtask fonts`
pins each font's file, checksum and coverage; the converter downloads only
the fonts an archive needs, the first time, and keeps them. Optional
feature bit 0 is set when FONT is present.

Measured: 2 glyphs for the Ray Charles selection, 457 (135 KB) for Simple
Wiktionary, 2,599 (0.9 MB, 0.55% of the archive) for chemistry, 6,816 (2.2
MB, 3%) for mathematics and 10,157 (4.8 MB, 0.56%) for Swedish Wikipedia's
2.6 million articles. A Han character takes about 590 bytes, most other
glyphs 200-300. What no Noto font draws is rare: private-use and
unassigned code points in the source text, some of CJK extension B, and
symbols added in Unicode 15 and 16 (`xwa-tool verify` lists them).

## 11. Device I/O budget (expected)

| Operation | Reads |
|---|---|
| Search keystroke, index order | 1-3 (index) + 1 per extra leaf block |
| Search keystroke, most popular first | 4-16 median for 1-2 letters, 8 results (6.5) |
| Fold a typed character | 0 (ASCII, Hangul), else 1 FOLD block (cached) |
| On this day | 1 (DAYS) + opening the article |
| Open article | 1 (article record) + 1 (chunk table) + 1 (chunk) |
| Follow link | same as open article; no search |
| Jump to section/table | 1-2 (chunk table, chunk) |
| Show image | 1 (image record, cached per page) + 1 (data) |
| Draw formula | 0 (inline in content) + glyph outlines (cached per page) |

The firmware should map the archive file's extents at open time so raw
sector reads work without the "copy contiguously" step.

## 12. Open questions

1. Footnotes/references: include as NOTE_REF with a per-article notes block?
2. Compact-profile deflate window: 32 KB (as today) or smaller.
3. FAT32 cards: archives over 4 GB need exFAT; split archives are out of
   scope for v0.
4. A title word index (WORD_INDEX), to find "Albert Einstein" by typing
   "einstein".
