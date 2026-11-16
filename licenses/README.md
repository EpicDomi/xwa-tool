# Third-party notices

Every release zip has a `THIRD-PARTY-LICENSES.txt`: each Rust crate built
into the programs, with its licence and source repository, then the licence
texts. `cargo xtask licenses` writes it (to `target/` by default). It is
for information only: it never limits which crates xwa-tool may use.

xwa-tool embeds KaTeX's fonts (through the katex-native crate) to draw
formulas. They are under the SIL Open Font License 1.1; see
`KaTeX-fonts-OFL.txt`. Archives written by xwa-tool contain the outlines of
the glyphs their formulas use; the OFL does not cover documents made with
the fonts.

The app draws its screen in the e-reader firmware's fonts, Ubuntu (Ubuntu
Font Licence 1.0, `Ubuntu-font-UFL.txt`) and Noto Sans (SIL Open Font
License 1.1, `NotoSans-OFL.txt`), with icons from the CPR-vCodex firmware
and Wikipedia's globe from Kiwix's Wikipedia files.

This software is based in part on the work of the Independent JPEG Group
(through the jpeg-encoder crate).
