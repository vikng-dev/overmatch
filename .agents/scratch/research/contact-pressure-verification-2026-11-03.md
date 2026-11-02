# Contact-law review verification (#197, #260)

Subject: the game's damage model; game state only. Crew are HP pools at stations.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | F1: spent-round skid chain | Verified already fixed: friction-held rest is in `walk::begin` | ballistics: lay a round's own disc on every leg after a skid (#260) |
| Claude | F2: false final departure | Verified already fixed: recorder waits for rest or tank clearance | ballistics: lay a round's own disc on every leg after a skid (#260) |
| Codex | P2: entry force omits slope charge | Verified red under missing-charge mutation | ballistics: charge impact-fuze force along the real path (#197) |
| Claude | F3: cofacial strength follows mark owner | Verified red with both volume-key orders; strength now reads the first union segment | ballistics: read cofacial strength from the union field (#197) |
| Codex | P2: scalar oracle uses unit charge only | Verified red when the slope adapter is bypassed | ballistics: make contact regressions reject their named mutants (#197) |
| Codex | P2: soft/steel fixture differs in thickness | Verified red under material-blind contact with equal geometry | ballistics: make contact regressions reject their named mutants (#197) |
| Claude | F10: soft share is never strictly compared | Verified red under share deletion; mechanism and entrance tests now compare strictly | ballistics: make contact regressions reject their named mutants (#197) |
| Claude | F11: mounted test cannot reject a fixed angle | Verified claim mismatch; removed that claim, retained the independent pressure sweep | ballistics: make contact regressions reject their named mutants (#197) |
| Claude | F12: floor admission guard untested | Verified red with the finite-floor guard removed | ballistics: make contact regressions reject their named mutants (#197) |
| Codex | P2: production hypothetical probe numbers | Verified; exact registry-count test rejects the extra probes | ballistics: probe only shipped gun and round data (#197) |
| Claude | F4: unread mechanism Geometry | Verified by callers/implementations and deletion | ballistics: remove unused contact resolution seams (#197) |
| Claude | F5: local duplicate axis and directions | Verified for the bite axis argument, embedded direction and walk outcome direction | ballistics: remove unused contact resolution seams (#197) |
| Claude | F5: perforated resume direction always equals incoming | Not a defect: `Pending::world_decision` adds carrier velocity and rewrites this field; preserved | Preserved; documented in ballistics: remove unused contact resolution seams (#197) |
| Claude | F6: deleted bend/angle prose in source | Verified against straight transit | ballistics: state the current contact law and data jobs (#197) |
| Claude | F7: false DERIVED skid limits | Verified stale; replaced by the fixture's asserted first-contact-skid invariant | ballistics: state the current contact law and data jobs (#197) |
| Claude + Codex | F8 / P3: active design prose in sections 3, 9, 13.5 | Verified against the walk and carrier paths | ballistics: state the current contact law and data jobs (#197) |
| Claude | F9: Effect::Line chord described as real geometry | Verified charged readout; documentation now says as charged | ballistics: state the current contact law and data jobs (#197) |
| Claude | F13: floor_m labelled Unit | Verified: it bounds flight, not balance scale; now StandIn for missing map-authored vertical bounds | ballistics: state the current contact law and data jobs (#197) |
| Claude | F14: fixture called capped-steel class | Verified against shipped preset data; now independent fixture data | ballistics: state the current contact law and data jobs (#197) |
| Claude + Codex | F16 / P3: stale compatibility identifiers | Verified; report now points to the owning protocol constants | ballistics: state the current contact law and data jobs (#197) |
| Claude | F15: unused report prints and prefix-contract violation | Verified no tracked script readers; both extra prints deleted | ballistics: remove unconsumed contact report prints (#197) |
| Claude | F16 / backlog net-04 | Verified open defect: out-of-range disclosed index logs refusal but presents index zero | Reported, left open as authorized; fire_presentation.rs unchanged |
| Claude | Footprint P1: Held main armour fails authority delivery | Verified red through the shipped offline delivery plugin: `Path("the resting volume must be a component or retaining plate")`; Held armour now supplies a zero-chord retaining plate | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude + Codex | Footprint P1: skid exchanges with no clock | Verified red in the converging wedge at 10 degrees; the replacement checks positive travel or terminal rest at both fixture angles, without a decision-count allowance | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Codex | Footprint P1: the infinite plane acts before a finite rim clip | Verified in `skid_turn`: its read-plane turn can be zero while finite contact remains ahead; the finite-contact fixture requires that forward travel | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P2: unbounded negative step-back | Verified red under the original turn formula: the fixture returned -0.40199503 m (MEASURED); turns never retrace a paid crossing | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P2: future strike and false out-and-back record clock | Verified: old aggregate pulls the initial contact 0.75 m ahead, and the record-clock mutant ends the axis at its 1.4 m offset strike (MEASURED fixture outputs); contact and axis now have separate owners | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude + Codex | Footprint P2: Held reports Ricochet | Verified in `steps_of`, `Ledger::bounce`, and the delivery witness; Held supplies Stopped, no ricochet entry, and NonPen without a shown HP loss | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P2: trapped-round test never reads the record | Verified red when Ledger always writes Left; it now asserts the delivered encounter's Rest and retainer | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: Held SpendExit fuze loses its delay | Verified red: the old branch reports zero instead of the fixture's 0.012 s (MEASURED); a stopped round retains a positive reference speed for its time count | ballistics: preserve a held round's fuze delay (#260) |
| Claude | Footprint P3: obsolete playback/reflection prose and name | Verified against discovery and playback; corrected to the current skid and clock invariants | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: s5 header disagrees with scenarios | Verified; generated header now follows scenarios, and all recorded scenarios were regenerated through the real march | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: dead recessed fixture | Verified: only its entity was printed; removed it while strengthening the outgoing-disc test | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: 54 micrometres is not the stated derivation | Verified arithmetic; corrected to 51.7 micrometres DERIVED from the stated inputs | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: recorder closure prose and test helper omit armed_by | Verified against `finish_records`; both now include the running fuze that the tank armed | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude | Footprint P3: trees read_shell lacks a skid-line witness | Verified; its mounted observer test now covers zero-chord, empty-contact skid momentum as well as a crossing | ballistics: exercise skid momentum in the trunk observer (#260) |
| Claude | Footprint P3: backlog ballistics-24 | Not an open defect: already closed by `ballistics: lay a round's own disc on every leg after a skid (#260)`; verified in backlog history and by the new trunk skid-line witness | Already closed; additional witness in ballistics: exercise skid momentum in the trunk observer (#260) |
| Claude | Footprint leftover: partly held entrance locates a future free entry | Verified red: 0.75 m of invented advance becomes zero (MEASURED); present samples locate a present contact | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude + Codex | Footprint leftovers: unrun cursor and offset mutants | Both run and rejected separately: nonzero inherited cursor; inherited incoming offsets outside the outgoing disc's plane | ballistics: give skid contacts a clock or a terminal rest (#260) |
| Claude + Codex | Footprint leftovers: five-lens sweep of edited items | Swept error-handling, code-branching, base-truths, testing and brevity; removed the count allowance and dead fixture, kept fail-loud gates, added no switch, quota or authored physical constant | ballistics: give skid contacts a clock or a terminal rest (#260) |

First-round MEASURED evidence: the combined targeted regression/mutation run failed all eight selected cases before restoration/fixes. Final CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) and `cargo fmt --all --check` passed. The full library passed 2209 tests, with 0 failures and 26 ignored measurement/tool beds. It includes ballistics, pen_cam, field_casts, damage_cam, recording_tests, headless_test, doc_citations, ui_ascii, net_boundary, bake and wire-pin guards. Recorded scenarios and field-cast pins passed without re-blessing.

First-round MEASURED Rust line delta against the dispatched base: 297 added, 352 removed, 55 removed net. Counts include comments, blanks and tests; prose/data files are excluded. No open backlog row names a file edited by this lane. net-04 is explicitly reported above.

Players gain impact-fuze arming on eligible sloped bites and contact decisions independent of cofacial volume key order. The moving-body world resume heading is retained. The first pressure-review brief is complete; net-04 remains open as reported.

Decisions:

- Preserve the existing entrance-owner tie break for momentum attribution; read face strength from the opening union-field segment.
- Use one shared slope-charge function and hand-off read for force and resistance, and express fuze force per metre of real travel.
- Use only shipped gun/round configurations as fuzz probes; remove hypothetical-only gun absence and preset lookup, retaining preset enumeration only for law tests.
- Keep BiteEnd::Perforated.direction: Pending::world_decision adds carrier velocity, so the world resume heading can differ from the relative incoming heading. Remove the unused embedded direction, Outcome direction, Geometry, duplicate bite axis argument, and unsupported control variants.
- Keep PROTOCOL_REV unchanged: removed resolution types do not ride the wire; compatibility identifiers remain owned by protocol.rs.
- Classify floor_m as StandIn for missing map-authored vertical bounds, without changing its value.
- Replace stale numeric skid limits with the tested first-contact-skid invariant; retain historical rejected-alternative prose where it is explicitly historical.
- Remove the mounted test's fixed-angle mutant claim; the direct pressure sweep owns that proof.
- Compare charged and zero-penalty force at the same leading disc encounter, rather than assuming the full disc is already in contact.
- Leave Claude F1/F2 untouched because the footprint commit already fixed them; report net-04 as a verified open defect, as authorized.
- Use separate commits for face behavior, tests, probe inputs, seam removal, prose/data labels, and print removal; validate the final composition in one full-library batch.
- Keep the fuzzer's sampling budgets, clustering cell and translated coordinate probe as instrument Resolution parameters; they bound sampling work and expose coordinate/precision errors, rather than author projectile physics.

Validation was consolidated after shared-lock waits; each concern is committed separately. All builds and test binaries ran from the leased bay through serial-cargo, with unattended headless muted settings. No captures, pricing runs, issue comments, pushes, tags or merges were performed.


Continuation evidence and decisions:

- The subject remains the game's damage model, game state only; crew are HP pools at stations. The owner's full-disc Held cliff at atan(contact_friction) is preserved unchanged.
- The skid law uses forward incoming travel to the disc's resting height, bounded by its hand-off. A future finite contact is reached before an exchange whose infinite-plane turn would be zero. A contact with no forward turn ends the round. No negative travel, counter, minimum-time nudge, altered step limit, or skipped exchange substitutes for that terminal state.
- A present contact is located only by samples already holding material. Its mark and momentum use that contact point; the record's path, clock and departure use the separately recorded axis position. The wire schema is unchanged, so PROTOCOL_REV remains owned by protocol.rs and is not bumped.
- Held armour gets a zero-chord retaining Plate, preserving strict validation. Held boxes remain components. Stopped is an internal path step; damage verdicts still give priority to an actual penetration and then a shown HP loss.
- A Held SpendExit fuze restarts the same authored delay, retaining the preceding positive reference speed for its time count. It waits at rest rather than becoming immediately due.
- The trapped-box witness reads the real recorder's result; the separate delivery witness mounts the shipped offline delivery plugin. Cursor, offset, resting-end, fuze-delay, present-entry and record-clock mutants were run and restored. The corner/groove decision-count test was replaced by the actual time-or-terminal invariant in shallow converging wedges.
- The source-return fixtures fix their faces so they test source admission independently of carrier motion placing a disc too deep to turn. Their geometry is unchanged. An unintended pose shift found in the intermediate diff was restored and is absent from the final change.
- s5 keeps Penetration, one skid, and Rest, with corrected timing, remaining charged chords and dose. s9 is unchanged. The axis-clock correction also updates s3 and s8a path/departure data. All changes are re-recordings through the production march, with no capture or pricing run.
- The tree observer's existing test is parameterized over a positive-chord crossing and a zero-chord skid, retaining its force and moment assertions. ballistics-24 was already deleted by the footprint commit; net-04 remains the explicitly reported open row in an untouched file. No current backlog row names an edited file.
- The edited items and their tests were checked under all five lenses. Existing authored numbers remain read from their owners, new inline numbers are Fixture data, internal diagnostics remain fail-loud, and there is one production path with no new flag or retained alternative.

Continuation green evidence (MEASURED): CI clippy and cargo fmt --all --check passed; the detached whole library passed 2213 tests, with 0 failures and 26 ignored tool/measurement beds. This includes ballistics, pen_cam, field_casts, damage_cam, recording_tests, headless_test, whole-tree guards and wire pins. Production scenario re-recording passed. The final comment-only clarification then passed CI clippy and all 44 selected whole-tree guards again. All executed processes have ended.

MEASURED Rust delta over both rounds, against the dispatched base: 742 added, 560 removed; net lines removed = -182 (182 added net). The continuation alone adds 445 and removes 208 Rust lines, 237 added net. Counts include tests, comments and blanks; RON and Markdown are excluded. The first-round-only figure above remains its historical measurement.

MEASURED all-file delta over both rounds, including this report and recorded data: 879 added, 604 removed; net lines removed = -275.

All requested findings in both briefs are resolved or evidenced as already fixed/not a defect. net-04 remains the explicitly authorized reported exception. Three code commits separate the fuze delay, the trunk observer witness, and the shared contact/record law; this report is the final evidence commit. The bay remains on its leased lane branch, with no push, merge, release, issue comment, external-repository mutation, screen capture, or pricing run.
