---
name: lookup-dep-api
description: Find Bevy, Avian3d, or tungstenite API names (structs, traits, functions) without reading dependency source code. Use when unsure whether an API exists, what it's called, or when hitting compile errors about unknown items in dependencies.
---

# Looking up dependency APIs

Do **not** read dependency sources under `target/` or `~/.cargo` — the project keeps a grep-able
index of every public item exported by every crate in the dependency graph instead.

## Use the index

```bash
grep -i 'boomerang\|linearvelocity' .claude/deps-index.txt
```

Format: `=== <crate> ===` section headers, then one `kind<TAB>name` line per public item
(struct/trait/enum/function/constant/static/type_alias/macro/union), sorted and deduped. Good for
answering "does Avian have a `ConstantLinearAcceleration`?" or "what's the exact name of that
Bevy state trait?" — then consult docs.rs or type inference for signatures.

## (Re)generate the index

If `.claude/deps-index.txt` is missing or stale (e.g. after a dependency bump):

```bash
rm -f .claude/deps-index.txt
bash extract-cargo-expoorts.sh   # yes, "expoorts" — the typo is the real filename
```

That iterates every package from `cargo metadata` and appends its exports via
`./extract-exports.sh <crate>` (which runs `cargo +nightly rustdoc --output-format json` and
filters with `jq`).

Requirements: **nightly Rust** (`rustup toolchain install nightly`) and **jq**. Failures for
individual crates are silently skipped (`|| true`), so a crate missing from the index may just
have failed rustdoc — rerun for it alone: `./extract-exports.sh <crate>`.

Note: the full run rustdocs the entire dependency graph — slow the first time (minutes). Prefer
`./extract-exports.sh <crate>` for a single crate you care about.
