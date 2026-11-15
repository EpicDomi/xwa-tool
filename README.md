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
- `xwa-convert`: ZIM to XWA conversion of Wikipedia text: paragraphs,
  headings, lists, tables, infoboxes, and links resolved to articles.
  Navigation boxes, citation markers, reference lists and external-link
  lists are left out. Redirects, including mwoffliner's redirects to a
  section, become searchable alternative titles.
- `xwa-tool`: command line with `convert`, `info`, `verify`, `dump` and
  `title-index`.
- `xwa-tool-gui`: a desktop window: choose a ZIM (or drop one on it), pick
  your e-reader, convert.

Not yet: pictures, and formulas drawn as math (for now they appear as text,
such as a² + b² = c²). Both are the next steps.

## Building

```
cargo build --release
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim                # for the X4 Pro
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim --device x4    # for the X4 or X3
./target/release/xwa-tool-gui
```

Measured on four English Wikipedia ZIMs (4 cores):

| ZIM | Articles | ZIM size | X4 Pro archive | X4/X3 archive | Time |
|---|---|---|---|---|---|
| Top 100 | 101 | 333 MB | 2.2 MB | 2.5 MB | 1.2 s |
| Chemistry | 9,313 | 384 MB | 26.4 MB | 31.6 MB | 12 s |
| Mathematics | 23,255 | 996 MB | 60.8 MB | 72.1 MB | 30 s |

Most of a ZIM's size is pictures, which these archives do not include yet.

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
