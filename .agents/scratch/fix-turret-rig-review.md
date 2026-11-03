# Turret rig fix review

Subject: the game's tank construction and damage model; game state only.

Decision: `turret.node` identifies the detachable construction; required `gunner_yaw` identifies its independent Gunner actuator. `GunnerYaw` is resolved in the complete spawn flush. A Gunner view must name a Pitch row below the declared Yaw. `turret.node: None` is refused before play; unarmed tooling explicitly declares its construction and has `gunner_yaw: None`.

| Reviewer | Finding | Disposition |
|---|---|---|
| Both | Structural assembly used as the aim servo; incomplete load-07 closure | Verified. Separate spawn handles, role/ancestry admission, optic working frame and public angle lookup. Parent-local targets prevent a nested sight repeating its parent's yaw. |
| Claude | An absent assembly certifies, then fails complete spawn | Verified and fixed at spec admission. Both Rust and Blender fixtures declare a root. |
| Claude | Powered rows inherit the turret box's role-selected inertia | Verified, **not fixed**. The required lookup is inside reserved `drive_servos`; no unused replacement field or guessed moving-load fact was added. |
| Claude | Assembly body facts bypass provenance admission | Verified and fixed with `Sourced`, field-specific diagnostics, and shipped-policy tests. |
| Claude | Post-cut clutch test cannot distinguish its claimed relation | Verified; removed. An undersized slipping clutch is legal, so no new clutch-sizing restriction was invented. |
| Claude | Axis-inertia refusal has no test | Verified and fixed with independently constructed overflow/underflow witnesses for both evaluated axes. |
| Claude | Travel refusal quotes runtime radians | Verified and fixed; errors quote authored degrees. |
| Claude | Launch-scale and Trigger documentation conflate the assembly and yaw | Verified and fixed. |
| Codex | Canonical reference fixture omits Assembly | Verified and fixed; the reference interface pin includes Assembly and the separate yaw declaration. |

All fixes are in **tank: separate assembly and Gunner yaw facts (#257)**.

Additional verified fixes: corrupt controlled hull poses fail before a new pick can preserve stale memory; the finite sky-point stand-in is required labelled data, and participates in the handshake. The retired aim-scan implementation and empirical optic coverage floor are removed. Detachment stops descendant aim controllers and preserves held poses for existing and late views.

Ratchets closed: ballistics-17, -18, -19, -21, -22, -26, -27 and view-20. The production capability forwarder is removed; its remaining cfg(test) helper serves repeated fixture construction. Complete-path rates retain their generic law tests.

`load-03` and `load-09` were already closed: shipped-file provenance refusal and the origin-axle witness remain verified. `load-08` remains: wheel station-to-solid membership needs its own construction-data and consumer migration.

Remaining handoff: each Powered row needs its own sourced axial moving-load moment, with `StandIn` for an unestablished load model. Replace the role-indexed `ServoMechanism.inertia` lookup with that row fact in the law lane. Both reserved functions, `drive_servos` and `advance_servo`, remain unchanged.

Tank #2: offset sights and separate/two-part structural roots no longer determine actuator identity; nested yaw commands and optic bearings use their actual rest frame. A powered cupola is **not yet fully supported** by the load model. The existing public stream also carries only the selected yaw/pitch pair; additional independently moving remote axes require a separate wire-contract extension.

Verification:
- `cargo fmt --all --check` passed.
- Broad serial-cargo run: MEASURED 780 passed, 10 existing ignores, including aim, sight, command, spec, tank, damage, drivetrain supply, net, headless, pen_cam, field_casts and whole-tree guards.
- MEASURED 512 controlled before/after task-rate evaluations: zero bit differences. The temporary reference code is removed.
- Final all-targets CI clippy passed after removal of the temporary comparison and restoration of the existing Requirement API.
- Post-cleanup cached test-binary run: MEASURED 235 passed, including targeted checks and the whole-tree source guards; the temporary comparison was excluded. Final source was compiled by all-targets CI clippy.
- Bevy APIs verified against local 0.19.1 source and Avian against local 0.7.0 source. Python fixture syntax and embedded sheet admission passed.
- The full regression request waited on the shared machine queue; the broad run completed at the lane cutoff. Closing lint and the post-cleanup checks waited beyond that cutoff; no further feature work was started. Both checks completed successfully.

No captures, pricing runs, screen-taking binaries, issue comments, pushes, merges or releases.

