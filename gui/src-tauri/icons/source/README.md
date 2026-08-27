# Icon sources

The PNG/ICNS/ICO files one level up are **generated**. These five SVGs are the
originals; edit them, not the bitmaps. The attention frames are generated too, but
from `gen-attention.py` rather than a checked-in SVG, because their geometry is
computed per frame — see "The attention state" below.

The mark is concept **3d — "Doppia V"** from the Arvolo design system: two
concentric V's, the small one about to drop into the big one — the file still in
flight and the thing waiting to receive it, with no box drawn around either.
Every file here carries the identical geometry, drawn in a 100-unit box —

```
<path d="M36 8 L50 30 L64 8"/>
<path d="M18 42 L50 80 L82 42"/>
```

— `fill="none"`, both caps and joins round, on `#F97316` in `#faf8f6`.

**The stroke weight is not the same everywhere**, and neither is the glyph's
inset. Three combinations ship, for three different jobs.

| | Inset (of 1024) | Stroke | Used by |
| --- | --- | --- | --- |
| Boxed, regular | 240 macOS · 200 full bleed | 10.5 | every size from 30px up |
| Boxed, small | 196 macOS · 150 full bleed | 12.5 | 16px and 24px only |
| Bare monochrome | — | 12 | the menu bar, at any size |

The first two are the design system's own: a glyph sitting on its own coloured
tile can carry the finer line, and at 10.5 the channel between the two V's stays
open down to 30px. The bare template has no tile holding it, is scaled down
further still, and is tinted by the system, so it needs 12 or the small V closes
up. Those two are set independently on purpose — changing one does not and should
not change the other.

**The small boxed size is ours, not the design system's**, which draws 10.5 at
every boxed size including its 16px specimen. That specimen is an SVG scaled down
by a browser; a real 16px bitmap is not, and there the stroke lands at about one
pixel and antialiasing takes most of it. Two nested shapes need more room than the
one shape this mark replaced, so at 16 and 24 the glyph grows ~16% and the stroke
goes to 12.5, which is enough to read as *two* V's again. The crossover was picked
by eye across 16/20/24/30/32/44: below 30 the heavier cut wins, at 44 the regular
one has the better proportion, and 30 is where they are indistinguishable — so the
seam sits where nobody can see it.

That makes the ink bbox, stroke included, x 12.75→87.25, y 2.75→85.25 at stroke
10.5, and x 12→88, y 2→86 at stroke 12.

| Source | Feeds |
| --- | --- |
| `arvolo-icon.svg` | `icon.png`, and every `icon.icns` layer from 32px up — macOS |
| `arvolo-icon-small.svg` | `icon.icns`'s `16x16` and `16x16@2x` layers — macOS |
| `arvolo-icon-square.svg` | `32x32.png`, `128x128*.png`, `Square*Logo.png`, `StoreLogo.png`, and `icon.ico` from 32px up — Linux and Windows |
| `arvolo-icon-square-small.svg` | `icon.ico`'s 16px and 24px entries — Windows |
| `arvolo-tray-template.svg` | `tray.png` — the macOS menu bar |
| `gen-attention.py` | `../attention/tray-*.png`, `../attention/app-*.png` — the attention states |

Two app shapes, because the platforms disagree about the canvas. macOS reserves
a margin: the artwork is a 824×824 squircle centred on a 1024×1024 canvas, and
an icon that ignores the grid sits visibly larger than its neighbours in the
Dock. Windows and Linux draw theirs full bleed, so the same inset would make
Arvolo the small one there — `arvolo-icon-square.svg` fills the canvas with a
20% corner radius. The glyph is inset 240/1024 on the macOS master and 200/1024
full bleed, so it reads at the same size once the margin is accounted for. The
two small-size sources keep that same relation between them: both grow their
glyph's span by the same ~16%, so macOS and Windows stay in step at 16px too.

The squircle is a superellipse (exponent 5) rather than a rounded rect: its
profile was checked against `App Store.app`'s icon and matches within 2px at
every sampled offset, which `rx="186"` does not.

`tray.png` is a **template image**: black on alpha, no colour. macOS tints it to
match the menu bar — white on dark, black on light — the way every other status
item behaves; `setup_tray` in `src/main.rs` loads it. It is 36×36 (2× of the
18pt the system scales it to) and the SVG's viewBox — `2.75 -3.25 94.5 94.5`, the
stroke-12 ink bbox centred in a square — frames the glyph so its **height** lands
at 32px, i.e. 16pt, the height its neighbours sit at. Height and not width, which
is the change from the old mark: this one is taller than it is wide, so pinning
the width would push it past the bar. The viewBox origin is negative because the
square is 10.5 units taller than the ink it centres. Colour is only correct on
Windows and Linux, which keep the full app icon.

The relay's `dl.html` and `disabled.html` inline the same glyph by hand: those
pages may not fetch anything, so the geometry is duplicated there. Change the
mark and they need the same edit.

## The attention state

`../attention/` holds the frames that say *something is waiting for you*. There is
no counter and no badge: the mark says it itself. The small V is the file in
flight, so while a decision is pending it **breathes** — drifting down toward the
big V and back up, over and over, for exactly as long as something is waiting. At
rest it sits still at the top.

