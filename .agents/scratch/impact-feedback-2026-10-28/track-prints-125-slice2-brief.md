# Brief: track prints, slice 2: prints that read as depth (#125)

You are an implementer lane for Overmatch, a Bevy 0.19 / Rust 1.98 video game. You work in a leased bay
(your worktree) and commit on its `lane/…` branch. Read `AGENTS.md` first; it is the rulebook. The subject
is cosmetic terrain marks, view only, and not the damage model.

## Read first

- The slice-1 brief, `track-prints-125-brief.md`, and its code on main: `src/track_prints/`,
  `src/terrain_blend.rs` and `assets/shaders/terrain_blend.wgsl`.
- `track-prints-approaches.md` §4: POM, the sun march, and the belt float.
- `track-prints-ground-stamping.md` §1 and §4.

All three notes are in `.agents/scratch/impact-feedback-2026-10-28/`.

## What the owner saw in play

- **Too shallow.** On the base ground, which covers most of the map, prints are nearly invisible. A
  57-tonne machine should print clearly on this soil.
- **Links pop in.** Each link appears fully pressed the moment it is stamped. At slow speed it should press
  in gradually.
- **Flat at low angles.** No parallax yet.

## Decided (fixed)

Don't reverse these. If you disagree, flag it in your report.

1. **Grousers bite fully into soil.** On soil the grousers sink all the way in, and the plate sinks by the
   pack's depth on top of that. The load concentrates on the grousers until the plate bears, which is why
   the slice-1 rule printed only grouser tips.
   - Each pack gets a bite share in data: 1 on soil, 0 on rigid ground such as road or rock.
   - Raise the firm packs' starting depths so the full footprint reads clearly at 10–30 m. Label every
     value ASSUMED, with provenance.
2. **Live tuning in the offline game.** The owner tunes each pack's print response by eye while playing,
   through whatever the repo already supports: a `dev_ui` panel or hot-reloaded data. The chosen values
   persist in data.
3. **Gradual press.** A link's print deepens with its load.
   - Its depth ramps from zero at touchdown to full once the first road wheel has rolled over it, using the
     view's real road-wheel positions.
   - The link is re-stamped as the ramp grows, and the store keeps the deepest value, so a print only ever
     deepens.
   - Keep stamp counts bounded at speed and under pivot, and report the MEASURED rates as slice 1 did.
4. **Parallax and the sun march** inside the near store, per approaches note §4:
   - offset the UV and the world position before `pbr_input_from_standard_material`, so every pack shifts
     together;
   - use `textureSampleLevel` inside the loops;
   - adapt the step count at grazing angles;
   - march 4–8 taps toward the fixed sun for the shadow inside the rut;
   - fade to normal-only by about 50 m (ASSUMED).
5. **A view-only belt sink.** The conformed belt reads the near store's CPU mirror, so the track doesn't
   hover over its own deep rut (approaches note §4, "Belts"). The simulation, the oracle and the collider
   stay untouched (ADR-0014).
6. **Frame cost.** The review measured the stamper's CPU work at 30 pivoting tanks at 4.25–6.22 ms mean and
   5.12–8.29 ms p95 per frame, in the edit profile. See `track-prints-125-review.md` and its ignored bench
   `thirty_tank_pivot_cpu_and_upload_measurement`.
   - **Target:** bring the dispersed layout to at most 1 ms mean and 2 ms p95, with the gradual press
     included (an ASSUMED budget), and report both layouts MEASURED on the same bench.
   - **Allowed:** you may reverse slice 1's CPU-raster decision and stamp on the GPU instead, per approaches
     note §4, if that is what it takes. Culling (for example, far tanks feed only the world layer) is also
     yours to choose.
7. **A drivable start for the owner.** Add a dev-only way to start a normal, drivable offline game at a
   chosen spot, for example a spawn-at environment variable, so the owner can drive on mud or sand by hand.
   The screenshot demo stays a separate script.

## Tests

Each test names the invariant it protects:

- with bite 1, a firm pack prints the full footprint; with bite 0, it prints no depth;
- a pressed link's depth never decreases while its load ramp grows;
- stamp counts stay bounded at speed;
- the belt sink reads the same depth the shader draws;
- the server builds none of it (ADR-0014);
- no tank name appears in any law (ADR-0034).

## A look for the owner

Hidden window only (`visible: false`). Never take the screen; a crash dialog counts.

Capture three spots, saving each PNG as `.agents/scratch/impact-feedback-2026-10-28/track-prints-s2-*.png`:
- mud;
- sand;
- a spot where the base pack dominates.

For each spot, take:
- a low-angle shot of a straight drive and a pivot at about 10 m;
- three frames of the gradual press at slow speed.

Report no GPU cost as MEASURED, because a hidden window is occluded.

Give the owner a launch command for the drivable start.

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
- **Commit** with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't push.
- **Before you end your turn,** commit everything and leave no live shell.
- **Don't ask questions:** decide, and state the decision in your report.

## Report

No more than 15 lines:

- the commits;
- the bite and depth values;
- the live-tuning route;
- the press ramp and the MEASURED stamp rates;
- the POM step counts and fade;
- the belt sink;
- the drivable-start command;
- the test list;
- the capture paths;
- the gate results;
- the decisions you made.
