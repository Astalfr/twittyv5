"""Gives every COLR base glyph of Twemoji Mozilla an outline: a composite of
its colour layers.

The upstream font leaves base glyphs empty (only the layers have outlines).
GPUI sizes a glyph from its base outline, so an empty one is never drawn.
DirectWrite still paints the layers in colour; the composite only sets the
bounds (and is the monochrome fallback).

    pip install fonttools
    python fill_base_glyphs.py Twemoji.Mozilla.upstream.ttf Twemoji.Mozilla.ttf
"""

import sys

from fontTools.ttLib import TTFont
from fontTools.ttLib.tables._g_l_y_f import GlyphComponent, Glyph


def main(src: str, dst: str) -> None:
    font = TTFont(src)
    glyf = font["glyf"]
    filled = 0
    for base, layers in font["COLR"].ColorLayers.items():
        glyph = glyf[base]
        if glyph.numberOfContours != 0:
            continue
        components = []
        for layer in layers:
            component = GlyphComponent()
            component.glyphName = layer.name
            component.x, component.y = 0, 0
            component.flags = 0
            components.append(component)
        composite = Glyph()
        composite.numberOfContours = -1
        composite.components = components
        glyf[base] = composite
        filled += 1
    font.save(dst)
    print(f"filled {filled} base glyphs")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