| Family | Canvas | Feeds |
| --- | --- | --- |
| `tray-N.png` | 36×36, black on alpha | the macOS menu bar |
| `app-N.png` | 64×64, full-bleed colour | the Windows and Linux tray |

The sixteen frames are one full breath and they loop: frame 0 is the resting
position, 0→8 drift down, 9→15 come back up. It runs until the last offer has been
approved or refused — there is no fixed number of repeats and no landed state to
hold, which is the substantive change from the mark this replaced.

**The positions are not a linear ruler**, and that is the other thing that
changed. The old wedge fell once and stopped, so its frames could be evenly spaced
and the shape of the fall could live in `src/attention.rs` as a schedule of
(frame, hold) pairs. An oscillation admits no such split: its shape *is* a cosine,
and the only thing worth retuning is how long a breath takes. So the cosine is
baked into the bitmaps by `drift()`, every frame is held for the same `STEP_MS`,
and `BREATH_MS` over there is the single number. The curve pays for itself anyway —
a cosine is flattest at its extremes, so the mark dwells at the top and the bottom
and hurries through the middle without a schedule having to say so.

It travels `AMPLITUDE` — 12 units of 100, about 4.5px once the menu bar has scaled
it — deliberately short of the ~31 that would nest the small V inside the big one.
The breath runs indefinitely, so it has to catch the eye sideways without turning
the menu bar into a place where things move; and both families use the same 12,
because nothing here depends on the stroke weight any more.

`tray-0.png` is byte-identical to `tray.png` on purpose: if
the resting frame were framed even slightly differently, turning attention on and
off would visibly resize the mark in the menu bar. `drift(0)` is zero by
construction, which is what keeps that true.

Clearing the last request does not cut to the resting frame. `settle` walks the
mark home the short way round — back the way it came if it has not passed the
bottom, onward through the rest of the cycle if it has — so the return is at most
half a breath and never a jump.

A badge was tried first and does not survive this icon. At 36px a corner circle
either shrinks to an invisible sliver or, sized to be legible, eats the big V's
right arm until the glyph reads as a **check mark** — there is no middle ground
inside 36 pixels, with a digit or with an exclamation mark. Moving a piece that is already there
costs nothing and stays legible at any size, because it is motion and position
rather than detail.

```sh
cd gui/src-tauri/icons/source
python3 gen-attention.py        # needs rsvg-convert
```

## Regenerating

Needs `rsvg-convert`, `magick` and `iconutil` (`brew install librsvg imagemagick`;
`iconutil` ships with Xcode's command line tools).

**`tauri icon` is deliberately not used any more.** It scales one master to every
layer, which is exactly what the small optical size exists to avoid — its 16px
output is a downscale of the 10.5 cut, not the 12.5 one. The icns is assembled by
hand instead, which is also why `ios/`, `android/` and `64x64.png` no longer need
deleting after every run.

```sh
cd gui/src-tauri/icons

# macOS. The two 16pt layers come from the small source — @2x included, so a
# retina and a non-retina screen show the same drawing at the same nominal size.
rsvg-convert -w 1024 -h 1024 source/arvolo-icon.svg -o icon.png
rm -rf /tmp/arvolo.iconset && mkdir -p /tmp/arvolo.iconset
for spec in 16x16:16:small 16x16@2x:32:small 32x32:32:big 32x32@2x:64:big \
            128x128:128:big 128x128@2x:256:big 256x256:256:big 256x256@2x:512:big \
            512x512:512:big 512x512@2x:1024:big; do
  name="${spec%%:*}"; rest="${spec#*:}"; px="${rest%%:*}"; cut="${rest##*:}"
  src="source/arvolo-icon.svg"
  [ "$cut" = small ] && src="source/arvolo-icon-small.svg"
  rsvg-convert -w "$px" -h "$px" "$src" -o "/tmp/arvolo.iconset/icon_$name.png"
done
iconutil -c icns /tmp/arvolo.iconset -o icon.icns

# Windows + Linux: full bleed. Everything here is 30px or larger, so it is all
# the regular cut.
for spec in 32x32.png:32 128x128.png:128 128x128@2x.png:256 StoreLogo.png:50 \
            Square30x30Logo.png:30 Square44x44Logo.png:44 Square71x71Logo.png:71 \
            Square89x89Logo.png:89 Square107x107Logo.png:107 \
            Square142x142Logo.png:142 Square150x150Logo.png:150 \
            Square284x284Logo.png:284 Square310x310Logo.png:310; do
  rsvg-convert -w "${spec#*:}" -h "${spec#*:}" source/arvolo-icon-square.svg -o "${spec%:*}"
done

# The .ico is the one file that carries both cuts: 16 and 24 small, the rest
# regular. Getting this backwards is invisible at review size and obvious in a
# Windows file list.
for s in 16 24; do
  rsvg-convert -w $s -h $s source/arvolo-icon-square-small.svg -o "/tmp/ico-$s.png"
done
for s in 32 48 64 256; do
  rsvg-convert -w $s -h $s source/arvolo-icon-square.svg -o "/tmp/ico-$s.png"
done
magick /tmp/ico-16.png /tmp/ico-24.png /tmp/ico-32.png \
       /tmp/ico-48.png /tmp/ico-64.png /tmp/ico-256.png icon.ico

# Menu bar.
rsvg-convert -w 36 -h 36 source/arvolo-tray-template.svg -o tray.png
```
