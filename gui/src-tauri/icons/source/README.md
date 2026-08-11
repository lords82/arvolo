# Icon sources

The PNG/ICNS/ICO files one level up are **generated**. These three SVGs are the
originals; edit them, not the bitmaps. The counter badge is generated too, but
from `gen-badge.py` rather than a checked-in SVG — see "The badge" below.

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
| `gen-badge.py` | `../badge/tray-*.png`, `../badge/app-*.png`, `../badge/overlay.png` — the counter |

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
item behaves; see `TRAY_TEMPLATE_PNG` in `src/main.rs`. It is 36×36 (2× of the
18pt the system scales it to) and the SVG's viewBox frames the glyph so it lands
at 32px, i.e. 16pt, the height its neighbours sit at. Colour is only correct on
Windows and Linux, which keep the full app icon.

The relay's `dl.html` and `disabled.html` inline the same glyph by hand: those
pages may not fetch anything, so the geometry is duplicated there. Change the
mark and they need the same edit.

## The badge

`../badge/` holds the counter variants: the tray icon with a number on it, one
file per count, `1`–`9` then `9+`. The number is a circle knocked **out** of the
mark — a real hole, not a disc painted on top — with the digit sitting in it.
That is what makes a single file work everywhere: the hole is transparent, so the
same bitmap reads against a light bar and a dark one, and no platform needs its
own colour treatment. Nothing here is macOS-only; every platform goes through
`TrayIcon::set_icon`.

The two families place the badge differently, and the difference is deliberate:

| Family | Canvas | Where the circle bites |
| --- | --- | --- |
| `tray-N.png` | 36×36, black on alpha | The mark is **full bleed**, so the circle eats into it: the wedge loses its right corner and the box's right arm comes out shorter than the left. |
| `app-N.png` | 64×64, full-bleed colour | The mark is inset 19.5/100, so the same top-right circle removes only orange and the glyph is untouched. |

The bitten look on the menu bar is a choice, not a bug — a knockout needs a
filled field to remove, and the template has none, so inside a square canvas the
alternative was shrinking the mark by a fifth to let the circle pass. Restore the
unbitten version by giving `TRAY` in `gen-badge.py` the same inset `APP` has.

`overlay.png` is the Windows taskbar overlay, which the system draws at 16pt. A
digit does not survive that size — it turns to a blob — so the overlay carries a
plain disc and only says *there is something*. The same limit applies to a
Windows tray at 100% DPI, where the 64px `app-N.png` is downsampled to 16px; the
digit is legible from about 24px and clean from 32px.

Digits are baked in as **outlines**, pulled from Helvetica Bold in
`/System/Library/Fonts/Helvetica.ttc`, so regenerating does not depend on which
fonts a machine has installed. Only the script does, and only when the geometry
changes. Ten glyphs that never change are not worth a font rasteriser inside the
GUI, which is why none of this is drawn at runtime.

```sh
cd gui/src-tauri/icons/source
python3 gen-badge.py            # needs fonttools and rsvg-convert
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
