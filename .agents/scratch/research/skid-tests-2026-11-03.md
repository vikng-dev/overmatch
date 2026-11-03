# Skid witnesses and contact delivery (#260)

Subject: the game's damage model, round movement across armour; game state only.
Lane: `fix-skid-tests`. All work, compilation and runs stayed in bay-3; no push.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude P2 | Gate and wedge bypass `at_first_touch`; shared-machine claim false | Verified: removing reopening leaves the old wedge green; the new gate witness fails on invented re-entry | `ballistics: drive witnesses through live first touch (#260)` |
| Claude / Codex P2 | Wedge accepts rounding travel | Verified: continuing turns now require half `CONTACT_STAND_OFF`; `turn.max(1e-9)` fails with `DrainExhausted` | `ballistics: require resolved flight between wedge skids (#260)` |
| Claude / Codex P2 | Closed boxes accept solver rest, ignore elapsed flight, and permit an empty projectile query | Verified: the short clock fixture actually stops; the repaired long slide requires a projectile and empty playback. Held must end the first contact and meet the friction predicate; elapsed flight bounds travel | `ballistics: distinguish Held and elapsed box flight (#260)` |
| Claude / Codex P2 | Live box record bypasses delivery | Verified: the fixture's actual path, outcomes and retaining plate now pass the mounted offline delivery plugin. Fixture round/weapon/vehicle/servo identity is supplied around them; unhit helper staffing is outside this armour-only projection | `ballistics: distinguish Held and elapsed box flight (#260)` |
| Codex P2 | Authored `floor_m` literal couples the validation fixture | Not a current defect: the previous independent fixture stays green when shipped floor changes to -200 m | `ballistics: record skid mutation evidence (#260)` |
| Both | Previously unrun discovery/path mutants | Verified by execution; results below | `ballistics: record skid mutation evidence (#260)` |
| Codex P1 | Contact burst delivered before carrier correction | Verified: MEASURED displacement reaches 1.171875 m in the spent-relative-speed fixture, not a universal millimetre bound. Carrier translation and scene moment now precede the burst mark, field and environment delivery | `ballistics: carry contact bursts before delivery (#260)` |
| Claude P2 | Tick boundary changes a valid skid into rest/entry | Verified in both still and moving fixtures; retained approach prevents reopening far enough. An ignored executable witness names #260; production behavior remains with the contact-state rebuild | `ballistics: retain the tick-boundary skid reproduction (#260)` |
| Claude P3 | Transported frame retains a world-square disc | Verified by the offsets and frame sources; bound documented, behavior unchanged | `ballistics: state skid frame and path contracts (#260)` |
| Claude P3 | `skid_on` outgoing-frame prose | Verified: outgoing speed/direction are world values; incoming speed/shock are relative | `ballistics: state skid frame and path contracts (#260)` |
| Claude P3 | Returning-encounter `LockedAxes` crutch and false mass reason | Verified: tank helper explicitly supplies mass/inertia. Removed; real encounter test passes unlocked | `ballistics: state skid frame and path contracts (#260)` |
| Claude P3 | Trail test owns another test's doc | Verified and moved back to `capture_respects_spacing_and_cap` | `ballistics: state skid frame and path contracts (#260)` |
| Claude P3 | Resting height duplicated | Verified; `resting_height` owns the physical-radius calculation | `ballistics: state skid frame and path contracts (#260)` |
| Claude P3, crash leftovers | Platform `f32::atan2` | Verified against `trees.rs`; now `libm::atan2f`; the blend witness is red on `acos` | `ballistics: close the contact-walk crash leftovers (#260)` |
| Claude P3, crash leftovers | `REST_CLEARANCE` prose hides line-round branches | Verified; doc states targets and the stop band. Its second job remains explicitly recorded for the owner | `ballistics: close the contact-walk crash leftovers (#260)` |
| Claude P3, crash leftovers | AABB containment prose | Verified; names grown bounds and the rounding assumption. DERIVED f32 spacing at 8192 m nearly consumes the margin, rather than proving unlimited containment | `ballistics: close the contact-walk crash leftovers (#260)` |
| Claude P3, crash leftovers | Constant jobs missing | Verified; `ENTRANCE_SPAN`, `DISC_MIN_CALIBER`, `FIELD_REACH`, `MAX_STEP_CROSSINGS` name Resolution; `REACH_*` names Fixture | `ballistics: close the contact-walk crash leftovers (#260)` |
| Claude P3, crash leftovers | Crash aims can become misses | Verified; each recorded aim now asserts a real first touch | `ballistics: close the contact-walk crash leftovers (#260)` |
| Claude P3, crash leftovers | Rim-chain test restates the blend | Verified: it calls only `turned_toward`, never contact entry. Removed; `a_blend_closes_its_share_of_any_angle` kills its `acos` mutant | `ballistics: close the contact-walk crash leftovers (#260)` |

## Mutation evidence

Every temporary mutation was restored before the next baseline build. Logs remain under the ignored `target/fix-skid-tests/` directory in this warm bay.

