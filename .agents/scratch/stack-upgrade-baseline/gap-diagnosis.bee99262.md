# Baseline gap diagnosis at bee99262

Subject: the game's driving physics and damage-model presentation; game state only.
Temporary instrumentation was removed before the lane's code commit. Runtime figures below are
MEASURED; source conclusions are DERIVED. No production physics or recorder behavior was changed.

## Belt probe

Reproduction:

```sh
env OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  BELT_FRICTION_ONLY=house-pivot scripts/serial-cargo test --lib \
  --features dev_ui,bitprobe belt_friction_probe -- --ignored --nocapture
```

It panicked at `belt_friction.rs:178:27`, reading retired body `2202v0`. A temporary observer of
`construction::breakage::RetirePiece`, installed after the case's bed settled, printed:

```text
[DEBUG-baseline-gaps] retire 2202v0 y=0.8729292 split=true
```

`split=true` is `Has<PendingSplit>`: this is parent replacement by `split_groups`, not floor
retirement or a freeze. The change `937d1628` introduced landing-driven crumbling through
`PendingSplit`; it uses the parent-replacement path added in `00386404`. The current path is
`debris::crumble::crumble_landings` -> `construction::breakage::split_groups` in
`FixedPostUpdate`. The probe reads after the whole tick, whereas `ContactTransfers` records the
completed physics step: a contact's former body need not still exist. Production retirement is
legitimate; the probe's lifetime assumption was wrong.

The focused regression `a_retired_piece_ends_its_probe_track` failed at the same entity read before
the fix. The repaired probe reports absent contact bodies, ends retired bodies' motion series,
and requires pose/velocity on bodies that still exist; it fabricates neither stationary samples
nor replacement values. Its numerical table remains observations.

## Slice capture

The corner opens for a presented `Audience::Shooter` explanation addressed to the player's current
combatant, striking another combatant, whose verdict the corner setting admits
(`damage_cam/pen_cam/game.rs::drive`). Offline delivery uses `receive_drawn`, and the shipped
corner setting is `All`. `NonPen` selects `Slice`; automatic fire creates no records
(`recorder::ShotRecording::records`), and a main-gun penetration selects `Glass` instead.
The visible-window requirement in `run-pencam-cost.sh` is its frame-cost measurement contract.

A hidden, muted run with an empty config directory, `OVERMATCH_TARGET=ahead SPIKE_AUTO_FIRE=1`,
UI enabled and a fixed 0.015625 s capture step reproduced the unopened corner. Temporary capture
and recorder diagnostics showed the target still in action before the main-gun shot, then knocked
out, with no explanation delivered:

```text
fire single shot=Some(ShotId { combatant: CombatantId(1), weapon: 2, fire_tick: 3838 }) recorder=true
note recorded shell=8796v38 events=[(0, 7561v0), (1, 7561v0), (2, 7561v0)]
finish open=1
recorded tank=7561v0 frames=[(0, true), (1, false), (2, true)] shot=Some(ShotId { combatant: CombatantId(1), weapon: 2, fire_tick: 3838 })
t=61 stored=0 tanks=[(CombatantId(1), true, false), (CombatantId(2), false, true)]
```

Here each frame boolean means `Option<HullFrame>::is_some()`. `MarchRecorder::note` keeps the
missing interior frame; `pose_struck` fills only a missing final entry; `recorded` collects all
frames through `Option`, so the interior absence discards the entire record before report
construction. The free-flight witness-line writer `ballistics::wound_legs` appends a `Wound` mark
without stamping `marks.frames`; playback's own mark writer stamps frames. This is a production
recording defect, independent of the corner's window/focus or setting. It remains diagnosed,
unchanged: the writer needs the contact-time hull frame, rather than a guessed later pose.

The capture-only solution is `scripts/capture/pen-cam-slice.ron`: its offline cue replays the existing
simulated `s8b_mg_stopped.ron` report through normal projection, shooter storage and corner admission.
It applies no dose, changes no simulated HP pool, and is mounted only by `run_offline`.
UI remains enabled because the inset is an image node. This is shader evidence from a recorded
report, not evidence that the live record generator is repaired. The hidden preview logged
`the slot shows Slice on Corner at t=10.203 s (Time<Real>)`; visual inspection confirmed the
hatched plate cut and stopped path. No visible window is required.
