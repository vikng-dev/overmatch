# Driving: rollback design stop (#146)

Subject: the game's driving model. The lane stopped at item 1 under the owner's explicit
stop rule; the driving bar does **not** clear. Production code and tests are unchanged.
Evidence commit: **driving: record the rollback design stop (#146)**.
MEASURED start: 2026-11-02 19:57:34 UTC. This is a design stop, not the time limit.

## Verified rollback and terrain

The temporary witness called production `step` for 1,280 fixed ticks (Fixture: 20 simulated
seconds at the module's solver clock). Construction came from `grade_fixture_fp` / `tiger_tp`
and `compact_drivetrain`. Both belts entered at -2 m/s in the highest forward gear, clutch In,
park released, full forward intent and supply. Initial crank speed was synchronous and negative;
a separate run used the authored positive idle speed. Each tick supplied grade reaction
`W sin(theta)/2` per side, perceived grade `W sin(theta)` and grip `mu W cos(theta)`.
The fixture required `launch_pick` to return a capable gear before stepping; assertions never
became conditional on that answer. Below-limit cases required arrest and no cut before arrest;
above-limit cases additionally required recovered forward travel.

The compact construction with its original 50 kN per-side service capacity arrested both
20 and 40 degrees, with either initial crank sign. The review's service-only arrest estimate
does not establish the complete drivetrain's arrest limit. To exercise a second construction
with a brake shortfall inside grip, a declared compact fixture variant used 25 kN per side;
its engine, ratios, inertia and grip were unchanged. This is Fixture construction, not a
production adjustment or a relaxed assertion. Both above-limit grades still had an affordable
launch gear, independently of the service-brake budget.

MEASURED on unchanged production code, negative synchronous crank entry:

| Class fixture | Grade (degrees) | Launch gear | Final speed (m/s) | Arrest / cuts before arrest |
|---|---:|---:|---:|---|
| Heavy | 10 | 4 | 1.5175261 | yes / zero |
| Heavy | 30 | 3 | -130.0522 | no / zero |
| Compact, 25 kN per-side service | 20 | 2 | 2.3704474 | yes / zero |
| Compact, 25 kN per-side service | 40 | 1 | -158.06837 | no / zero |
| Heavy, shipped steepest face angle | 22.277782 | 4 | 2.049906 | yes / zero |

The above-limit fixtures retained their initial highest gear throughout. Positive-idle
heavy entry on 30 degrees also failed: MEASURED final speed -118.101105 m/s, F8, zero cuts.
These are transmission-unit witnesses with scripted support/reactions, not a real-contact
playtest or a claim about a full hull's acceleration.

MEASURED terrain: the production `terrain_grid::tests::shipped_grid` decode produced a
1,025-square grid at 1.4648438 m spacing. Scanning both anti-diagonal triangles of every cell
found a maximum upward-face angle of **22.277782 degrees**, cell (778, 754), second triangle.
The scan computed each triangle's two height derivatives, then `atan(hypot(dx, dz))`.
That face is within the fixtures' grip limit; upward faces satisfy `load::observe`'s positive-Y
support-normal admission. This measures face geometry, not a tank-footprint average or a claim
that a particular full-rig pose remains supported. The heavy unit witness arrests at that
angle, so the review's proposed P1 escalation is not established by the terrain maximum.

## Deletion trial

Only one production line changed: `driver` priced `command.abs().min(1.0)` unconditionally
instead of setting the price to zero under service. Slip, candidate pricing, reflected inertia,
and the upshift refusal all stayed unchanged. The same temporary class witnesses ran together
with `forward_crank_rollback_keeps_gear_under_service`.

MEASURED: four passed, three failed. Both above-limit recovery witnesses failed with the
same final speeds and gears. The existing arrestable-rollback witness also went red:
its forward-running crank, F4, -0.2 m/s, 20-degree entry selected F2 instead of retaining F4.
That grade's demand is below service capacity alone (DERIVED: approximately 191 kN against
192 kN), so this is exactly a cut the brief forbids the player buying. The deletion mutant
was rejected and restored. No other mutant was run.

Source explains the remaining gap: `choose_gear` holds on the live back-driven crank's
overrun; C3 requires rest; moving candidate landings use synchronous signed shaft speed and
its curve ceiling. None expresses whether the current controls can arrest the rollback.
Removing the service mask neither supplies that distinction nor reconciles launch pricing
with moving delivery.

**Stop decision.** The next implementation I can justify needs a condition for available
arrest force at the Driver seam: retain the gear when available braking and drivetrain force can arrest,
but admit recovery pricing when they cannot. The accepted design does not state that condition
or how a moving rollback enters launch-priced clutch delivery. Adding that predicate would
overturn the audit's CONVERGES verdict under the owner's stop rule. The lane therefore restores
the last green implementation and reports the missing design; it does not implement a new
predicate, state, Slip rule, or overhaul. The owner must settle that policy before this fix
chain continues. The separate over-rev and empty-station descent decisions remain untouched.

## Review disposition

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | 1: rollback cannot reach a capable gear | verified red on two class constructions; service-only compact limit corrected | Unfixed; evidence in driving: record the rollback design stop (#146) |
| Claude | 2: fair broader-Slip trial / upshift refusal | not reverified; stop rule reached first | — |
| Claude / Codex | 3: governor job, work ceiling, admitted inertia | not reverified; stop rule | — |
| Claude | 4: reverse steering rows | not reverified; stop rule | — |
| Claude / Codex | 5: second-tank coverage and mutants | only rollback class evidence above; remaining claims unverified | — |
| Codex | 6: reachable notch admission | not reverified; stop rule | — |
| Claude | 7: Driver seam, HUD, documentation and failure outcomes | not reverified; stop rule | — |

Tank #2 result is deliberately narrow: the original compact construction arrested the tested
entries; the weaker-brake compact construction reproduced the missing recovery. No real spec,
TrackGear/contact composition, power-curve settling trial, or item-5 mutant was run. The only
executed mutant was deletion of the service-price mask. This does not certify another tank. The temporary probes and mutant are removed rather than retaining failing tests or
assertions that bless the defect.

Player behavior is unchanged: a sufficiently steep, capable rollback still never buys recovery;
ordinary arrestable rollback retains its no-cut behavior. The bar remains blocked by item 1
and the uncompleted findings, including the unresolved broader-Slip trial.

Decisions: honor the item-1 stop before item 2; distinguish actual arrest behavior from the
review's brake-only estimate; use a declared weaker-brake class fixture rather than pretend
original compact data fails; reject the service-mask deletion; retain no experimental code;
commit evidence only. No wire change, so PROTOCOL_REV stays unchanged. No Bevy/Avian API was
introduced. No lens backlog row names the sole retained edited file; no row is closed.

## Green checkpoint gates

MEASURED on restored production code: `cargo fmt --all --check` passed; CI clippy passed
with `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
The targeted batch (`track:: headless_test command crew_ui doc_citations ui_ascii net_boundary
bake::`) passed 379 tests with four ignored. The final whole-library run passed 2,236 tests
with 26 ignored and zero failures. The batch exited successfully. All compilation and tests
ran through `scripts/serial-cargo` under its one machine lock, unattended, headless and muted.
No source changed between these gates and the evidence commit. All diagnostic and gate
processes exited; there is no lane process left running. No capture, push, merge, issue comment,
protocol change, or release occurred.
