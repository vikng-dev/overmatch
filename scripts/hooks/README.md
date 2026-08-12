# Git hooks

Local pre-commit / pre-push gates that mirror CI's ordinary checks, so formatting/lint drift is
caught before it reaches `main`. (MEASURED: CI sat red for two days in July 2026 from formatting
drift that a pre-commit hook would have blocked at the source.)

## One-time setup (per clone)

```sh
git config core.hooksPath scripts/hooks
```

That's the whole wire-up. Coming from JS: there is no `npm install`/husky `postinstall` step in
cargo, so this single `git config` replaces it. The path is **relative and versioned**, so it
resolves per working tree — every `git worktree` uses its own copy automatically, and the hooks
travel with the repo instead of living in the un-versioned `.git/hooks`.

## What runs

| Hook | Command | Mirrors | Cost |
|------|---------|---------|------|
| pre-commit | `cargo fmt --all --check` | CI `fmt` job | no compile |
| pre-push | the git-lfs upload, then fmt + clippy + the asset door over the trios this push changed | CI's fmt, clippy and asset lanes | warm cache = tens of seconds; plus a Blender launch and a MEASURED minute of texture encode per changed asset, and most pushes change none |
| pre-push, `OVERMATCH_FULL=1` | the above, plus the LOD chain verification and the full `cargo test` (excluding the exact 30-receiver stress probe) | CI's LOD and ordinary-test lanes too | a full compile and test run |

**Why the full suite is not the default.** CI runs `cargo test` and the LOD verification on every
push, and CI is the ratified backstop — running them here too buys a few minutes of local wall clock
for a second opinion that arrives ten minutes later on the same commit. What stays in the default is
what CI is slow or unable to save you from: the LFS transport (CI cannot upload objects it never
received), and the cheap gates whose failures are pure waste to discover remotely. Use
`OVERMATCH_FULL=1 git push` before a release, or when you will not be watching CI.

The **asset lane** runs `scripts/tank/build.py verify` — the one tank build, which drives the asset
door as its certification step — on every asset the push changes, and on every asset it discovers
when the push changes the shared surface that verdict is computed from (the material library, the
toolchain pin, the source pass, the build, the door, the encoder, the derivation verifier, the Rust
consumer contract). `verify` re-cuts the asset from its own stored source and holds the tracked
model, with its certified rung records stripped, against the result section by section — byte for
byte wherever the chain is deterministic, and by stated KTX2 header facts over the texture payloads,
whose bytes `basisu` varies with the SIMD it was built for. That subsumes the mip gate this lane
replaced: a tracked glb the compare law holds against a candidate the derivation verifier passed is
mipped KTX2 by construction. It also re-derives the sim artifact from the tracked view artifact and
holds the certificate's three digests against the bytes beside it (ADR 0035); no LOD search runs
here, because the certificate is what carries the measurements forward.

Assets are **discovered**, never listed: an asset is a sibling trio `<id>.blend`, `<id>.tank.ron`,
`<id>.glb` in one directory, hydrated with the two artifacts the build publishes beside the model
(`<id>.sim.glb`, `<id>.lod.json`). Adding a second vehicle needs no hook edit. Discovery, selection and
hydration live in `scripts/hooks/pushed_assets.sh`, sourced by the hook — and by CI's assets job,
which asks the same two predicates over the push's whole range to decide whether to run its ~35
minute re-cut at all. All of it is driven over synthetic revisions, and the hook itself over
synthetic pushes, by `sh scripts/hooks/test_pushed_assets.sh`.

It reads the **pushed revisions**, not the work tree: the trio's bytes come out of the pushed commit
(its git-lfs pointers resolved against this clone's object store), because the work tree is a
different, mutable thing from what the remote is about to receive. If an object or the linked
`assets/materials/materials.blend` is not there, the lane REFUSES and names what is missing — it
never falls back to the work tree and never passes on absence.

A green pre-push does not certify the separately bounded CI stress lane; CI remains authoritative.

## Escape hatch — name the lane, never `--no-verify`

```sh
OVERMATCH_SKIP=clippy git push          # skip one gate lane
OVERMATCH_SKIP=assets,clippy git push   # or several: assets, fmt, clippy — and lod, test under FULL
```

Every skip prints loudly, and CI re-runs the skipped lane on the pushed commit.

**Do not use `git push --no-verify`**: the pre-push hook is the only git-lfs transport
(`core.hooksPath` replaces `.git/hooks`, so the hook git-lfs installs never runs). Skipping the
whole hook pushes pointers without objects. Recovery: `git lfs push --all origin` — the plain
per-branch form computes an empty delta and uploads nothing.

(`git commit --no-verify` remains fine — pre-commit is only a format check.)

## Trimming

Already trimmed once, deliberately: the `cargo test` and LOD lanes moved behind `OVERMATCH_FULL=1`
(2026-08-10) because CI runs them on every push anyway. If clippy-on-every-push still feels heavy,
`OVERMATCH_SKIP=clippy` per push, or drop its lane and let CI own it — the fmt check alone still
prevents the drift class that actually bit us.

The inventory of checks is **closed**. A new gate here or in CI needs a paid incident or a
demonstrated hole, not a plausible risk.