| mutation | witness | observed result |
|---|---|---|
| Remove `at_first_touch` reopening, old driver | Old converging wedge | GREEN: demonstrated the bypass |
| Remove reopening, live-entry driver | `the_gate_keeps_a_first_touch_skid_in_flight` | RED: skid re-enters the same plate and embeds |
| `turn.max(1e-9)` | Converging wedge | RED: `DrainExhausted`, crossing 4097; baseline stricter travel assertion is green |
| Disable `Begin::Held`, original multi-contact fixture | Spent box | GREEN: even the Held-angle check could be imitated by terminal skid |
| Disable `Begin::Held`, first-contact fixture | Repaired spent box | RED: first outgoing speed is MEASURED 4.2788396 m/s instead of rest |
| Shipped floor -100 m to -200 m | `shared_physical_data_is_required_and_validated` | GREEN: valid independent fixture |
| Drop discovery's `bounced` break | Long-box clock witness | RED: future playback remains where the slide should return to the tick clock |
| Authority path index + 1 | `a_skid_names_its_turn_in_the_shell_path` | RED |
| Replica appends strike instead of resume | `a_replica_draws_a_skid_through_its_axis_not_its_strike` | RED |
| Blend angle via `acos` | `a_blend_closes_its_share_of_any_angle` | RED |
| Existing tick-boundary behavior, explicitly run ignored | Both still and moving near-contact fixtures | RED: both become terminal while reference approaches fly on |

## Measured and derived bounds

The burst witness's uncorrected tip offsets are MEASURED from live authority marks against the independently advanced face:

| relative speed (m/s), fixture | carrier speed (m/s), fixture | uncorrected offset (m), MEASURED |
|---|---|---|
| 773 | 15 | 0.000018835068 |
| 100 | 15 | 0.00015002489 |
| 1 | 100 | 1.171875 |

The fixed witness additionally includes relative speed 0.1 m/s against the 100 m/s carrier. All four cases require the live tip within 0.2 mm of the advanced face, allowing the disc's gravity tilt and f32 pose/cast rounding; field origin equals the corrected tip. The environment's authored filler centre is calculated from that same corrected flight before delivery.

DERIVED general displacement bound: `stand_off <= relative_speed * seconds`, hence `short <= seconds <= remaining_tick`; the missing correction is at most `|carrier| * remaining_tick`, not a fixed distance. The measured spent fixture reaches 0.75 of its fixture tick. This confirms P1 severity and supports a local translation/order repair using existing `ContactMotion`.

DERIVED disc-tilt bound: for relative/world heading angle delta, rim axial stagger is at most `r sin(delta)` and projected-width loss at most `1 - cos(delta)`. Fixture r = 44 mm, relative speed 100 m/s and transverse carrier 15 m/s give 8.53 degrees, 6.53 mm and 1.11% respectively. No tilt repair was made.

The tick witness's MEASURED references retain about 746.66 m/s along the face. Both 5 mm near-contact starts terminate instead. DERIVED affected band for its 88 mm, 75-degree fixture: `(r * (1 - sin(theta)) + clearance) / cos(theta) = 9.66 mm`. Both scene fixtures are evaluated before any assertion, so the first failure cannot hide the second run.

## Decisions and player-visible behavior

1. Preserve the accepted contact law and the provisional slide exit. No new contact state, negative travel, minimum-time nudge or solver limit was introduced.
2. Share `at_first_touch` with the frozen geometric gate. Charge the open approach once and keep its witness pass. The slide-step assertion now checks forward collinearity instead of assuming that the crossing owns the earlier open flight.
3. Use fixed geometry for the box witnesses. Its long slide tests elapsed flight and planner ownership; a first-contact Held fixture prevents a later terminal skid from impersonating Held. The delivered record retains actual resolved geometry and timing, with explicit fixture identity around it.
4. Apply contact carrier translation and the contact scene moment before burst delivery. Players stop seeing instant contact bursts and their blast displaced behind moving armour; still targets retain their result.
5. Leave tick partitioning as an ignored reproduction: extending behind spawn/tick start would rewind time, and resolving the active contact requires the owner's rebuild. Players can still see that near-boundary stop; it is not claimed fixed.
6. Document disc tilt without changing it. Keep the axis-only `radius == 0` stop and `REST_CLEARANCE`'s stop-band job with the rebuild, as dispatched.
7. Preserve authority/replica path identity behavior, remove the unlocked-encounter crutch, give radius geometry one owner, and remove the redundant chain test after checking its sole dependency and the surviving mutant witness.
8. Apply the crash draft without its already-landed flush hunk. Use the platform-independent angle implementation, accurate containment prose, named constant jobs and nonvacuous crash aims.
9. No backlog row names an edited file; no backlog row was deleted. No issue was commented on, no external repository was modified, no excluded document was opened, and no web search or capture was taken.
10. All experiments and tests run unattended, headless and muted under the machine lock. The mutation batch holds one lock and uses `serial-cargo`'s existing held-lock entry, avoiding nested locks and interleaved mutation builds. Queued builds were stopped only before they acquired the lock; the initial broad run was stopped after its identified assertion failure. Those interrupted runs are not counted as green gates.

## Gates

PASS: `cargo fmt --all --check`; CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`); targeted ballistics, pen cam, field casts, damage cam and trail tests; whole-tree `doc_citations`, `ui_ascii`, `net_boundary`, `bake::` guards. Targeted result: `test result: ok. 669 passed; 0 failed; 14 ignored; 0 measured; 1633 filtered out; finished in 11.77s`. Whole library, including `headless_test`, detached under the machine lock: `test result: ok. 2284 passed; 0 failed; 32 ignored; 0 measured; 0 filtered out; finished in 288.87s`. The one new tick-boundary test is deliberately ignored and was explicitly run red for both frames. No windows, audio, captures or pushes.
