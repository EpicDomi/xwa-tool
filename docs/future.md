# Possible future work

Ideas set aside for now, with what is known about them, to come back to
if they become worth it.

## Rare CJK ideographs in the fallback font

The Noto fonts cover only about 2,300 of the 70,000 CJK ideographs of
Unicode's extensions B to I. English Wikipedia's text uses 72,103 of them
(`xwa-tool verify` counts them), almost all in the code charts of its
articles on Unicode's blocks, which list every code point; a few appear in
real text (dictionary radicals, Chinese and Cantonese names, chữ Nôm). In
Chinese, Japanese or Cantonese wikis they appear in real text (names,
places, classical texts), so they matter more there.

Covering them would mean an optional extra fallback source used only for
what Noto lacks: Plangothic (SIL Open Font License), which is built on the
same design as Noto's CJK fonts. Jigmo covers them too, but in a serif
style that would not match.

What it would cost:

- About 800 bytes a glyph in FONT (measured: 750-795 for CJK glyphs), so
  about 58 MB for English Wikipedia (0.07% of its 87.5 GB archive);
  archives that do not use them pay nothing.
- A second font source outside Google Fonts for `cargo xtask fonts`, and
  `license.font`, the spec and the README updated.
- Its files downloaded when an archive needs them (a few tens of MB, not
  yet checked).
- FONT growing from about 80,000 to about 150,000 glyphs (about 0.4 MB
  more of tables, 6 bytes a glyph): the firmware should confirm it is
  happy with that.

Not covered either way: symbols and scripts added in Unicode 16 and 17 that
no font has yet, and code points no font draws (unassigned, private use).
