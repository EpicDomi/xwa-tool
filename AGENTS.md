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
  resume, SHA-256), removable drives and copying to them.
- `crates/xwa-cli`: the `xwa-tool` command line.
- `crates/xwa-gui`: the `xwa-tool-gui` desktop app (iced).
- `xtask`: repository chores; `cargo xtask licenses` writes the third-party
  notice that ships in every release zip.
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
- New dependencies must be under a licence `xtask/src/main.rs` accepts (no
  copyleft); CI runs `cargo xtask licenses` to check.
- Do not commit, tag, or push unless the user asks.
