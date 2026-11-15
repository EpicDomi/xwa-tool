# Third-party notices

Every release zip has a `THIRD-PARTY-LICENSES.txt`: each Rust crate built
into the programs, with its licence and source repository, then the licence
texts. `cargo xtask licenses` writes it (to `target/` by default); CI checks
that every dependency is under an accepted licence.

xwa-tool embeds KaTeX's fonts (through the katex-native crate) to draw
formulas. They are under the SIL Open Font License 1.1; see
`KaTeX-fonts-OFL.txt`. Archives written by xwa-tool contain the outlines of
the glyphs their formulas use; the OFL does not cover documents made with
the fonts.

This software is based in part on the work of the Independent JPEG Group
(through the jpeg-encoder crate).
