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
- `xwa-format`: the XWA format, draft v0. So far: the title index
  (case- and accent-insensitive prefix search).
- `xwa-tool`: command line with `inspect` and `title-index`.
- `xwa-tool-gui`: a desktop window that opens a ZIM and shows what it
  contains. Conversion is not implemented yet.

## Building

```
cargo build --release
./target/release/xwa-tool inspect wikipedia_en_100_2026-08.zim
./target/release/xwa-tool title-index wiki.zim --query "einstein"
./target/release/xwa-tool-gui
```

Pushing a `v*` tag builds Windows, macOS and Linux binaries and attaches them
to a GitHub release.

## License

Public domain, via the [Unlicense](LICENSE). Third-party dependencies keep
their own licenses.
