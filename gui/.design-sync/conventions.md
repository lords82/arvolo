# Building with Arvolo

Arvolo is the desktop UI of a peer-to-peer file-transfer app. It is a **CSS-class
design system**: one stylesheet (`styles.css` → `_ds_bundle.css`) defines every
token and every component class, and components carry class names rather than
inline style. Read `_ds_bundle.css` before styling anything — it is the truth,
and it is commented.

## No provider, no theme setup

There is no provider to wrap. Components read no React context: language comes
from a module-level store, so `window.Arvolo.Button` works standing alone. Just
render components; the stylesheet does the rest.

**Theming.** Light and dark are both first-class and the palette is defined
twice. Dark follows the OS by default; set `data-theme="dark"` or
`data-theme="light"` on `<html>` to override. Never hard-code a hex colour —
every colour you need is a token below, and hard-coding breaks dark mode.

## Two rules that run through the whole system

1. **Direction is a colour.** Outgoing is warm (`--out`, amber), incoming is cool
   (`--in`, blue). Progress bars, avatar rings, row accents and icons all obey
   it, so a glance says which way the bytes go. Semantic colours (`--green`,
   `--red`, `--amber`) are reserved for *outcomes* — never for direction.
2. **Trust is typography.** Public ids, fingerprints and pairing codes are the
   security surface. Set them in `--mono` (class `.mono`), spaced and selectable,
   and never truncate one where a decision depends on it. Prefer the components
   that already do this: `ShortId`, `Fingerprint`, `CopyField`, `CodeHero`.

## Token vocabulary (use `var(--token)`)

| Family | Tokens |
|---|---|
| Surfaces | `--canvas` (page), `--surface`, `--surface-2`, `--surface-3`, `--surface-sunk`, `--track` |
| Ink | `--ink`, `--ink-sec`, `--ink-mut`, `--ink-weak` |
| Lines | `--line`, `--line-strong`, `--line-hard` (rules use the `.divider` class) |
| Direction | `--out`, `--out-soft`, `--out-fill`, `--out-line`, `--out-strong`, `--out-stall` — and the same six on `--in` |
| Semantic | `--green`, `--red`, `--amber`, `--teal`, `--violet`, each with a `-soft` pair |
| Radius | `--r-xs`, `--r-sm`, `--r-md`, `--r-lg`, `--r-xl`, `--r-pill` |
| Elevation | `--shadow-1`, `--shadow-2`, `--shadow-3`, `--overlay-scrim` |
| Type | `--font` (system stack), `--mono` |
| Motion | `--t-fast`, `--t`, `--t-slow`, `--ease` |
| Focus | `--focus`, `--focus-ring` |

## Class vocabulary for your own layout glue

Compose with these rather than inventing names:

- **Layout**: `.stack`, `.stack-sm`, `.hstack`, `.hstack-sm`, `.grow`, `.row`,
  `.rows`, `.section`, `.section-head`, `.card`, `.card-head`, `.card-pad`,
  `.view`, `.divider`
- **Type helpers**: `.t-title`, `.t-head`, `.t-sec`, `.t-label`, `.t-sm`,
  `.t-xs`, `.t-mut`, `.mono`, `.tnum`, `.truncate`, `.wrap`, `.selectable`
- **Tone / tint** (text vs background): `.tone-out`, `.tone-in`, `.tone-ok`,
  `.tone-bad`, `.tone-warn`, `.tone-mut`, `.tone-violet` — and `.tint-*` with the
  same suffixes
- **Stats**: `.stats`, `.stat`, `.stat-label`, `.stat-value`

Component-owned classes (`.btn-*`, `.sheet`, `.field`, `.empty`, `.prog`,
`.menu`, `.toast`, `.segmented`, `.switch`, `.input`, `.avatar`, `.badge`,
`.kbd`, `.qr`, `.code-hero`, `.copyfield`, `.fingerprint`, `.ext`) belong to the
components — render the component instead of reproducing its markup.

## Idiomatic example

```jsx
<div className="card card-pad stack">
  <div className="section-head">
    <span className="t-head">Transfers</span>
    <Badge>2 active</Badge>
  </div>

  <div className="row">
    <div className="row-main">
      <div className="row-name mono truncate">contract-draft.pdf</div>
      <div className="row-meta t-xs t-mut">
        <ExtChip name="contract-draft.pdf" /> 4.8 MB · to Lorenzo's MacBook
      </div>
    </div>
    <div className="row-actions">
      <IconButton label="Pause"><Icon.Pause /></IconButton>
    </div>
  </div>

  <Empty icon={<Icon.Send />} title="Nothing in flight">
    Transfers you send or receive show up here while they run.
  </Empty>
</div>
```

`Badge` carries the outcome vocabulary in a prop, not a class:
`kind="out" | "neutral" | "ok" | "warn" | "bad" | "info" | "trust" | "dev"`.
`IconButton` requires `label` — an icon-only control with no accessible name is
unlabelled on hover and invisible to a screen reader.

`Icon` is a record of components, not a component: use `<Icon.Send />`,
`<Icon.Check />`, `<Icon.People />`. Toasts are imperative — render `<ToastHost />`
once, then call `toast.ok(...)` / `toast.bad(...)` / `toast.info(...)`.
