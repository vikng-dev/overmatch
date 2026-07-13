# Playtest forks — decisions only the game can settle

A register of design forks where the *correct* answer can't be reasoned out at the
desk — it depends on how the slice actually **feels in play**. Each fork records the
default we ship now, the alternatives we deliberately kept alive, why it's a playtest
call, and the cost to flip.

This is **not** the issue tracker (`.agents/scratch/<feature>/`, see
`.agents/docs/issue-tracker.md`) and **not** the ADR log (`.agents/docs/adr/`, for
*settled* decisions). It's the home for decisions that are *open by design* — chosen
provisionally, revisited at the controller.

## How to read an entry

- **Default** — what's wired in the code today.
- **Alternatives** — the paths we preserved, with enough detail to actually build them.
- **Why it's a playtest call** — what feel-question decides it; what to watch for in play.
- **Revert cost** — how mechanical the flip is, so a default never silently calcifies
  into an assumption. A high revert cost is a warning to playtest *sooner*.
- **Lives in** — code / design-doc anchors so the fork and its implementation stay linked.

When a fork is settled by play, promote it: write the ADR (or fold it into the design
doc), record the verdict here, and strike it from the live list.

---

## F1 — Crew juggling model (lone-survivor gun operation)

**Status:** open · default chosen + mechanic designed 2026-06-29 · settles in slice 2 playtest

**The question.** When only one combat-crewman survives (e.g. Driver + Gunner alive,
loader and commander dead), how does one human operate a gun that needs both an
aimer/firer *and* a loader?

**Default — hardcore (physical swap, one role at a time).** The survivor physically
moves between positions and can serve only one at a time: aim/fire **or** load, never
both at once. The cost of doing both is *time* (the swap), not a degraded stat. Real
tactical tension — the player gives up one capability to gain another.

**Mechanic (designed 2026-06-29, full spec in design §7b).** Bare **tap-source** (living
crewman) → **tap-target** (station). **Timed transition**: the crewman keeps manning the
source station until the timer completes, then the assignment flips atomically — trade-
with-latency, *no dead window*. Cancellable, not pausable. **Anyone → any** station;
`competence` (1.0 native / 0.6 foreign) is the only gate. The swap is a **1:1 transposition**
(crew ↔ station stays a bijection; the corpse takes the survivor's vacated seat).

**Two sub-knobs that are themselves playtest calls:**
- **Transition cost model** — *source-live latency* (chosen; the survivor keeps the old
  capability during transit) vs *both-ends-dark* (a real sacrifice window). Source-live is
  the gentler dial; flip to both-ends-dark if play feels too cheap. This is the lever that
  decides how harsh hardcore actually feels.
- **Timer duration** — ~3–5s start, tuned live.

Chosen because it's the most physically honest (a human can't occupy two seats), the
cleanest data model (`Occupies` stays strictly 1:1, efficiency stays 1.0 — no
coefficient field needed yet), and it layers: arcade efficiency tiers can be added *on
top* later if the time-cost feels too punishing. Going the other direction (ripping out
dual-occupancy) is harder.

**Alternatives kept alive:**
- **Arcade/1 — dual-occupancy at reduced efficiency.** The survivor occupies both
  positions simultaneously; reload and aim both run slower. Needs an `efficiency < 1.0`
  coefficient on `Occupies` and a rule for how one crewman maps to two positions at once.
  Ships with *no new player input* — its appeal and its danger (invisible stat-fudge,
  against the visualisation-first principle: the player should *see* the crewman move).
