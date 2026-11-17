# Red CI fixture repair

Subject: Overmatch's game damage model and wreck physics, game state only.

Reproduced on `a2cef3d1` using the requested bake and rolling-fade commands. Observed:
`bake::tests`: 13 passed, 22 failed; rolling-fade: 0 passed, 1 failed.

Every bake failure below has the same cause: `bake::fixture::spec_with` declares both
vehicle and assembly mass as Fixture 1 kg, so `TankSpec::validate` refuses it at `L2.SPEC`.
The shared fix declares Fixture 2 kg total / 1 kg assembly, and a Fixture (20, 40, 20) m
inertia box enclosing the synthetic solids deliberately separated for role checks. The
old claim that all fixture values were the smallest accepted values was removed.
No pass condition changed.

## Each reproduced bake failure

All symbols are in `bake::tests`; locations refer to the reproduction's source.
Each row uses the shared fixture fix above.

| Test | Panic line in `src/bake.rs` | Actual symptom |
|---|---|---|
| `a_collider_whose_substance_states_no_friction_is_refused_at_load` | 2748 | Finding names the sheet instead of the collider `Proxy`. |
| `a_component_of_several_substances_is_refused` | 2979 | Single-substance control receives `L2.SPEC` before the mixed-substance mutant. |
| `a_coordinate_outside_the_certified_range_is_refused` | 3194 | Receives `L2.SPEC` instead of `["L2.CERTIFIED_RANGE"]`. |
| `a_node_that_cannot_play_its_declared_role_is_refused` | 2789 | Receives `L2.SPEC` instead of `["L2.ROLE_COHERENCE"]`. |
| `a_primitive_the_sim_cannot_read_is_refused` | 3072 | Receives `L2.SPEC` instead of `["L2.DOCUMENT"]`. |
| `a_scaled_node_the_sim_composes_through_is_refused` | 3112 | Receives `L2.SPEC` instead of `L2.UNIT_SCALE` findings. |
| `a_shell_that_passes_through_itself_is_refused` | 3446 | Receives `L2.SPEC` instead of `["L2.SHELL_EMBEDDING"]`. |
| `a_station_can_carry_its_declared_solids_on_children` | 2767 | Child-solid control receives `L2.SPEC`. |
| `a_volume_wearing_an_unworn_substance_is_refused` | 3011 | Receives `L2.SPEC` instead of `["L2.ROLE_COHERENCE"]`. |
| `a_zero_area_face_is_refused` | 3259 | Receives `L2.SPEC` instead of `["L2.EXACT_DEGENERACY"]`. |
| `an_unreadable_sheet_or_an_unresolved_reference_is_refused` | 2547 | Missing-reference case receives the mass refusal instead of naming the missing node. |
| `armour_is_what_the_sheet_declares_not_what_a_material_implies` | 3036 | Undeclared-decoration control receives `L2.SPEC`. |
| `assembly_root_does_not_follow_optics` | 1971 | Extraction unwrap receives `L2.SPEC` before optics are changed. |
| `certification_separates_assembly_from_the_gunner_chain` | 2033 | Independent construction certification receives `L2.SPEC`. |
| `crew_and_witness_substance_come_together_or_not_at_all` | 2896 | Matching crew-station and witness control receives `L2.SPEC`. |
| `every_shipped_asset_passes_the_consumer_contract` | 2389 | Shipped loop passes; the synthetic origin-axle control receives `L2.SPEC`. |
| `open_inverted_and_doubled_shells_are_refused` | 3363 | Receives `L2.SPEC` instead of `["L2.MANIFOLD_WINDING"]`. |
| `shipped_file_readers_refuse_fixture_provenance` | 2179 | Fixture-policy restoration control is nonempty at line 2179; provenance refusals already passed. |
| `the_asset_door_refuses_a_file_below_the_crew_floor` | 2579 | Below-floor sheet receives the mass refusal instead of the crew-floor refusal. |
| `the_canon_file_carries_the_reference_list_and_the_registry_keys` | 2671 | `spec_validates` is false instead of true; reference and registry lists already passed. |
| `the_registry_the_door_supplies_is_the_one_the_contract_reads` | 2228 | Supplied-registry control receives `L2.SPEC`. |
| `the_report_is_deterministic_and_carries_every_field` | 3531 | Receives `L2.SPEC` instead of `["L2.MANIFOLD_WINDING", "L2.UNIT_SCALE", "L2.UNIT_SCALE"]`. |

## Rolling-fade failure

`track::sim::tests::a_pressure_heavy_construction_stalls_in_the_rolling_fade` panics at
`src/track/sim.rs:3122` in `spec.validate().unwrap()`. The grip and engine cases complete;
the launch case scales only total mass. DERIVED from its Fixture pressure ratio, the
launch vehicle is about 7,990 kg, less than its inherited 8,000 kg assembly. The fix
scales the assembly by the same density ratio at the pressure-fixture construction site,
keeping geometry and hull/assembly mass shares fixed. Driving assertions are unchanged.

## Shipped data and production code

Discovery found one shipped sheet, `assets/tiger_1/tiger_1.tank.ron`: declared total
57,000 kg, full box (3, 2, 6.3) m; declared assembly 8,000 kg, box (3, 1.2, 2.4) m.
These are authored data, not new measurements. No second shipped sheet was found.
The shipped consumer loop passed before its synthetic origin-axle control failed.
No shipped-data or production-code defect was found; both new load gates remain intact.

## Validation

- `scripts/serial-cargo test --locked --lib --features dev_ui,bitprobe bake::`: 41 passed, 0 failed (assets workflow consumer test set).
- `scripts/serial-cargo test --locked --lib --features dev_ui,bitprobe track::sim::tests::a_pressure_heavy_construction_stalls_in_the_rolling_fade`: 1 passed, 0 failed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe`, run once after repair, detached:
  `test result: ok. 2851 passed; 0 failed; 50 ignored; 0 measured; 0 filtered out; finished in 347.75s`
- `scripts/serial-cargo test --locked --workspace --exclude overmatch --features overmatch/dev_ui,overmatch/bitprobe`:
  `test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s`
- `cargo fmt --all --check`: passed.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed.

The CI workspace test set was split into the full game library and the remaining workspace
member, `overmatch-tools`, to avoid repeating the game suite. Both manifests disable binary
test harnesses and doctests, so this covers the same Rust tests as CI's workspace command.
The first tools-only invocation with unqualified `dev_ui,bitprobe` was refused by Cargo
before compilation; qualifying the dependency's features enables the required game feature set.
The ignored library tests remain ignored by the same default selection CI uses.

Read-only review against the six lenses found no new finding in the changed items. The
lens backlog names neither changed item. No new test was needed: the reproduced failures
are the regression witnesses, and their assertions remain unchanged. The orchestrator's
independent lens review remains the merge gate.
