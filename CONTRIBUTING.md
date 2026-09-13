# Contributing to Reclaim

Thanks for helping. Reclaim recovers people's data and promises never to damage
it, so the bar for correctness and safety is high. Please read this before opening
a PR.

## Ground rules (non-negotiable)

1. **Read-only sources, by construction.** Nothing outside
   `reclaim-block/src/imaging/dest.rs` and `reclaim-session` may open anything for
   writing. `scripts/check-readonly.sh` enforces the allow-list in CI. A write to
   a source is a **P0** and will not be merged.
2. **No GPL/LGPL code — clean-room only.** PhotoRec/TestDisk, The Sleuth Kit,
   libfsapfs, apfs-fuse, dislocker and friends may be **read for understanding**
   but **never copied** — not their code and not their signature tables.
   Signatures and validators are written from **primary format specifications**
   (which are facts). `cargo deny` blocks any GPL/LGPL/AGPL dependency. A PR that
   copies code or a signature table verbatim will be rejected outright.
3. **Corruption is the normal input.** Parser crates carry
   `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]`.
   Every on-disk struct is parsed with bounds checks; no parser may panic on
   malformed data. New parsers need a `cargo fuzz` target before merge.
4. **Everything green.** `cargo ci` (fmt, `clippy -D warnings`, tests, `cargo
   deny`, the read-only grep, and — on macOS — the Recovery-libs linkage check)
   must pass. See `scripts/ci.sh`.

## Developer Certificate of Origin (DCO)

Reclaim uses the [DCO](https://developercertificate.org/) instead of a CLA. Every
commit must be signed off, certifying you wrote the change (or have the right to
submit it) and agree to license it under the project's terms:

```bash
git commit -s        # appends: Signed-off-by: Your Name <you@example.com>
```

The `Signed-off-by` name/email must be real. PRs without sign-off on every commit
will be asked to amend (`git rebase --signoff`).

## Licensing of contributions

Unless stated otherwise, contributions are licensed under **Apache-2.0** (the
project license). Do not contribute code you cannot license this way.

## Adding a file-signature (the common PR)

Signatures are **data**, not code (build guide Part 3.4). To add a format:

1. **TOML entry** in `crates/reclaim-sigs/catalog/*.toml` following the doc 05 §1
   schema (magic/offset, extensions, family, tier, validator name, footer if any).
   `build.rs` compiles the catalog.
2. **Validator** (only if header-matching isn't enough) in
   `crates/reclaim-carve/src/validators/<name>.rs`, registered by name. It decodes
   the header / verifies structure to reject false positives (doc 05 §4).
3. **Sample file** under `testdata/samples/`, **≤ 64 KiB**, and **synthetic or
   public-domain only** — never a copyrighted file, never a file carrying personal
   data. State its provenance in the PR.
4. **A test** that carves the sample and asserts the validator's verdict.

Then run `reclaim sigs test <sample>` and `cargo test -p reclaim-carve`.

## Larger changes

- Filesystem engines implement the `FileSystem` trait in `reclaim-fs-core`; wire
  them into `reclaim-session::meta::engines()`.
- Discuss architectural changes in an issue first — the phase build logs
  (`docs/build-log/`) and the plan (`docs/plan/`) are the design record.
- Keep the CLI contract and the on-disk session format stable (the GUI reads the
  same session files — there is no second data model).

## Commit & PR style

- Small, focused commits with a clear message; reference the FR/NFR or doc section
  where relevant.
- Regenerate derived docs when you change the surface: `scripts/gen-cli-docs.sh`
  (CLI reference), `scripts/gen-third-party.sh` (dependency licenses).
- CI runs on macOS and Linux; make sure both pass.
