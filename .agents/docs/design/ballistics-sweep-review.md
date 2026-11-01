# Ballistics sweep review

Subject: the game's ballistics and damage model, game state only. Crew are HP pools at stations;
fields deposit dose, and a pool emptied leaves its component knocked out.

## Review of sweep-ball-1

Reviewed the commits after `ff571fe0` through `2cff0cf5` against the error-handling, code-branching
and base-truths lenses and the sweep brief. No earlier ballistics sweep record was present in
`.agents/docs/design/` at this base.

### Standards findings

- **B1 remains P1:** the drag parameters in `round_types.ron` are bit-preserving and honestly
  labelled `Tuned`, including the original calibration and absence of an aerodynamic source.
  This makes the assumption visible; it does not supply the missing aerodynamic evidence. The
  universal relationship between the hit-nose factor and aerodynamic drag remains an assumption.
  Do not call B1 fixed or replace it with another unsupported coefficient.
- **R1, P2, fixed:** `fuzz::probe_rounds` used `round_by_key`, whose uniqueness requirement the gun
  registry does not establish. Another gun chambering an existing round would panic the
  instrument, and the stowage class-invariant test would reject that valid registry. Enumerate
  every `(gun, round)` configuration at its own muzzle speed; reports retain both identities.
  The synthetic shared-ammunition test exercises distinct speeds through the same round key.
- **P3:** `ShellVisual` still describes a calibre classification the renderer no longer uses.
  `ShellReadout` still duplicates projectile state for test-only readers. The fuzzer still names
  one tank in its target/report. The gun parser validates then discards `names` and `caliber`.
- **P3 / B11 follow-up:** the pen cam still loads one scene and scales it by record calibre. Its
  record already carries a round key on the wire; the presentation can use that identity to read
  authored body metadata without adding another protocol field.

The other reviewed changes introduce no further P1/P2 findings: authored body and streak presence
reaches world rendering; model metadata is checked against glb bounds and flying diameter; the
replacement registry/body tests express class invariants; the damage queries now assert their
spawn/relationship invariants; member coefficients are refused outside their fractional domain.
The codegen workflows use one feature set and cache family; the asset door still runs the Rust
bake consumer it verifies. The disk sweep's size readout remains diagnostic while reap failures
remain visible.

### Spec review

The implemented registry, visual and error-handling portions match their brief, subject to R1
above. B1 is incomplete, as are the explicitly handed-off pen-cam and impact consumers. The visual
model declarations are authored stand-ins, not a claim of measured round-specific shape.

### Verification

R1: `cargo fmt --all --check`, workspace/all-target clippy with `dev_ui,bitprobe`, and the targeted
`guns::`, `ballistics::fuzz::`, `doc_citations` tests passed. MEASURED: 17 passed, one ignored
on-demand instrument, no failures. No captures or pricing runs.
