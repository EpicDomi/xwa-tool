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
- `xwa-tool`: command line with `info`, `verify`, `dump` and `title-index`.
- `xwa-tool-gui`: a desktop window that opens a ZIM and shows what it
  contains.

Conversion from ZIM (reading Wikipedia's HTML, images and math into the
archive) is the next step and is not implemented yet.

## Building

```
cargo build --release
./target/release/xwa-tool info wikipedia_en_100_2026-08.zim
./target/release/xwa-tool title-index wiki.zim --query "einstein"
./target/release/xwa-tool-gui
```

For XWA files:

```
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
