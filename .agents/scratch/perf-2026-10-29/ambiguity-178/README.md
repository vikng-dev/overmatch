# Presented-part audit (#178)

The game's presented tank parts were audited with Bevy ECS's schedule ambiguity detection.
The adjacent `ambiguities.txt` lists the relevant pairs from the production offline composition,
including the game's damage-model replay-view systems by their code identifiers.

## Finding

The claim that an unordered PostUpdate pose writer caused the reported wheel change was **not
substantiated**. The large difference is a LOD change. One unchanged binary, with the upload edge
present in both runs, produced different wheel meshes when its hidden primary window reported
different physical heights. The screenshots themselves were the same size.

MEASURED before the fix:

| Same binary, same upload edge | LOD height | Standing full-frame differing pixels | Moving full-frame differing pixels |
|---|---:|---:|---:|
| Ambiguity warnings enabled vs disabled | 1440 px vs 720 px | 54,545 | 84,817 |

MEASURED wheel/belt crop differences were 41,030 standing pixels and 38,331 moving pixels.
`compare.py /tmp/ambiguity-178-audit /tmp/ambiguity-178-repeat /tmp/ambiguity-178-before-proof`
reproduced the failure with a nonzero exit. The artifact panels under `before/` preserve it.
The fixture certificate's `Wheel_L#0` first deviation is MEASURED 6.976162 mm, with a
MEASURED 0.727982 m origin radius. At the capture's ASSUMED field and pixel budget, its first
switch is DERIVED 6.791 m for 720 px and 12.854 m for 1440 px. The near view therefore selects
different wheel geometry under these two heights; apparent hub changes need no pose writer.

The warning flag is a diagnostic, not a deterministic selector for either window size: later
runs of both arms reported the same height. This is a hidden-window/backing-size dependency,
not evidence that logging changes a wheel pose.

A temporary capture probe inspected wheel-local transforms and belt phases. Runs with equal LOD
height had identical wheel/belt crop pixels at both capture times. Their full-frame residuals
were confined above the running gear (MEASURED: 77 standing pixels, 124 moving pixels); the moving
wheel-local transforms matched exactly. The probe was removed from source.

## Pairs and dispositions

MEASURED production audit totals: 254 Update pairs and 192 PostUpdate pairs. The totals include
unrelated UI, rendering and exclusive-world-access conflicts. They are not cleanup targets.

New explicit orders:

- `ApplySettings -> track_view_facts`: the applied render scale/window mode must be fresh before
  the frame's LOD facts are measured. This transitively orders `ApplySettings ->
  compose_view_profile`, so the composed profile also consumes the current pixel budget.
- `compose_view_profile -> bind_link_template`: initial shoe switches must use the current
  composed profile, including a newly measured target height or applied pixel budget.

Already ordered and now regression-guarded:

- `drive_track_views -> mark_dirty_trees -> propagate_parent_transforms -> sync_simple_transforms`
  through `TrackViewSet.before(TransformSystems::Propagate)`.
- `drive_track_views -> upload_belt_frames` through `TrackViewSet -> BeltUpload`.
- `BeltUpload -> AssetEventSystems` remains shipped unchanged; its existing regression test guards it.

Bevy's `animate_targets` opts out of ambiguity detection (`ambiguous_with_all` in the
versioned engine source). The captured fixture GLB has MEASURED zero animation clips, and
this project registers no animation-player playback, so that opt-out supplies no hidden
wheel writer in this scene.

No new PostUpdate transform order was justified:

- `drive_track_views` vs `orbit_look`, `orbit_camera`, captive `place`, or demo `hold_camera`:
  the camera writers modify `PlayerView`; running gear writes the bound view-part entities and
  reads the root carrying `TrackRig`. These are separate entities. The camera's root read does
  not consume a running-gear write.
- `drive_track_views` vs `face_billboards` or `present_fall`: these write effect entities or
  tree-view nodes, not tank parts. The broad transform queries make them look conflicting.
- `drive_track_views` vs `run_egui_context_pass_loop_system`: exclusive World access is reported;
  the hidden scenario has no user-driven gizmo edit to a tank part.
- Update servo/barrel writers target `ViewServo` nodes or the barrel's `ViewNode`; the running
  gear binder reads sim roadwheels and the sprocket/idler mesh hierarchy. Their broad queries
  overlap on the component type but their actual part roles do not overlap.
- Update replay-stage and effect writers operate on their own view/stage/effect entities.
  `commit_aim` reads the sim hierarchy, while interpolation writes the separate servo view.
- `capture_turret_pivot` reads the sim hull, not an interpolated servo or recoil barrel.
- `spawn_tank_when_loaded` vs `compose_view_profile` is a cold-start deferred spawn in the optional
  LOD showcase; it waits for the profile to exist and does not rewrite a presented wheel.
- `track_view_facts` vs `observe_window_placement`: placement observes OS fullscreen/monitor
  state and reconciles the mode; it does not write the physical dimensions read by LOD facts.

The mask-copy `follow_pose` writer's additional pairs are in `mask-pose-ambiguities.txt`. It writes
mask-copy globals; belt selection and shoe stamping consume the source belt, camera/HUD readers
consume the player view, and light/audio systems consume their respective entities. Those are
separate from the mask-copy write set.

The exact pairs and conflicting resource/component names are in these lists; no broad
`ambiguous_with` suppression or arbitrary serialization was added.

## Fix and checks

An image-backed player view measures its image asset's physical height through the applied render
scale. It does not measure the hidden window behind it. A missing image holds the last measured
height, following the same absence rule as a missing window.

The image-target test uses ASSUMED fixture sizes and scale and changes the hidden window size
without changing the image. The settings-order test enables `LogLevel::Error`, so removing its
production order fails during schedule construction regardless of lucky executor ordering. The
production presentation audit enables `LogLevel::Warn` on both schedules and rejects unordered
profile/template or running-gear/propagation/upload pairs. It also checks the built topological
order, so reversing a required edge fails instead of merely silencing the ambiguity.

## Capture reproduction

Use `../track-belt-172/followup/capture.sh` with the bay's `target/debug/overmatch`, scene `near`.
It supplies the ASSUMED camera pose, fixture map, fixed virtual step and capture times, and runs
hidden, muted and offline under a single machine lock. No frame-time measurements were taken.
The final comparison removes only `.before(AssetEventSystems)` from `BeltUpload` locally for the
second build, then restores it. It does not reverse the edge: deliberately uploading after the
asset flush introduces an expected GPU-table lag and would test a different behavior.

MEASURED final parity (`stills/counts.json`): zero differing pixels across the full frame at
both the standing and moving capture times. This includes zero differing wheel/belt-crop pixels
and zero maximum channel delta. The side-by-side panels show shipped edge, locally removed edge,
and their amplified difference. The binary hashes and capture-log extracts are retained beside
this report. Both captures exited successfully; the local toggle is absent from the final source.

Final gates passed:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe view::tests`: MEASURED 54 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe dev_capture::tests`: MEASURED 6 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe a_changed_table_reaches_the_render_world_in_the_frame_it_changed`:
  MEASURED 1 passed.

Every cargo command used `OVERMATCH_CARGO_LOCK_WAIT_S=10800`; each game run used the capture
script's single `lockf`, mute and hidden-window settings. All started captures exited normally.
No push, main-branch merge, issue comment, external-repository action or frame-time measurement
was performed.
