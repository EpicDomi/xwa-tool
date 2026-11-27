# xwa-tool agent guide

xwa-tool converts Kiwix ZIM archives into XWA archives for Xteink e-readers
running the CPR-vCodex wiki firmware (`EpicDomi/cpr-vcodex-wiki-x4pro`). The
X4 Pro (ESP32-S3, 8 MB PSRAM) is the primary target; the X4/X3 (ESP32-C3,
about 380 KB RAM, no PSRAM) are supported through the compact profile.

## Layout

- `crates/xwa-zim`: ZIM reader.
- `crates/xwa-format`: XWA format reference implementation: content
  encoding, title index, archive writer, reader and verifier.
- `crates/xwa-convert`: ZIM to XWA conversion (HTML extraction, link
  resolution, the parallel pipeline), shared by the CLI and the GUI.
- `crates/xwa-fetch`: Kiwix library catalog (OPDS), downloads (Metalink,
  resume, SHA-256), removable drives and copying to them, and sharper
  diagrams from Wikimedia (`wikimedia.rs`).
- `crates/xwa-flash`: installs firmware on the e-reader over USB
  (espflash): firmware image checks (`image.rs`), the Xteink partition
  layouts and OTA data (`layout.rs`), GitHub releases (`release.rs`), and
  the reader itself (`device.rs`). Only the app slot not in use and then
  the OTA data are ever written.
- `crates/xwa-cli`: the `xwa-tool` command line.
- `crates/xwa-gui`: the `xwa-tool-gui` desktop app (iced), drawn as the
  e-reader: `device.rs` (the cases and keys of each model, and whether the
  window is the device's shape), `eink.rs` (the screen: the firmware's
  Lyra theme sizes, fonts and widgets), `screens.rs` (what each screen
  shows), `app.rs` (state and messages), `library.rs` (the wikis, filters
  and sorts of the library browser), `downloads.rs` (the download queue),
  `graphics.rs` (which renderer draws the window, and starting again
  another way when one fails), `crash.rs` (the crash report and dialog),
  `prefs.rs` (remembered choices), `read/` (reading an archive in the app:
  `layout.rs` lays articles out in pages, as the firmware does, and
  `draw.rs` draws them on a canvas). `XWA_GUI_SCREENS=DIR cargo test -p
  xwa-gui -- --ignored screens` draws every screen of every model into
  `DIR` (`XWA_GUI_ZIM` adds a real ZIM, `XWA_GUI_XWA` an archive to read
  with `XWA_GUI_READ=Title:0,3;Other:1` choosing articles and pages,
  `XWA_GUI_ONLY` picks screens by name).
- `vendor/iced_wgpu`, `vendor/iced_winit`: iced's wgpu renderer and window
  shell with small patches, used through `[patch.crates-io]`: a transparent
  window on Windows, resizing that works on Wayland (KDE), and windows that
  open centred. `vendor/README.md` says what each is; re-apply them when
  iced is upgraded, and change nothing else there.
- `xtask`: repository chores; `cargo xtask licenses` writes the third-party
  notice that ships in every release zip, and `cargo xtask days` writes
  `crates/xwa-convert/data/days.tsv.zst`, each Wikipedia's day articles
  from Wikidata, for the DAYS section; `cargo xtask fonts` writes
  `crates/xwa-convert/data/fonts.tsv.zst`, the Noto fonts on Google Fonts
  (each style's address, SHA-256, coverage) that the fallback FONT and
  FONT_BOLD are drawn from
  (`xwa-convert/src/fallback.rs`; `xwa-fetch/src/fonts.rs` downloads and
  keeps them); `cargo xtask fonts --rare` writes `fonts-rare.tsv.zst` beside
  it, the optional fonts for rare characters (other Noto designs,
  Plangothic's latest release, NewGardiner at its newest commit, GNU
  Unifont), each listed only for what it adds, with the mean size of those
  glyphs for cost estimates. It needs api.github.com and github.com (and
  `git`), and fails, after writing what it could, when a source can't be
  had. `cargo xtask unicode` follows the newest Unicode version.
- `docs/format-spec-v0.md`: the format specification.

## Rules

- The spec is the contract with the firmware. Change the spec and the
  `xwa-format` code together, and never let the X4/X3 limits of the compact
  profile leak into the X4 Pro's full profile.
- Back size and speed claims with measurements on real ZIMs.
- Keep `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` passing; CI runs all three on Windows, macOS
  and Linux.
- The GUI targets non-technical users: plain wording, no jargon.
- The app's screen copies the firmware's Lyra theme (CPR-vCodex
  `src/components/themes/lyra/`): keep its sizes, fonts and icons in step
  with the firmware rather than inventing new ones.
- Do not test sharper diagrams against Wikimedia from CI or shared servers:
  point `XWA_TOOL_UPLOADS` at a local stand-in server instead. Likewise,
  test the fallback font with stand-in fonts (KaTeX's) and a prepared
  cache, not against Google Fonts.
- Do not commit, tag, or push unless the user asks.
