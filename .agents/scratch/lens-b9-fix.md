# Lens b9 fix disposition

Subject: the video game's development capture plans, track sandbox and construction bake tooling; game state only.

All five brief findings were verified, accepted and fixed. None was rejected.

| Brief finding | Disposition |
|---|---|
| 1 — capture admission | `CapturePlan::validate` refuses unknown or ambiguous construction references before cues run. `Aim` parses and retains `Dir3`, rejecting zero, nonfinite, overflowing, underflowing and inaccurately normalized input. Startup binds named targets to entities; firing retains the checked direction. Diagnostics name the plan path, field and repair. A server plan targeting an offline preview is refused at startup. |
| 2 — sandbox admission | `Scene::validate` refuses nonfinite `rubble_wall_at_m`. `PileRow` admits only nonzero widths whose square fits `usize`, retaining the capacity for validation and spawning. |
| 3 — sandbox source | Required `rubble_construction` scene data owns the yard's source choice, with Taste provenance. The shipped data selects its current construction; the Rust asset-name constant is removed. |
| 4 — bake failures | `BakeError` distinguishes admission, filesystem and filename failures. Spec discovery, output comparison, directory creation and output writing retain their OS errors. The binary prints the contextual failure and returns an unsuccessful exit status. Admission still precedes output mutation. |
| 5 — preview borrowing | `place_preview` borrows the preview and construction library entries. DERIVED: the borrowing change removes one net source line, the preview allocation and temporary Arc clones. |

An additional touched-path defect was fixed: spec enumeration no longer converts non-UTF-8 filenames lossily. Its production admission function returns a typed refusal retaining the original name; the test exercises that function directly because the host filesystem refuses creating such a filename.

Decisions: capture target references must be unique; unavailable preview targets fail before any cue; sandbox construction selection has one required data owner. No owner decision or scoped work remains. No backlog row names the edited files.

Validation, all passed (test counts MEASURED):

- `cargo fmt --all --check` and workspace/all-targets CI clippy with `--locked --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- dev_capture doc_citations`: 26 checks.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- track_sandbox::data headless_test::track_sandbox_boots_headless doc_citations`: 8 checks. MEASURED headless boot: the current yard spawned its 78-chunk wall section and 108 loose pieces; these observations are not new test obligations.
- `scripts/serial-cargo test --workspace --lib --features dev_ui,bitprobe -- construction::tests::the_bake_entry construction::tests::a_shipped_spec_refuses_a_fixture_label construction::tests::spec_enumeration doc_citations ui_ascii net_boundary bake::`: 48 game guards and 5 tooling tests.

New engine API uses were checked against the locked Bevy 0.19.1 registry source. Every build/test ran through the machine queue with the requested features, unattended and headless. No captures or frame/tick pricing were taken. The full suite remains CI's gate.
