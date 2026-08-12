#!/usr/bin/env python3
"""Generate the attention states for the tray icon.

There is no counter and no badge. The mark says it itself: the wedge is the file
in flight, so when something is waiting for the user to decide, the wedge **falls
into the box** and stays there. At rest it sits above, still in the air.

That leaves the mark whole, which a badge never managed — at 36px a corner circle
either shrinks to an invisible sliver or eats the box's right arm until the glyph
reads as an L. Moving a piece that is already there costs nothing.

Frames go into ../attention:

  tray-N.png    36x36, black on alpha — the macOS menu bar template.
  app-N.png     64x64, full-bleed colour — the Windows and Linux tray.

Frame 0 is the resting state and the last frame is the landed one, which is what
the tray holds for as long as anything is pending; the frames between them are
only played on the way in.

The travel is **linear**. An ease-in looks right on paper and wrong here: over so
few frames and so little distance — 32 units of 100, about 11px once the menu bar
has scaled it — the slow part wastes frames where nothing visibly moves and the
fast part lands in jumps you can count. Even steps read as motion; uneven ones
read as stutter.

Needs `rsvg-convert` (brew install librsvg).

    python3 gen-attention.py
"""
import pathlib
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "attention"

ORANGE, CREAM = "#F97316", "#faf8f6"
FRAMES = 12
# How far the wedge travels, in the mark's own 100-unit box. At +32 its base sits
# just inside the box's mouth and its tip clears the inner floor at y=74.
DROP = 32

BOX = ('<path d="M18 46 V64 a16 16 0 0 0 16 16 h32 a16 16 0 0 0 16 -16 V46" '
       'fill="none" stroke="{ink}" stroke-width="12" stroke-linecap="round"/>')
WEDGE = '<path d="M50 40 L38 12 H62 Z" fill="{ink}" transform="translate(0 {dy})"/>'


def svg(dy, *, ink, bg, viewbox, place):
    mark = BOX.format(ink=ink) + WEDGE.format(ink=ink, dy=dy)
    body = f'<rect width="100" height="100" rx="20" ry="20" fill="{bg}"/>' if bg else ""
    body += f'<g transform="{place}">{mark}</g>' if place else mark
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{viewbox}">'
            f'{body}</svg>')


# The tray frames must reuse `arvolo-tray-template.svg`'s framing exactly — same
# viewBox, no wrapper transform — or frame 0 would not match the `tray.png` the
# menu bar already shows, and turning attention on would visibly resize the mark.
# The colour icon keeps its own 100-unit canvas with the glyph inset 19.5/100,
# matching the 200/1024 the other sources here use.
TRAY = dict(ink="#000000", bg=None, viewbox="7.25 6.25 85.5 85.5", place=None)
APP = dict(ink=CREAM, bg=ORANGE, viewbox="0 0 100 100",
           place="translate(19.5 19.5) scale(0.609)")


def render(text, name, px):
    tmp = OUT / f".{name}.svg"
    tmp.write_text(text)
    subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), str(tmp),
                    "-o", str(OUT / f"{name}.png")], check=True)
    tmp.unlink()


def main():
    OUT.mkdir(exist_ok=True)
    for i in range(FRAMES):
        dy = round(DROP * i / (FRAMES - 1), 3)
        render(svg(dy, **TRAY), f"tray-{i}", 36)
        render(svg(dy, **APP), f"app-{i}", 64)
    print(f"scritti {FRAMES * 2} fotogrammi in {OUT}")


if __name__ == "__main__":
    main()
