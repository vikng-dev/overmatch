Subject: the video game's environment destruction model; game state only. Session 10; the sweep budget remains unmet.
Main fix: `lane/massfix`, “construction: initialize complete group mass on main (#235)”, is ready for separate landing.
MEASURED main witnesses read zero mass before physics preparation; DERIVED first-fragment occupied mass is 26.983126 kg. Fresh groups and rotated/translated split children now carry complete mass, inertia and COM at spawn; MEASURED 104 targeted/guard tests and 34 headless checks pass, with fmt and CI clippy.
MEASURED impact +1: 70 convex leaf-pair casts total 0.539128 ms, DERIVED 7.702 microseconds each. Source hulls reach MEASURED 93 vertices; targets have MEASURED 8. Each cast visits one source/target leaf; the body-local tree already exists.
Pinned Parry uses repeated closest-point GJK and time bisection, with full vertex support scans and a contact query for the witness. Iteration counts were not measured.
MEASURED separate diagnostic probes: preload 7.588 ms; outer field 5.836 ms versus kernel phase 1.067 ms; field rebuild 1.029 ms and settling 5.135 ms. The matching-signature/empty-slip reuse predicate was false, without distinguishing its causes.
MEASURED rejected candidate A/B: maximum sweep 9.557/9.314 ms, worst tick 26.705/25.887 ms, overruns contact/all 4/4 in both; session 9 was 9.694/9.544 ms sweep, 26.783/25.872 ms worst, 4/4 in both. Every new ram has a sibling load.log.
Decision: discard parallel face/bond verdict batching; it missed the budget without a clear benefit for added dispatch. All probes and the candidate are deleted; retained production source equals the entry tip. The recorded source/cast/awake census moves and is not an obligation on physics.
MEASURED retained gates: 105 targeted/guard tests pass, fmt and CI clippy pass. The completed full library suite passes: MEASURED 2497 passed, zero failed, 41 ignored.
Remaining: isolate and reduce the outer gravity contact-set settling work; no cost fix or new retained-tip ram prices were delivered. The candidate prices above are not prices of retained code. Nothing pushed.
[Methods, raw evidence, prices, census and load paths](rubble-session10.json).
