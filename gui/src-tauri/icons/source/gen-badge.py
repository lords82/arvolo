#!/usr/bin/env python3
"""Generate the counter-badge icon variants.

The badge is a circle knocked *out* of the mark — a real hole, not a disc drawn
on top — with the count sitting in it. Because the hole is transparent, the same
file reads on a light and a dark ground, which is what lets one asset serve every
platform.

Three families come out of here, into ../badge:

  tray-N.png     36x36, black on alpha — the macOS menu bar template. The mark
                 stays full bleed and the circle bites it: the wedge loses its
                 right corner and the box's right arm is shortened. That is the
                 chosen look, not an accident.
  app-N.png      64x64, full-bleed colour — the Windows and Linux tray. The badge
                 sits top-right where it only removes orange, so the glyph is
                 untouched.
  overlay.png    32x32 — the Windows taskbar overlay slot, which is far too small
                 for a digit, so it carries a plain disc.

N is 1..9 plus `9plus`. Digits are baked as outlines pulled from Helvetica Bold,
so regenerating does not depend on what fonts a machine happens to have; only
this script does, and only when the geometry changes.

Needs `rsvg-convert` (brew install librsvg) and `fonttools` (pip install fonttools).

    python3 gen-badge.py
"""
import pathlib
import subprocess
import sys

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTCollection

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "badge"
FONT_PATH = "/System/Library/Fonts/Helvetica.ttc"
FONT_STYLE = "Bold"

ORANGE, CREAM = "#F97316", "#faf8f6"
# The mark, in its own 100-unit box — identical to the other three sources here.
GLYPH = ('<path d="M18 46 V64 a16 16 0 0 0 16 16 h32 a16 16 0 0 0 16 -16 V46" '
         'fill="none" stroke="{ink}" stroke-width="12" stroke-linecap="round"/>'
         '<path d="M50 40 L38 12 H62 Z" fill="{ink}"/>')


def load_font():
    """Helvetica Bold out of the system collection."""
    try:
        coll = TTCollection(FONT_PATH)
    except Exception as e:
        sys.exit(f"{FONT_PATH} non leggibile ({e}). Serve per estrarre le cifre.")
    for f in coll.fonts:
        names = {n.toUnicode() for n in f["name"].names if n.nameID == 2}
        if FONT_STYLE in names:
            return f
    sys.exit(f"stile {FONT_STYLE} assente in {FONT_PATH}")


FONT = load_font()
UPM = FONT["head"].unitsPerEm
CMAP = FONT.getBestCmap()
GSET = FONT.getGlyphSet()


def text_path(s, cx, cy, height, max_width, ink):
    """Outlines for `s`, scaled to `height` (capped by `max_width`), centred on
    (cx, cy). Font space is y-up, so the group flips y."""
    pen = SVGPathPen(GSET)
    bounds = BoundsPen(GSET)
    x = 0
    for ch in s:
        glyph = GSET[CMAP[ord(ch)]]
        shift = (1, 0, 0, 1, x, 0)
        glyph.draw(TransformPen(pen, shift))
        glyph.draw(TransformPen(bounds, shift))
        x += glyph.width
    if bounds.bounds is None:
        return ""
    x0, y0, x1, y1 = bounds.bounds
    s_h = height / (y1 - y0)
    s_w = max_width / (x1 - x0)
    sc = min(s_h, s_w)
    tx = cx - sc * (x0 + x1) / 2
    ty = cy + sc * (y0 + y1) / 2
    return (f'<g transform="translate({tx:.3f} {ty:.3f}) scale({sc:.5f} {-sc:.5f})">'
            f'<path d="{pen.getCommands()}" fill="{ink}"/></g>')


def svg(label, *, ink, bg, glyph_scale, glyph_xy, badge, digit_ink):
    """One badged icon. `badge` is (cx, cy, r); `label` empty means no badge."""
    cx, cy, r = badge
    hole = f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="#000"/>' if label else ""
    gx, gy = glyph_xy
    body = (f'<rect width="100" height="100" rx="20" ry="20" fill="{bg}"/>' if bg else "")
    body += (f'<g transform="translate({gx} {gy}) scale({glyph_scale})">'
             f'{GLYPH.format(ink=ink)}</g>')
    mark = ""
    if label == ".":
        mark = f'<circle cx="{cx}" cy="{cy}" r="{r * 0.52:.2f}" fill="{digit_ink}"/>'
    elif label:
        mark = text_path(label, cx, cy, r * 1.15, r * 1.5, digit_ink)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <defs><mask id="k">
    <rect x="-20" y="-20" width="140" height="140" fill="#fff"/>{hole}
  </mask></defs>
  <g mask="url(#k)">{body}</g>{mark}
</svg>'''


def render(svg_text, name, px):
    tmp = OUT / f".{name}.svg"
    tmp.write_text(svg_text)
    subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), str(tmp),
                    "-o", str(OUT / f"{name}.png")], check=True)
    tmp.unlink()


# The mark sits full bleed on the menu-bar template and the circle bites it; on
# the colour icon it is inset 19.5/100, which is why the same top-right badge
# clears the glyph there and only removes orange.
TRAY = dict(ink="#000000", bg=None, glyph_scale=0.966,
            glyph_xy=(1.5, 0.5), badge=(74, 24, 21), digit_ink="#000000")
APP = dict(ink=CREAM, bg=ORANGE, glyph_scale=0.609,
           glyph_xy=(19.5, 19.5), badge=(78.1, 21.9, 19.5), digit_ink=ORANGE)


def main():
    OUT.mkdir(exist_ok=True)
    labels = [str(n) for n in range(1, 10)] + ["9+"]
    for label in labels:
        name = "9plus" if label == "9+" else label
        render(svg(label, **TRAY), f"tray-{name}", 36)
        render(svg(label, **APP), f"app-{name}", 64)
    # The taskbar overlay slot is 16pt: a digit cannot survive it, so it gets the
    # disc. Drawn on its own, not knocked out of anything.
    render(f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <circle cx="50" cy="50" r="46" fill="{ORANGE}" stroke="{CREAM}" stroke-width="8"/>
</svg>''', "overlay", 32)
    print(f"scritti {len(labels) * 2 + 1} file in {OUT}")


if __name__ == "__main__":
    main()
