# Icon sources

The PNG/ICNS/ICO files one level up are **generated**. These three SVGs are the
originals; edit them, not the bitmaps. The attention frames are generated too, but
from `gen-attention.py` rather than a checked-in SVG, because their geometry is
computed per frame — see "The attention state" below.

The mark is concept **2c — "In arrivo"** from the Arvolo design system: a wedge
dropping into an open box, the file still in flight and the box waiting for it.
Every file here carries the identical glyph, drawn in a 100-unit box —

```
<path d="M18 46 V64 a16 16 0 0 0 16 16 h32 a16 16 0 0 0 16 -16 V46"
      fill="none" stroke-width="12" stroke-linecap="round"/>
<path d="M50 40 L38 12 H62 Z"/>
```

— on `#F97316` in `#faf8f6`. Its ink bbox, stroke included, is x 12→88, y 12→86.

| Source | Feeds |
| --- | --- |
| `arvolo-icon.svg` | `icon.png`, `icon.icns` — macOS |
| `arvolo-icon-square.svg` | `32x32.png`, `128x128*.png`, `icon.ico`, `Square*Logo.png`, `StoreLogo.png` — Linux and Windows |
| `arvolo-tray-template.svg` | `tray.png` — the macOS menu bar |
| `gen-attention.py` | `../attention/tray-*.png`, `../attention/app-*.png` — the attention states |

Two app shapes, because the platforms disagree about the canvas. macOS reserves
a margin: the artwork is a 824×824 squircle centred on a 1024×1024 canvas, and
an icon that ignores the grid sits visibly larger than its neighbours in the
Dock. Windows and Linux draw theirs full bleed, so the same inset would make
Arvolo the small one there — `arvolo-icon-square.svg` fills the canvas with a
20% corner radius. The glyph is inset 240/1024 on the macOS master and 200/1024
full bleed, so it reads at the same size once the margin is accounted for.

The squircle is a superellipse (exponent 5) rather than a rounded rect: its
profile was checked against `App Store.app`'s icon and matches within 2px at
every sampled offset, which `rx="186"` does not.

`tray.png` is a **template image**: black on alpha, no colour. macOS tints it to
match the menu bar — white on dark, black on light — the way every other status
item behaves; `setup_tray` in `src/main.rs` loads it. It is 36×36 (2× of the
18pt the system scales it to) and the SVG's viewBox frames the glyph so it lands
at 32px, i.e. 16pt, the height its neighbours sit at. Colour is only correct on
Windows and Linux, which keep the full app icon.

The relay's `dl.html` and `disabled.html` inline the same glyph by hand: those
pages may not fetch anything, so the geometry is duplicated there. Change the
mark and they need the same edit.

## The attention state

`../attention/` holds the frames that say *something is waiting for you*. There is
no counter and no badge: the mark says it itself. The wedge is the file in flight,
so when a decision is pending it **falls into the box** and stays there; at rest it
sits above, still in the air.

| Family | Canvas | Feeds |
| --- | --- | --- |
| `tray-N.png` | 36×36, black on alpha | the macOS menu bar |
| `app-N.png` | 64×64, full-bleed colour | the Windows and Linux tray |

Frame 0 is the resting state and frame 7 is the landed one, which the tray holds
for as long as anything is pending; the frames between are only played on the way
in. The fall accelerates — `dy` scales with `t` squared — so it reads as a drop
rather than a slide. `tray-0.png` is byte-identical to `tray.png` on purpose: if
the resting frame were framed even slightly differently, turning attention on and
off would visibly resize the mark in the menu bar.

A badge was tried first and does not survive this icon. At 36px a corner circle
either shrinks to an invisible sliver or, sized to be legible, eats the box's right
arm until the glyph reads as an **L** — there is no middle ground inside 36 pixels,
with a digit or with an exclamation mark. Moving a piece that is already there
costs nothing and stays legible at any size, because it is motion and position
rather than detail.

```sh
cd gui/src-tauri/icons/source
python3 gen-attention.py        # needs rsvg-convert
```

## Regenerating

Needs `rsvg-convert` and `magick` (`brew install librsvg imagemagick`).

```sh
cd gui/src-tauri/icons

# macOS: 1024 master through the Tauri CLI (it writes icon.png + icon.icns,
# plus the Windows/Linux set that the next step overwrites, and ios/ +
# android/ folders this desktop-only app does not use — delete them).
rsvg-convert -w 1024 -h 1024 source/arvolo-icon.svg -o /tmp/icon-1024.png
(cd ../.. && ./node_modules/.bin/tauri icon /tmp/icon-1024.png)
rm -rf ios android 64x64.png

# Windows + Linux: full bleed.
for spec in 32x32.png:32 128x128.png:128 128x128@2x.png:256 StoreLogo.png:50 \
            Square30x30Logo.png:30 Square44x44Logo.png:44 Square71x71Logo.png:71 \
            Square89x89Logo.png:89 Square107x107Logo.png:107 \
            Square142x142Logo.png:142 Square150x150Logo.png:150 \
            Square284x284Logo.png:284 Square310x310Logo.png:310; do
  rsvg-convert -w "${spec#*:}" -h "${spec#*:}" source/arvolo-icon-square.svg -o "${spec%:*}"
done
for s in 16 24 32 48 64 256; do
  rsvg-convert -w $s -h $s source/arvolo-icon-square.svg -o "/tmp/ico-$s.png"
done
magick /tmp/ico-16.png /tmp/ico-24.png /tmp/ico-32.png \
       /tmp/ico-48.png /tmp/ico-64.png /tmp/ico-256.png icon.ico

# Menu bar.
rsvg-convert -w 36 -h 36 source/arvolo-tray-template.svg -o tray.png
```
