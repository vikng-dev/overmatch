# Brief: track prints in the ground, first slice (#125)

You are an implementer lane for Overmatch, a Bevy 0.19 / Rust 1.98 video game. You work in a leased bay
(your worktree) and commit on its `lane/…` branch. Read `AGENTS.md` first; it is the rulebook. The subject
is cosmetic terrain marks, view only, and not the damage model.

## Read first

- `.agents/scratch/impact-feedback-2026-10-28/track-prints-approaches.md`: §2, §4 (approach 2, the one you
  build) and §9.
- `.agents/scratch/impact-feedback-2026-10-28/track-prints-ground-stamping.md`: §1 (link-derived stamps)
  and §4 (the design).
- `.agents/scratch/impact-feedback-2026-10-28/wot-scout-round12-track-traces.md`, for context only.
- Issue 125 (`gh issue view 125`).

Both research notes are claims. Where they and the code disagree, the code wins.

## Decided (fixed)

Don't reverse these. If you disagree, flag it in your report.

- **Approach 2: a trail map read by the ground's shader.**
  - This slice covers the stamps, the log, the stores, darkening and normals.
  - Parallax and the sun march are the next slice. Don't build them, and don't block them either: the store
    keeps depth, not only a normal.
- **The stamp is derived from the link.**
  - At load, bake the underside of the shoe mesh into a height-and-coverage stamp in the shoe's canonical
    frame. Bake from the highest-detail geometry.
  - Reuse a mirrored stamp for the other side only where the geometry is genuinely mirrored.
  - The stamp is presentation data.
- **The stamp source is the client's conformed belt view,** which every windowed client runs for every tank.
  It is not the simulation's contact readout, which covers only the local tank and lasts one tick.
  - Each grounded shoe is stamped at its world pose.
  - A shoe is stamped again only after it moves more than half a texel. Rolling then lays about one stamp
    per pitch per side, and slips and pivots smear by the simulation's own motion.
- **Prints are client-local.** No wire change, no protocol bump, and the server builds nothing. A late
  joiner misses earlier prints; that is accepted.
- **Stores follow §4 of the approaches note:**
  - L0: near the camera, toroidal, ~96 m across with ~4.7 cm texels, redrawn from the log when it
    recentres;
  - a world layer over the whole map;
  - a spatially indexed CPU stamp log.

  L1, for the gunner's optic, is deferred. The sizes are the note's DERIVED starting points: change them if
  you find a reason, and report it.
- **Each ground pack answers a print its own way, from data.** Each pack gets a small response record:
  print depth and appearance at least (darkening, roughness).
  - The shader applies it through the mask weights it already holds, so the stamper needs no ground sensor.
  - Starting values come from the ground-stamping note's §2 table. Label them ASSUMED, with provenance.
  - Berms, recovery and slip-specific looks belong to later slices.
- **Geometry and the simulation are untouched (ADR-0014).** That covers the ground's geometry, its collider,
  the oracle and the simulation.
- **The new texture binding shares an existing sampler.** Metal allows 16 samplers per stage (approaches note
  §2).

## Build

1. **The underside bake,** a pure, testable function: mesh triangles in, a height-and-coverage grid out,
   with bounds and frame metadata.
2. **The stamper:** grounded shoes from the conformed belt become log entries under the half-texel rule. A
   teleport or respawn resets a tank's history.
3. **The render-world stamping job.** The precedent is `src/damage_cam/pen_cam/picture.rs`. The job also
   covers the L0 toroidal update, with its recentre redraw from the log, and the world layer.
4. **`assets/shaders/terrain_blend.wgsl`:**
   - darken and roughen from the world layer everywhere;
   - inside L0, build a normal from the depth (surface gradient).
5. **Verify every Bevy and wgpu API** against `docs.rs/bevy/0.19.0`, the `v0.19.0` tag, or `vendor/`.
   Don't write engine code from memory.

## Tests

Each test names the invariant it protects:

- **the bake:** a synthetic shoe with a grouser prints the grouser deepest, and an opening stays uncovered;
- **the stamper:** rolling without slip lays one stamp per shoe per pitch, and a pivot stamps every grounded
  shoe again;
- **the recentre:** an L0 recentre, away and back, reproduces the texels of a fresh redraw from the log;
- **ADR-0014:** the dedicated server builds none of it;
- **ADR-0034:** no tank name in any law.

## A look for the owner

Use the capture harness only if it can render with a **hidden window** (`visible: false`). Never take the
screen; a crash dialog counts.

- **Shots:** a straight drive and a pivot turn, on two contrasting packs (for example mud and sand), at about
  10 m and about 30 m, under the game's sun.
- **Where:** save the PNGs as `.agents/scratch/impact-feedback-2026-10-28/track-prints-*.png`.
- **If driving can't be scripted,** feed the stamper a scripted path.
- **If no capture is possible,** say so.

GPU timings from a hidden window are not valid, because the window is occluded. Report no GPU cost as
MEASURED.

## Rules

- **Don't open** `shape-law.md`, `tiger-ammo-history.md` or `burst-delay-history.md`.
- **Compile only** via `scripts/serial-cargo … --features dev_ui,bitprobe`. Format with
  `rustfmt --edition 2024 <file>`.
- **Before committing,** run `cargo fmt --all --check`, CI clippy
  (`scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`), and the
  targeted tests for what you touched.
- **Tanks are data** (ADR-0034).
- **Comments** state the current invariant, never edit history.
- **Label numbers** MEASURED, DERIVED or ASSUMED.
- **Headless only.** Never print `~/.config/overmatch/*`, `.git/config` or `.lfsconfig` values.
- **Spawn no subagents,** and don't run `codex`.
- **Commit** with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't push.
- **Before you end your turn,** commit everything and leave no live shell.
- **Don't ask questions:** decide, and state the decision in your report.

## Report

No more than 15 lines:

- the commits;
- the stamp format and bake cost (MEASURED on the CPU);
- the store sizes;
- the per-pack response values;
- the test list;
- the capture paths, or why there are none;
- the gate results;
- the decisions you made.
