# Contributing to Arvolo

Thanks for your interest in Arvolo!

## License of contributions

Arvolo's open core is licensed under **AGPL-3.0-only** (see [`LICENSE`](LICENSE)).
Arvolo is also offered under a separate **commercial license** for users who do
not want the AGPL's obligations. To keep this dual-licensing possible, every
contribution must be made under terms that allow the maintainers to relicense it.

By submitting a contribution (pull request, patch, etc.) you certify the
**Developer Certificate of Origin** (DCO, https://developercertificate.org/) and
agree that your contribution may be distributed under **both** the AGPL-3.0 and
the Arvolo commercial license. Sign your commits with `git commit -s` (adds a
`Signed-off-by` line).

> The sign-off is all that is asked for. For anything larger than a fix, open an
> issue first and let's agree on the shape before you write it — that is worth
> doing on any project, and here it also gives us a chance to talk through the
> dual-licensing side before, rather than after, you have spent your evening on it.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Requires a recent Rust toolchain (>= 1.88; CI uses stable).

## Copy

Anything the user reads — a label, an error, a line of `--help` — follows
[docs/COPY.md](docs/COPY.md). In one line: say what happened, say what they can
do about it, and put the reason on the second level (a hint line in the GUI,
`-v` in the CLI) instead of in front of them every time.

`gui/src/test/copy.test.ts` enforces the length caps and the voice, and lists
what is still over. Commit messages, release notes and PR titles are in English.

## Trademark

"Arvolo" and the Arvolo logo are trademarks of the project owner. The AGPL covers
the *code*, not the *name*: forks and derivatives may not use the Arvolo name or
branding in a way that implies endorsement. See the README for details.
