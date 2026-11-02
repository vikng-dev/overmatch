# Contact-law review verification (#197)

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

MEASURED evidence: the combined targeted regression/mutation run failed all eight selected cases before restoration/fixes. Final CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) and `cargo fmt --all --check` passed. The full library passed 2209 tests, with 0 failures and 26 ignored measurement/tool beds. It includes ballistics, pen_cam, field_casts, damage_cam, recording_tests, headless_test, doc_citations, ui_ascii, net_boundary, bake and wire-pin guards. Recorded scenarios and field-cast pins passed without re-blessing.

MEASURED Rust line delta against the dispatched base: 297 added, 352 removed, 55 removed net. Counts include comments, blanks and tests; prose/data files are excluded. No open backlog row names a file edited by this lane. net-04 is explicitly reported above.

Players gain impact-fuze arming on eligible sloped bites and contact decisions independent of cofacial volume key order. The moving-body world resume heading is retained. No requested contact finding remains pending; net-04 remains open as reported.

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
