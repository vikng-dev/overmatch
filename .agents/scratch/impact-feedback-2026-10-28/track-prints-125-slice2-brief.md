# Brief: track prints, slice 2: ground types, depth that reads, frame cost (#125)

You are an implementer lane for Overmatch, a Bevy 0.19 / Rust 1.98 video game. You work in a leased bay
(your worktree) and commit on its `lane/…` branch. Read `AGENTS.md` first; it is the rulebook. The subject
is cosmetic terrain marks, view only, and not the damage model.

Another builder is doing this same task independently in another bay, as a one-time comparison. Don't look
for its work and don't coordinate with it. Build your best version.

## Read first

- The slice-1 brief, `track-prints-125-brief.md`, and its code on main: `src/track_prints/`,
  `src/terrain_blend.rs`, `assets/shaders/terrain_blend.wgsl`.
- The review of slice 1, `track-prints-125-review.md`, including its ignored bench
  `thirty_tank_pivot_cpu_and_upload_measurement`.
- `track-prints-approaches.md` §4: POM, the sun march, and the belt float.

All of these are in `.agents/scratch/impact-feedback-2026-10-28/`.

## What the owner saw in play

- **Too shallow.** On the base ground, which covers most of the map, prints are nearly invisible.
- **Links pop in.** Each link appears fully pressed the moment it is stamped.
- **Flat at low angles.** No parallax yet.

## Decided (fixed)

Don't reverse these. If you disagree, flag it in your report.

1. **A ground-type table, from squishy mud to hard concrete, as data.**
   - Each ground type carries:
     - softness: print depth per unit of ground pressure;
     - a depth cap;
     - darkening;
     - a roughness change.
   - Each terrain pack names its ground type.
   - Later the table will also drive handling (grip, rolling resistance, sinking). Don't add those fields
     now, but give the table a shape that can grow them.
   - Starting values are ASSUMED, with provenance. Choose them so the full footprint reads clearly at
     10–30 m on today's four packs.
2. **Depth is a simple proxy: one number per tank per ground type.**
   - Depth = softness × the tank's ground pressure (mass over track footprint area, both read from tank
     data), capped by the ground type.
   - No per-contact load and no weight distribution.
   - The baked link shape stays, and supplies the pattern.
   - On any ground with a nonzero depth, the whole footprint reads, pattern included. Concrete prints no
     depth.
3. **Live tuning in the offline game.** The owner adjusts the ground-type values by eye while playing,
   through whatever the repo already supports: a `dev_ui` panel or hot-reloaded data. The chosen values
   persist in data.
4. **Press: fade in over one link.**
   - A link's depth ramps from 0 to 100% while the tank travels one link pitch after that link touched
     down.
   - Use a few re-stamps per link at most.
   - The store keeps the deepest value, so a print only ever deepens.
5. **Parallax and the sun march** inside the near store, per approaches note §4. They ship as a graphics
   setting in the existing display settings: lighting only, parallax, or parallax with the sun's shadow,
   plus the distance they're drawn to.
   - Offset the UV and the world position before `pbr_input_from_standard_material`.
   - Use `textureSampleLevel` inside the loops.
   - Adapt the step count at grazing angles.
   - Fade to lighting only at the set distance.
6. **Frame cost.** The review measured the stamper's CPU work at 4.25–6.22 ms mean and 5.12–8.29 ms p95
   per frame, with 30 pivoting tanks in the edit profile.
   - **Find what costs** and fix it.
   - **Target:** the dispersed layout at no more than 1 ms mean and 2 ms p95, press included (an ASSUMED
     budget). Report both layouts MEASURED on the same bench, before and after.
   - **Allowed:** you may stamp on the GPU instead, reversing slice 1's CPU-raster decision, if that is what
     it takes. You may simplify the review's history and world-layer machinery, keeping correctness on the
     recentre.
7. **A view-only belt sink.** The drawn track lowers into its own deep rut. The simulation, the oracle and
   the collider stay untouched (ADR-0014).
8. **A drivable start for the owner.** Add a dev-only way to start a normal, drivable offline game at a
   chosen spot, so the owner can drive on mud or sand by hand. The screenshot demo stays a separate script.

## Tests

Each test names the invariant it protects:

- higher ground pressure prints deeper on the same ground type, with no tank name in the test;
- concrete prints no depth;
- the press ramps monotonically over one pitch;
- stamp counts stay bounded at speed and under pivot;
- the belt sink reads the same depth the shader draws;
- the server builds none of it (ADR-0014);
- no tank name appears in any law (ADR-0034).

## A look for the owner

Hidden window only (`visible: false`). Never take the screen; a crash dialog counts.

Capture three spots, mud, sand and the base pack, at about 10 m: a low-angle shot of a straight drive and a
pivot at each setting level. Save each PNG under `.agents/scratch/impact-feedback-2026-10-28/` with a
prefix naming your bay, for example `bay1-track-prints-s2-*.png`. Report no GPU cost as MEASURED, because
a hidden window is occluded.

## Rules

- **Don't open** `shape-law.md`, `tiger-ammo-history.md` or `burst-delay-history.md`.
- **Compile only** via `scripts/serial-cargo … --features dev_ui,bitprobe`. Format with
  `rustfmt --edition 2024 <file>`.
- **Before committing,** run `cargo fmt --all --check`, CI clippy
  (`scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`), and the
  targeted tests for what you touched.
- **Verify every Bevy and wgpu API** against `docs.rs/bevy/0.19.0`, the `v0.19.0` tag or `vendor/`.
- **Tanks are data** (ADR-0034).
- **Comments** state the current invariant, never edit history.
- **Label numbers** MEASURED, DERIVED or ASSUMED.
- **Headless only.** Never print `~/.config/overmatch/*`, `.git/config` or `.lfsconfig` values.
- **Spawn no subagents,** and don't run `codex`.
- **Commit** with the `Co-Authored-By` trailer your dispatch prompt names. Don't push.
- **Before you end your turn,** commit everything and leave no live shell.
- **Don't ask questions:** decide, and state the decision in your report.

## Report

No more than 15 lines:

- the commits;
- the ground-type table and its values;
- the depth for the fixture tank on each pack (DERIVED);
- the live-tuning route;
- the press;
- the settings levels;
- the frame cost before and after (MEASURED);
- the belt sink;
- the drivable-start command;
- the tests;
- the captures;
- the gate results;
- the decisions you made.
