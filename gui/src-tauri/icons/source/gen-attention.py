#!/usr/bin/env python3
"""Generate the attention states for the tray icon.

There is no counter and no badge. The mark says it itself: the small V is the
file in flight, so while something is waiting for the user to decide it
**breathes** — drifting down toward the big V and back up, over and over, until
the last request has been approved or refused. At rest it sits still at the top.

That leaves the mark whole, which a badge never managed — at 36px a corner circle
either shrinks to an invisible sliver or eats one of the arms until the glyph
reads as a check mark. Moving a piece that is already there costs nothing.

Frames go into ../attention:

  tray-N.png    36x36, black on alpha — the macOS menu bar template.
  app-N.png     64x64, full-bleed colour — the Windows and Linux tray.

Frame 0 is the resting position and the sixteen frames are one full breath: 0
through 8 drift down, 9 through 15 come back up, and the sequence loops. Frame 8
is the bottom of the travel.

**These positions are not a linear ruler**, which is the one thing that changed
with the mark. The old wedge fell once and stopped, so the frames could be evenly
spaced and the shape of the fall could live in `attention.rs` as a schedule of
(frame, hold) pairs. An oscillation has no such split: its shape *is* a sine, and
the only thing anyone would want to retune is how long a breath takes — which is
still a single constant over there. So the sine is baked in here and every frame
is held for exactly the same time.

Needs `rsvg-convert` (brew install librsvg).

    python3 gen-attention.py
"""
import math
import pathlib
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "attention"

ORANGE, CREAM = "#F97316", "#faf8f6"
FRAMES = 16
# How far the small V drifts, in the mark's own 100-unit box. Deliberately short
# of the ~31 units that would nest it inside the big V: the breath runs for as
# long as something is pending, so it has to be noticeable out of the corner of
# the eye without turning the menu bar into a place where things move.
#
# Read this in points, not pixels. It was 12, chosen as "about four and a half
# pixels of travel at 18pt" — which is true on a Retina screen and is also the
# wrong unit: a Retina pixel is half a point, so the eye was being offered 2.2
# points. That is below what peripheral vision picks up, and the reported
# symptom was the animation not running at all. It was running.
#
# 24 is 4.3 points, twice the movement, and still seven units clear of the
# nesting depth — so at the bottom of the travel the small V is plainly a V of
# its own rather than a shape that has merged with the big one.
AMPLITUDE = 24

# The mark, in its own 100-unit box. The two V's are the same drawing at two
# sizes; only the stroke weight changes between the colour icon and the mono
# template, which is why both paths below are weight-free and the callers set it.
SMALL = '<path d="M36 8 L50 30 L64 8" transform="translate(0 {dy})"/>'
BIG = '<path d="M18 42 L50 80 L82 42"/>'


def drift(i):
    """The small V's offset at frame `i` — one cosine over the sixteen frames.

    Zero at frame 0 by construction, which is what keeps `tray-0.png` identical
    to `tray.png`; the extremes are where the curve is flattest, so the mark
    dwells at the top and the bottom and moves quickest through the middle,
    without any of that having to be spelled out in a schedule.
    """
    return round(AMPLITUDE * (1 - math.cos(2 * math.pi * i / FRAMES)) / 2, 3)


def svg(dy, *, ink, stroke, bg, viewbox, place):
    mark = (f'<g fill="none" stroke="{ink}" stroke-width="{stroke}" '
            f'stroke-linecap="round" stroke-linejoin="round">'
            f'{SMALL.format(dy=dy)}{BIG}</g>')
    body = f'<rect width="100" height="100" rx="20" ry="20" fill="{bg}"/>' if bg else ""
    body += f'<g transform="{place}">{mark}</g>' if place else mark
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{viewbox}">'
            f'{body}</svg>')


# The tray frames must reuse `arvolo-tray-template.svg`'s framing exactly — same
# viewBox, same stroke, no wrapper transform — or frame 0 would not match the
# `tray.png` the menu bar already shows, and turning attention on would visibly
# resize the mark. The colour icon keeps its own 100-unit canvas with the glyph
# inset 19.5/100, matching the 200/1024 the other sources here use, and takes the
# lighter stroke the boxed app icons carry.
TRAY = dict(ink="#000000", stroke=12, bg=None,
            viewbox="2.75 -3.25 94.5 94.5", place=None)
APP = dict(ink=CREAM, stroke=10.5, bg=ORANGE, viewbox="0 0 100 100",
           place="translate(19.5 19.5) scale(0.609)")


def render(text, name, px):
    tmp = OUT / f".{name}.svg"
    tmp.write_text(text)
    subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), str(tmp),
                    "-o", str(OUT / f"{name}.png")], check=True)
    tmp.unlink()


def main():
    OUT.mkdir(exist_ok=True)
    for family, px, name in ((TRAY, 36, "tray"), (APP, 64, "app")):
        for i in range(FRAMES):
            render(svg(drift(i), **family), f"{name}-{i}", px)
    print(f"scritti {FRAMES * 2} fotogrammi in {OUT}")


if __name__ == "__main__":
    main()
