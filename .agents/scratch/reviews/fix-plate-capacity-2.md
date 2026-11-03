# Plate-capacity fix review

Subject: the game’s damage model, game state only. Crew are HP pools at stations.

Commit subject: `ballistics: decouple fuze arming from plate work (#263)`. No finding was rejected as not a defect. The welded-share defect is verified but deliberately remains unresolved under the brief’s missing-construction-data stop rule.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude P1 | Arming followed charged work | Verified: the normal 36 mm Fixture plate failed to arm; the two-calibre march regression now passes. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P2 | Welded run sets the plate share | Verified red: MEASURED touching share 1.0 versus gapped 0.49 for the steel/soft-box Fixture. Bonded-plate membership and interface/load-transfer data are absent. | Unfixed by instruction; missing construction fact documented in §3b and this commit. |
| Claude P2; Codex P3 | s16 caption assigns arming to the wheel incorrectly | Verified against the original charged exit readouts. Independent plate-met arming changes the shot; its caption and record now agree. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P2 | StandIn labels and which rows fit each parameter | Verified: the return magnitude fits one upper row, while any positive exponent joins. Labels now name the missing opening-work law and replacement. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P2 | Unsupported near-calibre return interval | Verified. §3b includes the interior instrument readings and unsupported band; the endpoint-F crossing is x = 0.73044 DERIVED, correcting the review’s approximate 0.72. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P2 | Shared-pressure stop reason and two thickness definitions | Verified against the local candidate relation and recalculated pressures. The bulk mismatch is the blocker; geometric potency x and steel-equivalent skid x are distinguished. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | False at-rest accounts elsewhere in the design doc | Verified against the current record. The origin-clipped-field rule is generic; §5a points at the current side-burst account. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | Wire measured table and maximum message | Verified: rerunning encoded_sizes gives s16 three spall cones and 2082 B MEASURED. Every table row and its account are refreshed. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | s16 name and missing at-rest library lesson | Verified: renamed to s16_side_burst. No at-rest library scenario is claimed; the simulation and presentation fixtures cover that state separately. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | Inverse-limit test names a mutant it cannot catch | Verified by its manually composed law: omitting the production thin share does not reach this test. Its comment now names its actual scope; the driver proof owns wiring. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | REV 81 reason claims client derivation | Verified: replicas consume authority-exported paths. Revision 81 remains a compatibility checkpoint; current PROTOCOL_REV stays 82. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | resistance_charge bypasses its WalkLaws | Verified red with two accepted Fixture fits. PlateCapacity now travels in WalkLaws and the supplied-law regression passes. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | Live field count silently truncates | Verified red at both atlas and shader packing, one field beyond the shared budget. Plan admission and direct packing now fail explicitly. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | Lane test counts, wall clock and lock handling in §3b | Verified edit history. Removed from the design narrative; permanent contracts and evidence remain. | ballistics: decouple fuze arming from plate work (#263) |
| Claude P3 | mechanism, fuzz and Plate readout prose | Verified: bulk reference-mm is not a geometric defeat thickness, and the chord includes both work shares. Corrected at each owner. | ballistics: decouple fuze arming from plate work (#263) |
| Codex P3 | Unarmed fixture comment and hardcoded threshold assertion | Verified: the charged fixture threshold had changed. The fixture now derives from real chord; its assertion reads its own trigger and material factor. | ballistics: decouple fuze arming from plate work (#263) |
| Codex P3 | Engine dose conflated with capped HP loss | Verified from both actual records: line 267.97104 → 362.68326 HP, field 268.85187 → 234.678 HP, loss 336000 pool units MEASURED. §3b distinguishes them. | ballistics: decouple fuze arming from plate work (#263) |

The authoritative law, comparisons and unresolved construction requirement are in [design §3b](../../docs/design/armor-penetration-and-damage.md). No backlog row names an edited file.

## Player-visible result

A normal plate meets the same arming threshold across calibres; thin-share potency no longer moves that threshold. s16 now bursts before the transmission at 452.53116 m/s MEASURED, and its nose piece stops there. The other fifteen report payloads are identical; s15’s header wording also names plate met. The live view no longer hides a field past its budget.

## Executed red variants

- Original charged-work arming: the two-calibre march regression failed on the 36 mm Fixture plate, then passed after the fix.
- Original hidden global-law lookup: the supplied-law regression failed after adding the parameter seam, then passed when the production read used it.
- Original atlas and shader truncation: both overflow regressions failed because no panic occurred, then passed with the shared explicit refusal.
- Welded versus gapped Fixture: the equality diagnostic failed with MEASURED shares 1.0 and 0.49. It was removed because the missing construction truth prevents an honest fix. The existing thick-soft-body invariant stays.

These are executed pre-fix defect variants, not a claim that an additional post-fix synthetic mutation campaign ran.

## Decisions

- Arming integrates material-weighted real chord per reached body, independent of capacity and slope work shares. Charged work still controls speed and dose; the dead charged arming readout is removed.
- Keep the empirical curve and its values. Report its unsupported return interval instead of refitting it.
- Stop the welded-share item until bonded-plate membership, interfaces and load transfer are authored. Do not infer them from material strength, body role or weld tolerance.
- Keep the skid stand-in. The proposed pressure relation exists, but the bulk mismatch at one calibre remains.
- Carry a copy of the admitted capacity law through WalkLaws, matching the existing law-parameter seam.
- Treat field-budget overflow as an internal view-capacity bug: fail at plan construction or a direct packing entry point. Keep the current shared atlas/shader budget; add no clipping or second rendering path.
- Keep PROTOCOL_REV at 82 and correct the revision-81 note. Neither clients’ reading nor the wire layout changes.
- Rename the current scenario without a compatibility alias. Historical research tables retain their historical scenario name.
- Add no at-rest scenario or new aim: a_burst_draws_only_what_flies_on covers presentation, and an_armed_round_at_rest_bursts_where_it_rests covers simulation on generic fixtures.
- Make missing reached-sample membership and an empty reached-member maximum fail as invariant bugs instead of inventing zero.
- Keep the composed inverse-law test and correct its claimed mutant; keep production wiring coverage in the walk tests.

Additional stale prose verified while checking the edited scenario paragraph: §5a’s s13 account named the wrong emptied ammunition/station pools. It now follows the current aggregate record; no sim or asset parameter changes were needed.

## Gates

- Formatting and CI workspace/all-target clippy with --locked, dev_ui,bitprobe and -D warnings passed.
- Targeted ballistics, damage_cam, headless_test and whole-tree guards (doc_citations, ui_ascii, net_boundary, bake) passed: 684 passed, zero failed, 17 ignored tools/beds MEASURED.
- Final whole library passed: 2276 passed, zero failed, 31 ignored MEASURED. Doc-citation guards passed again after the final prose correction.
- Rerecording and the limit-speed/wire-size instruments passed. All builds and test binaries used serial-cargo’s machine lock with unattended, muted headless settings. No captures or pricing runs were taken.
- Production resolve.rs is untouched; the shared SpallSource field rename reaches its test fixture. This is the only resolve-side integration point for the parallel lane.
