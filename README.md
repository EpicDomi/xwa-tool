# xwa-tool

Fetch and convert ZIM archives to XWA format for XTEINK devices.

xwa-tool turns Kiwix ZIM archives (Wikipedia first) into XWA archives (`.xwa`
files), a format built for Xteink e-readers running the CPR-vCodex wiki
firmware: the X4 Pro first, the X4 and X3 as far as their memory allows. The
format is described in [`docs/format-spec-v0.md`](docs/format-spec-v0.md).

## Status

Early. What works today:

- `xwa-zim`: a ZIM reader (header, metadata, directory scans and path lookups,
  zstd and XZ clusters, the title listing).
- `xwa-format`: the XWA format, draft v0: the article content encoding, the
  title index (case- and accent-insensitive prefix search), and an archive
  writer, reader and verifier. The writer compresses on every CPU core and
  produces the same bytes for the same input.
- `xwa-convert`: ZIM to XWA conversion of Wikipedia: paragraphs,
  headings, lists, tables, infoboxes, links resolved to articles, and
  pictures (photos as greyscale JPEG; diagrams, line-art bitmaps and icons
  as lossless 4-grey line art, each picture stored once). Navigation boxes,
  citation markers, reference lists and external-link lists are left out.
  Redirects, including mwoffliner's redirects to a section, become
  searchable alternative titles.
- `xwa-tool`: command line with `convert`, `info`, `verify`, `dump` and
  `title-index`.
- `xwa-tool-gui`: a desktop window: choose a ZIM (or drop one on it), pick
  your e-reader, convert.

Not yet: formulas drawn as math (for now they appear as text, such as
a² + b² = c²). That is the next step.

## Building

```
cargo build --release
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim                 # for the X4 Pro
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim --device x4     # for the X4 or X3
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim --no-images     # text only
./target/release/xwa-tool-gui
```

`--jpeg-quality` (default 75) trades photo quality for size.

Measured on English Wikipedia ZIMs (4 cores), with pictures:

| ZIM | Articles | Pictures | ZIM size | X4 Pro archive | X4/X3 archive | Time (X4 Pro) |
|---|---|---|---|---|---|---|
| Top 100 | 101 | 3,098 | 333 MB | 39.7 MB | 40.1 MB | 5 s |
| Chemistry | 9,313 | 28,649 | 384 MB | 163.6 MB | 171.6 MB | 39 s |
| Mathematics | 23,255 | 42,673 | 996 MB | 245.3 MB | 260.5 MB | 68 s |

Without pictures the same archives are 2.2, 26.4 and 60.8 MB for the X4 Pro.

For ZIM and XWA files:

```
xwa-tool info wiki.zim                      # what a ZIM contains
xwa-tool title-index wiki.zim --query "einstein"
xwa-tool info wiki.xwa                      # format, features, sections, metadata
xwa-tool verify wiki.xwa                    # check every checksum, article, link and image
xwa-tool dump wiki.xwa --title "Albert Einstein"
xwa-tool dump wiki.xwa --search "einst"
xwa-tool dump wiki.xwa --image 12 --out diagram.pgm
```

Pushing a `v*` tag builds Windows, macOS and Linux binaries and attaches them
to a GitHub release.

## License

Public domain, via the [Unlicense](LICENSE). Third-party dependencies keep
their own licenses.
