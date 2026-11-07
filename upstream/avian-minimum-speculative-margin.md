# Apply the minimum speculative margin when pruning contacts

Subject: the video game's server physics and environment destruction; game state only.

**Target:** vendored Avian 0.7.0, Bevy 0.19.1. **Status:** LOCAL DRAFT, not filed; upstream head was not checked.

`NarrowPhaseConfig::contact_tolerance` promises a minimum speculative margin, including for quiet
contacts. `NarrowPhase::update` floors the geometry query by that tolerance but prunes its returned
points against the velocity-only margin. At zero relative velocity, a positive gap inside the
configured tolerance therefore loses its contact. Small numerical gaps can repeatedly interrupt
support. DERIVED at the configured fixture clock and gravity: one unsupported step adds
9.81 / 64 = 0.15328125 m/s, exceeding Avian's default 0.15 m/s linear sleep threshold.

The patch floors the effective speculative margin once, before both consumers. The query distance
is unchanged. No sleep threshold, rest duration, body-kind rule or manual sleeping command changes.

The direct guard, `stationary_contacts_keep_the_configured_minimum_speculative_margin`, places
stationary cubes inside and outside the configured band. The production construction witness,
`settled_freed_matter_sleeps_within_avians_rest_window`, removes a Fixture construction's supports
on triangulated terrain, mounts landing fragmentation, and reads Avian's thresholds and rest time.
Its eligibility allowance is DERIVED as two fixed steps for solve/sleep visibility; a separate
Fixture watchdog bounds the collapse transient. No emergent population is an assertion.

**MEASURED:** the construction witness ends with 27 of 45 fragments awake before the patch and
zero of 50 after it. A separate hook probe keeps the sleep failure with the sweep contact hook
disabled, and records zero exchanges or new departures in the final rest window.

**MEASURED, local wave rehearsal:** the required scheduled twelve-construction run ends at
82 awake / 481 asleep, with a peak of 563 pieces. The orchestrator's supplied baseline is
227–237 awake / 290–299 asleep. These differing populations are observations, not a cost comparison.
A diagnostic repeat with a temporary eligibility census ends at 88 awake / 446 asleep. Its last
census has three rubble islands: 73 members with eight above a velocity threshold and 59 mature
rest timers; 13 members with nine mature timers; and two members both above a threshold. The
previous census has two above-threshold members in the thirteen-member island too. No rubble
island touches standing construction; the moving tank belongs to a separate island. These islands
remain ineligible under Avian's unchanged sleep law. The source of their residual motion is
unresolved; this patch does not establish that every wave fragment settles. The probe is removed.

**DERIVED work:** the existing per-pair maximum moves from the query distance into the shared
margin; there are no additional geometry queries or cuts. Each newly retained in-band point adds
one contact point to the substep solver while its island is awake. **MEASURED:** the final wave
trace reads 6,787 structure-contact points in its last tick (each structure side is a reading,
not a unique solver point). No tick or frame pricing is claimed.

Verification: workspace formatting and CI-feature workspace/all-target clippy with warnings denied
pass. The combined `construction::`, `debris::`, `net::ram`, `headless_test`, `avian_island_sleep`,
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::` run passes 342 tests, with six ignored.
The full suite is left to CI. All temporary probes are removed. The affected items pass the
repository lenses; no backlog row names an edited file.
