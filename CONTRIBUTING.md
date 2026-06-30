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

> Note: until the project formalizes a Contributor License Agreement, non-trivial
> external contributions may be deferred so the dual-licensing rights stay clean.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Requires a recent Rust toolchain (>= 1.88; CI uses stable).

## Trademark

"Arvolo" and the Arvolo logo are trademarks of the project owner. The AGPL covers
the *code*, not the *name*: forks and derivatives may not use the Arvolo name or
branding in a way that implies endorsement. See the README for details.