- **Arcade/2 — remote load from the gunner's seat at reduced efficiency.** Loading is
  performed without a physical swap. Needs the `provides` override deferred in slice 1
  (a position grants a role it isn't natively wired for).

**Why it's a playtest call.** Whether the swap *time-cost* reads as engaging tension or
as punishing dead-time can't be known until the swap is in hand and timed. Watch:
does the lone-survivor endgame feel like a desperate juggle (good) or like the tank is
just broken (bad)? If the latter, the arcade tiers are the pressure-release.

**Revert cost.** Low → medium. Hardcore is the smallest model; arcade is *additive*
(introduce the efficiency coefficient and/or the `provides` override on top). No teardown
of hardcore is required to trial an arcade tier.

**Lives in.** Design §7b (backfill + capability model v2 + the swap mechanic). Code:
the `Occupies` relationship and the capability-effectiveness evaluator in `src/damage.rs`
(slice 2 — renamed from slice-1's `action_available`/`TankActions`).

---

## F2 — Kill threshold (when is a tank dead)

**Status:** open · default chosen 2026-06-29 · settles once §1a-A is played

**The question.** Does a tank die at a crew-*count* threshold (the "a one-man tank is
operationally finished" rule), or only when every position is unstaffed?

**Default — A: pure per-position gating, no crew-count KO.** Capability is derived
entirely from per-position staffing AND module HP. A tank with one surviving Driver can
still drive; one surviving Gunner can still traverse/fire/reload. A tank is fully dead
only at **0 living crew**. `TankKnockedOut` survives as a readout/label only — never a
gameplay gate. Cookoff still kills all crew (→ 0 living → dead) and launches the turret,
triggered off `CookedOff` directly.

Chosen because the backfill-slice gameplay we want (a lone survivor choosing which single
position to staff) is *only possible* if a lone-crewman tank is still alive — the old
`<2 = dead` rule gates that tradeoff off before it can exist. A is also the smallest
primitive.

**Alternative kept alive:**
- **B — `<2` living crew = dead.** Tanks die earlier; lone survivors can't act; the
  backfill tradeoff (F1) never arises. Re-added as a single AND-clause:
  `capability_available AND tank not KO`, where KO latches at living crew `< 2`.

**Why it's a playtest call.** Whether a lone-survivor tank crawling around half-capable
is compelling (A) or just an annoying mop-up that should already be dead (B). Tightly
coupled to F1 — if hardcore juggling feels good, A is vindicated; if the lone-survivor
state feels like dead-time, B (or a "combat-ineffective" derived label) is the answer.

**Revert cost.** Low — B is a one-AND-clause mechanical revert (the design doc spells out
the exact clause). The risk isn't the flip; it's that A and B imply different *content*
around the lone-survivor state, so deciding late is more expensive than the code suggests.

**Lives in.** Design §1a (A/B under test). Code: `action_available` / `TankKnockedOut`
in `src/damage.rs`; cookoff hook on `CookedOff` (§8).

---

## F3 — Combat damage disclosure

**Status:** open · authority semantics settled, presentation deliberately temporary

**The question.** How much additional information should a shooter receive after the authority confirms enemy damage? The spectrum runs from no cue beyond visible world damage to an exact internal X-ray account.

**Default — explicit authoritative confirmation with temporary presentation.** The player's firing action is predicted immediately, but damage is never predicted as fact. The current hit-marker-like cue is a disposable view of authoritative damage, not the product target and not the semantic interface.

**Alternatives kept alive:**
- **World evidence only.** No additional indication beyond visible damage, fire, smoke, lost capability, and enemy behavior.
- **Restrained confirmation.** A subtle sound, crew callout, sight response, or other low-information acknowledgement that damage occurred.
- **Detailed disclosure.** Increasingly precise penetration, crew, tank-module, or X-ray information, up to a complete internal account.

The server owns disclosure. A live client must receive only the detail the current rule permits; hiding privileged truth in the UI is not concealment from a modified client. Armor inspection and scientific diagnostics may use a separate privileged adapter.

**Why it's a playtest call.** The correct point depends on whether feedback improves comprehension and learning or erodes observation, uncertainty, and tank knowledge. It cannot be settled from implementation convenience.

**Revert cost.** Low for presentation: the shooter already receives a discrete, shot-attributed authoritative `DamageConfirm`, deduplicated by `ShotId`, rather than inferring its cue from replicated health snapshots. Changing how much detail the server discloses may widen or narrow that semantic fact, but no presentation belongs in ballistics or damage truth.

**Lives in.** `src/net/hit_feel.rs`, the authoritative damage/outcome path in `src/net/protocol.rs` and `src/net/server.rs`, and the future armor-inspection presentation.
