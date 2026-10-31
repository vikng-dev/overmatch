# Boundary-fix lane review

Subject: Overmatch, a video game; discussion of damage, crew and rounds concerns only the game's
damage model and game state. Crew are HP pools at stations; the terms in AGENTS.md apply.

## Sweep-load checkpoint

Reviewed every hunk of `29614433..7e073952` and the additional sweep-load commits already present
in this bay (`711792b4`, `a8405f29`, `46253a1b`), against correctness and all three review lenses.
The bay started clean on `lane/boundary-fix-1101` at `46253a1b`; those existing commits are retained.
No subagents, captures, pricing runs or damage-topic web searches were used.

No P1 finding. P2 findings fixed before the boundary items:

- Error handling: `spec::duration` accepted finite delays outside `Duration` or the weapon gate's
  tick-count representation. The load gate now refuses both, naming the authored field. The
  existing reload/swap mutation battery includes these cases. The tick domain uses the default
  fixed timestep, the same timestep the game's compositions use.
- Base truths: `tools::buildings::tests::a_house_the_generator_cannot_lay_out_is_refused` assumed
  that every shipped house had east-wall openings. Its mutations now start from a labelled,
  constructed opening on each house, so a valid house without openings also exercises the gate.

P3 findings retained for the owner:

- `settings::store::location` still links the deleted `reset` function in its doc.
- `TankSpec::capabilities` says `{}` gates no capability, but `validate` requires every capability
  to be keyed; `[]` is the ungated requirement.
- The recoil-spring test's rationale was attached to the adjacent primary-count test in the last
  sweep-load commit; each test should carry its own rationale.
- `assets::asset_root`'s error tells the operator to set `BEVY_ASSET_ROOT` as a bypass, but it still
  calls `current_exe` before resolving that override. Its deliberate refusal without an executable
  path is preserved; the remedy in the diagnostic should match that behavior.
- `assets::tests::the_selected_tank_derives_its_trio_paths` pins the selected vehicle's literal
  paths. The fixture is labelled, but a selection change should ideally exercise the generic
  path relation instead of requiring new expected strings.

Code branching: no new P1/P2 finding. The removed switches have no remaining live callers.
The two known walk-test failures belong to `walk-w3` and are excluded from this review.

## Boundary items

Implementation decisions, verification and evidence are recorded below as each item lands.
