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
  headings, lists, tables, infoboxes, links resolved to articles,
  pictures (photos as greyscale JPEG; diagrams, line-art bitmaps and icons
  as lossless 4-grey line art, each picture stored once), and formulas
  drawn as vector math (KaTeX's layout of the LaTeX in the ZIM, with
  KaTeX's fonts; formulas it cannot lay out, such as mhchem's `\ce`, are
  written as text with real superscripts and subscripts). Navigation boxes,
  citation markers, reference lists and external-link lists are left out.
  Redirects, including mwoffliner's redirects to a section, become
  searchable alternative titles. Links to a section of another article,
  and redirects to a section (from links or from search), open the article
  at that section.
- `xwa-fetch`: the Kiwix library catalog, downloads (resumed after an
  interruption, mirrors tried in turn, checked against their SHA-256), and
  copying archives to SD cards (into the card's `wiki` folder, where the
  e-reader looks for them, with FAT32's 4 GB limit checked first).
- `xwa-tool`: command line with `catalog`, `download`, `convert`, `copy`,
  `info`, `verify`, `dump` and `title-index`.
- `xwa-tool-gui`: a desktop app. "Get Wikipedia" lists the Kiwix library
  by language and downloads a ZIM; "Convert" takes it (or any ZIM, also by
  drag and drop), converts it for your e-reader, and copies the result to
  an SD card.

## Building

```
cargo build --release
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim                 # for the X4 Pro
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim --device x4     # for the X4 or X3
./target/release/xwa-tool convert wikipedia_en_100_2026-08.zim --no-images     # text only
./target/release/xwa-tool-gui
```

`--jpeg-quality` (default 75) trades photo quality for size, and
`--math-as-text` writes formulas as text instead of drawing them.

`--hd-diagrams` (in the app: "Download sharper diagrams from Wikipedia")
replaces the ZIM's small pictures of SVG diagrams with sharper renderings,
800 px on the long side, from Wikimedia's thumbnail service. It needs the
internet and takes longer: requests go a few at a time, at Wikimedia's
standard thumbnail widths, and wait whenever Wikimedia asks. Diagrams it
cannot get keep the ZIM's picture. Downloads are kept in `xwa-tool/diagrams`
in the system's cache folder (or `--hd-cache DIR`), so converting again
downloads nothing; delete that folder to free the space.

Measured on English Wikipedia ZIMs (4 cores), with pictures and drawn
formulas:

| ZIM | Articles | Pictures | Formulas drawn | ZIM size | X4 Pro archive | X4/X3 archive | Time (X4 Pro) |
|---|---|---|---|---|---|---|---|
| Top 100 | 101 | 3,098 | 5 of 8 | 333 MB | 39.7 MB | 40.1 MB | 6 s |
| Chemistry | 9,313 | 28,649 | 19,687 of 21,776 | 384 MB | 164.4 MB | 172.4 MB | 48 s |
| Mathematics | 23,255 | 42,673 | 646,177 of 646,347 | 996 MB | 256.3 MB | 274.7 MB | 94 s |

Without pictures the same archives are 2.2, 27.2 and 71.6 MB for the X4
Pro. `xwa-tool dump wiki.xwa --title "Pythagorean theorem" --formulas
page.html` draws an article's formulas into a web page, for checking.

Memory use stays small whatever the size of the Wikipedia: titles are
sorted on disk in batches, and the converter keeps 12 bytes per page.
Swedish Wikipedia (the 4.0 GB "mini" ZIM: 5.0 million entries, 2.6 million
articles) converts in 9 min 39 s with at most 335 MB of memory, where it
took 10 min 48 s and 1,430 MB before. A title index the size of English
Wikipedia's (18.5 million titles) builds in 83 MB instead of 3.2 GB.
Temporary files go next to the archive and disappear once it is written.

Getting ZIMs and putting archives on the card:

```
xwa-tool catalog                            # English Wikipedia ZIMs in the Kiwix library
xwa-tool catalog --lang fra --search medecine
xwa-tool catalog --languages
xwa-tool download wikipedia_en_ray-charles --flavour maxi --dir ~/Downloads
xwa-tool copy wiki.xwa                      # to the SD card's wiki folder, if one card is inserted
xwa-tool copy wiki.xwa --to /media/me/SDCARD  # into /media/me/SDCARD/wiki
```

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
to a GitHub release, each zip with a `THIRD-PARTY-LICENSES.txt` that lists
every crate in the programs with its licence text. `cargo xtask licenses`
writes the same file locally.

## License

Public domain, via the [Unlicense](LICENSE). Third-party dependencies keep
their own licenses (see `THIRD-PARTY-LICENSES.txt` in each release); xwa-tool
embeds KaTeX's fonts, which are under the SIL Open Font License (see
[`licenses/`](licenses/)).
