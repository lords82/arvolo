# design-sync notes — arvolo-gui

Repo-specific gotchas for syncing `gui/` to claude.ai/design. Run every command
from `gui/`, not the repo root.

## This package is an app, not a library

There is no library build and no published `dist/` — `gui/dist/` is the Vite app
bundle. Two consequences, both already handled in `config.json`:

- **The JS bundle uses synth-entry mode.** The converter synthesizes an entry
  from `srcDir` (`src/ui`) and esbuild bundles it. `[NO_DIST] no built entry —
  synthesizing from 6 src files` on every run is expected, not a failure.
- **`node_modules/arvolo-gui` must exist or the build dies** with
  `ENOENT … node_modules/arvolo-gui/package.json`. It's a self-link, the same
  thing `npm link` makes. Recreate it **per clone** (it is inside the gitignored
  `node_modules/`):
  ```sh
  cd gui && ln -sfn .. node_modules/arvolo-gui
  ```

## Prop contracts need a generated .d.ts tree — regenerate before every sync

Without a types tree, every emitted `<Name>.d.ts` collapses to
`[key: string]: unknown` — the design agent then has no API to code against.
Two generated, gitignored artifacts fix it, and **both must be regenerated
whenever `src/ui/` changes**:

```sh
cd gui
rm -rf .ds-types && npx tsc -p .design-sync/tsconfig.dts.json
for m in Primitives Bits Sheet Menu Toasts Icons; do
  echo "export * from './.ds-types/ui/$m';"
done > index.d.ts
```

`index.d.ts` at the package root is the barrel the converter looks for
(`pkgJson.types` is unset, so it falls back to `index.d.ts`). Don't add `types`
to `package.json` — that would ship a dev-only path to real consumers.

## Component discovery and grouping

- Everything lands in group `general`. Grouping is derived from the directory
  under `srcDir`, and all 26 components live in six flat files
  (`Primitives.tsx`, `Bits.tsx`, `Sheet.tsx`, `Menu.tsx`, `Toasts.tsx`,
  `Icons.tsx`), so there are no subdirectories to group by. To split the pane
  into Actions/Forms/Feedback/…, add `docsMap` stubs whose only content is
  `---\ncategory: <Group>\n---`.
- `docs: 0/26 matched` is expected — there is no per-component docs tree.
  `.prompt.md` is synthesized from the `.d.ts` plus each component's leading
  JSDoc, which is why the tsc step above matters for docs too.
- `Icon` is excluded via `componentSrcMap` — it's a record of icon components,
  not a component, so it would crash as `<Icon />`. It still ships in the bundle
  as `window.Arvolo.Icon`, and previews use `<Icon.Send />`.
- `Segmented` is generic (`<T extends string>`); the emitted props leaked a bare
  `T`. Pinned via `dtsPropsFor` with `T` → `string`.

## Known render warns

None — the final validate is clean (26/26 render, 0 warnings). If any of these
come back, here is what they meant:

- **`[FONT_MISSING]` for Inter / JetBrains Mono / Roboto Mono.** Suppressed via
  `runtimeFontPrefixes`. These are *not* missing brand fonts: they are optional
  entries inside the system stacks in `theme.css` (`--font` starts at
  `-apple-system`, `--mono` at `ui-monospace`). Nothing renders in a wrong
  fallback — the DS is deliberately system-font. No woff2 to ship.
- **`Menu`: `anchor.getBoundingClientRect is not a function`.** Menu measures an
  anchor ref it is not given on a floor card. Harmless there; if `Menu` ever
  gets an authored preview, pass a real anchor element.

## Preview gotchas (`.design-sync/previews/`)

- **`Sheet` is `position: fixed`.** Both remedies the tool suggests are wrong
  here: `cardMode: single` renders a *blank* screenshot (a fixed root measures
  as zero), and the default grid mode makes it escape its cell. The fix that
  works is in `previews/Sheet.tsx` — a wrapper with `transform: translateZ(0)`,
  which makes the wrapper the containing block so the sheet and its scrim render
  inside the card. `Confirm` renders fine unwrapped, so it needs no preview.
- `CopyButton`, `Empty`, `Field` use `cardMode: column` — their stories are
  wider than a grid cell.
- Previews import from `"arvolo-gui"`, which resolves through the self-link.

## Re-sync risks

- **Stale `.ds-types/` / `index.d.ts` are silent.** They are gitignored, so a
  fresh clone has none and a stale one is never flagged: props would regress to
  `[key: string]: unknown`, or worse, describe an API that has since changed,
  with no warning. Always run the tsc step above before the driver.
- **The self-link is per-clone** and its absence is a hard crash, not a warning.
- **`conventions.md` enumerates real class and token names** scraped from
  `theme.css`. If that stylesheet is refactored (classes renamed, tokens
  dropped), the header will confidently name vocabulary that no longer resolves
  and the design agent will emit silently unstyled markup. Re-validate every
  name in it against `ds-bundle/_ds_bundle.css` on each sync.
- **21 components ship the floor card** (typographic placeholder) by the user's
  choice of a fast first import. They are fully importable and fully typed —
  only the preview picture is a placeholder. Authoring
  `.design-sync/previews/<Name>.tsx` for any of them on a later sync is the
  standing improvement, and grades carry forward.
- Only 2 of 26 components fuzzy-match a source file (`Sheet`, `Menu`), because
  the rest share flat files. That is cosmetic today (it affects group and JSDoc
  discovery, both already handled) but means `componentSrcMap` pins would be
  needed if per-component src paths ever start mattering.
