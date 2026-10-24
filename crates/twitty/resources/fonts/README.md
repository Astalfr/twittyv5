# Bundled fonts

## Twemoji.Mozilla.ttf

Twitter's emoji artwork (Twemoji) as a COLRv0 font, used for emoji in
tweets, names and bios (`ui::emoji`).

- **Source**: [mozilla/twemoji-colr](https://github.com/mozilla/twemoji-colr)
  release v0.7.0, `Twemoji.Mozilla.ttf`
  (sha256 `6d90152ee0d29e82fe2a87793af5aa4b7ad13e6538360889e141e81ed299ee8e`).
  It covers emoji up to Unicode 14; newer ones fall back to Segoe UI Emoji.
- **Modified**: `fill_base_glyphs.py` gives every colour glyph an outline (a
  composite of its layers), because GPUI never draws a glyph whose own
  outline is empty. The colours are unchanged. The bundled file's sha256 is
  `dffdab0c273edce60b2519b8e3695ebbfcd7b3e1b9c8b68eddfb7be2040ffb4a`.
- **Licence**: the artwork is © Twitter, Inc and other contributors,
  [CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/); the font build is
  Apache 2.0 (Mozilla).
