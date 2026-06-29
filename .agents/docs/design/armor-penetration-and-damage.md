# Design sketch: armor penetration & module damage (the ballistics simulator)

**Status: SPEC for the in-progress build (2026-06-27).** Decided in a design interview; being
implemented now, starting with an isolated sandbox (§11). Graduates to ADR(s) if it survives
contact — the `ballistics`-as-library-plugin + `shooting` split (§10) is the most ADR-worthy part.
Vocabulary from `.agents/CONTEXT.md` and `.agents/skills/codebase-design` (seam, depth, leverage).

This is the next vertical-slice mechanic after the gunner's sight (`design/gunner-sight.md`). It is
deliberately deep and physically-grounded; §9 names what is *out* of the first slice so the depth
doesn't masquerade as scope.

## 1. The kill model — crew is the only health

There is **no tank HP**. A tank is dead when it has **no living crew** — the count threshold
("fewer than 2 = dead") is **deferred pending playtest** (see §1a below). Everything else is
emergent and *repairable* — only ammunition is terminal. Three paths empty crew:

- **Direct hits** — a penetrator or spall fragment reaching a crewman.
- **Engine fire** — damages nearby crew slowly, by proximity, over time.
- **Ammo cookoff** — detonation of an ammunition volume; **instantly kills all crew**.

Module damage (engine, breech, optics, transmission, …) *never* kills the tank — it degrades
capability and can be repaired. Ammunition is the one exception.

### 1a. Kill threshold — A/B under test (2026-06-29)

(Tracked as a playtest fork: `.agents/scratch/playtest-forks/README.md` (F2).)

**Background.** Originally (this §1) a tank was dead at fewer than 2 living crew — the "a one-man
tank is operationally finished" rationale. Implementing per-position capability gating (§7) exposed
a tension: the backfill-slice gameplay we want (a lone survivor choosing which single position to
staff — drive OR shoot, not both) is *only possible* if a lone-crewman tank is still alive. So the
<2 threshold gates off the backfill tradeoff before it can exist.

**v1 cut (chosen for now, pending playtest): pure per-position gating, no crew-count KO.**
Capability loss is derived entirely from per-position staffing (§7): `capability_available =
staffed AND module HP > 0`. A tank with one surviving Driver can still drive; a tank with one
surviving Gunner can still traverse/fire/reload (but not move or see); etc. `TankKnockedOut`
becomes a *readout/label* only, never a gameplay gate. A tank is fully dead only at **0 living
crew** (every position unstaffed) — a separate condition from a count threshold.

**A (this cut) vs B (the old <2 rule):**
- **A — pure per-position (chosen for v1):** enables the backfill-slice tradeoff; smallest
  primitive; KO gate can be re-added as one AND-clause if A feels wrong in playtest.
- **B — <2 crew = dead:** tanks die earlier; lone survivors can't act; backfill tradeoff never
  arises because the tank is already dead at the moment a second crewman dies.

**What A means for the existing wiring:**
- `function_disabled` (module HP gate) stays as-is.
- A crew-staffing gate is *added* alongside it (§7): capability is on iff position staffed by a
  living crewman AND module HP > 0. Both must hold; they compose.
- `TankKnockedOut` is **retired as a gameplay gate** (no system reads it to disable drive/fire).
  It may survive as a derived label for HUD/scoring, or be removed entirely pending slice-2
  needs. Cookoff (§8) still kills all crew and launches the turret — that hook moves from
  `TankKnockedOut`-triggered to `CookedOff`-triggered directly.
- **If A plays wrong in testing**, B is re-added as: `capability_available AND tank not KO`, where
  KO latches at living crew < 2. One AND-clause, mechanical revert.

**What "kills" a tank under A:** only 0 living crew. (A "combat-ineffective" derived label — no
combat capability remains, or no drive AND no fire — may be added later for scoring; it is *not* a
gameplay gate in v1.)

## 2. The unified primitive — the ballistic volume

There is no real distinction between an "armor plate" and a "module": both are a **watertight solid
mesh + a material** (density/hardness → a *material factor*) that taxes a penetrator over the
line-of-sight distance through it. The Tiger's upper front plate is a thin, dense, high-cost slab;
the engine is a big low-density box that only stops a round if the path chews through enough of it.
Same primitive.

- A **module** = a ballistic volume that *also* carries function + state (engine, ammo, breech).
- **Crew** = a ballistic volume that can be incapacitated.
- Every module/crewman is a ballistic volume; not every ballistic volume is a module (bare plates
  have no function to lose). The march reads the *volume* layer (cost only); the consequence step
  reads the *module/crew* layer — "same geometry, consumed by type" (ADR-0008).

**Geometric thickness, not a slope coefficient.** Thickness is *measured* from the solid: the path
enters the front face and exits the back face, and the distance through the solid **is** the
line-of-sight thickness. Slope is free (a sloped plate is geometrically longer); no `cos` term.
Friend is modelling the Tiger's armor as solid volumes now.

**Convexity is not required; watertight/manifold is.** ADR-0008's convex constraint is a *physics
solver* requirement on the dynamic collision proxy. The armor layer is read by our **penetration
raycast** — a static-frame spatial query — and ray-vs-triangle works on any geometry. What replaces
convexity is **manifold/watertight**, so entry/exit faces pair cleanly and "inside the solid" is
well-defined.

## 3. The penetrator march — velocity is the source of truth

For a fixed projectile (mass, caliber, hardness constant), penetration is a monotonic, invertible
function of velocity (DeMarre, `pen ∝ vⁿ`). So **velocity is the stored state; penetration
*capability* is its derivative.** The march, per volume crossed:

1. `capability = f(mass, velocity)` — reference-mm this shell can defeat right now. **Mass is the
   primary driver** (sectional density / KE, `pen ∝ massᴹ·vⁿ`, M≈0.5); velocity secondary; both held
   in the shell, mass constant so velocity stays the stored state. Caliber is *not* here — it drives
   overmatch and spall hole-size, not raw penetration. (This is what separates a tank shell from a
   bullet at equal speed, and is the seed of the per-shell data struct.)
2. `cost = LOS_distance × material_factor` (× angle effects), also reference-mm.
3. If `capability > cost` → **perforate**: spend `cost`, invert `f` back to a reduced **residual
   velocity** (the Lambert–Jonas shape — barely-penetrate exits slow, big overmatch barely slows),
   and throw spall (§5).
4. Else → **embed** and stop.

Properties:

- **Modules tax the penetrator too** — they are volumes. A shell can run out of steam *inside* the
  tank after crossing the outer plate plus an engine block.
- **The path is a multi-segment world-space ray** that bends (normalization) and deflects
  (ricochet). A segment can leave one tank and strike another (skip off a glacis into a neighbour).
- **Deliberate omission:** *shatter* (penetration going non-monotonic at extreme velocity against
  hard armor) is out of the slice.

## 4. Boundary interaction — the decision tree at each face

When the path reaches a volume's face:

1. **Impact angle** = angle between path and surface normal at the hit point.
2. **Overmatch** — if caliber ≥ ~k× the volume's thickness *along its normal* at that point: suppress
   ricochet, normalize almost fully, punch through (cost still applies). The game's namesake, but
   **one modifier among many — not the centerpiece.** Don't over-build it.
3. **Ricochet** — if not overmatched and impact angle exceeds the **per-shell-type ricochet
   threshold** (a fixed constant for now; we have one shell): **deflect** — spawn a new path segment
   off the face, bleed velocity, do *not* enter. **No spall on ricochet** for now (spall is an
   exit/perforation event only — §5).
4. **Otherwise** — **normalize** (bend the path toward the normal), enter, and march the geometric
   LOS cost; perforate→exit (spall) or embed.

The *structure* is the design; the magnitudes (k, ricochet angle, normalization degrees, material
factors, the DeMarre exponent) are **live knobs the sandbox exists to tune** — built as adjustable
constants, not baked.

## 5. Spall — the exit cone (the primary crew-killer)

On every **perforation exit**, the volume emits a **consistent, fixed-shape cone** — *not* a
per-shell dynamically-derived spray (that read as confusing/inconsistent). Dense on-axis at the
source (point-blank behind the plate is a guaranteed hit), thinning with angle and distance, so
survival odds rise the further off-axis and the deeper a component sits.

- **Cone density = expected fragment-units per direction.**
- A **fragment is an energy packet** (superseded the original 1-HP/no-pen token — energy-packet cut
  2026-06-28). It carries a penetration value (RHA-mm) that bleeds with distance (drag); it deposits
  damage scaled by that energy, then **punches through thin volumes** (losing the cost it spends) or
  **stops in thick ones**. So **geometric shadowing** still holds for the engine block, but a thin
  bulkhead no longer fully protects what's behind it, and a strong on-axis fragment can exit the tank
  to reach another a few metres back (arriving weak). On-axis fragments are stronger (narrower, more
  penetrating) — the continuous form of WT's "more power ↔ narrower cone" groups.
- AP spalls at the exit of **every** plate it perforates → multiple spall events per shot, each
  rolling independently against nearby components.

**Locked v1 model (2026-06-28).** The cone's *shape* is fixed (symmetric, half-angle constant,
axis = the penetrator's **exit direction** — which already carries the normalization bend; obliquity
adds no skew); only its *fragment budget* (density) scales with the shot. The budget is the
**product of a body term and a shell term** — both must be present or there is no spall:

> `spall_budget ∝ cost_paid × v_res² × caliber`

- **Body term = `cost_paid` = LOS_distance × material_factor** — the material the round chewed, i.e.
  the *supply* of fragments. Thin/soft body → ~0; crew (≈0 factor) don't scab. (Resolves the §9 open
  tab: budget scales with residual energy *and* material, not either alone.)
- **Shell term = `v_res²`** (residual energy) — the *power* to throw them forward. Barely-through →
  v_res≈0 → ~0; overmatch with energy to spare → violent.
- This is why both extremes are weak: barely-through starves the shell term; thin/soft body starves
  the body term. Optimal = a well-sized plate perforated with energy to spare.
- `material_factor` doubles as the spall-supply proxy for v1. A dedicated **`spall_factor`**
  (brittleness, the physically-correct scabbing driver — ductile RHA resists, brittle armor/cast iron
  throws) is **noted as a later refinement**, not built yet.

**Energy-packet refinement (2026-06-28).** The total-energy budget above is *factored* into a
**count × per-fragment energy**, which is cleaner and is where shell-type variation will live:
- **count ∝ cost × caliber** (material supply + hole size — how *many* fragments).
- **per-fragment energy ∝ v_res² × (on-axis weight)** (the shot's push, concentrated on-axis — how
  *hard* each one is). Energy → both damage *and* penetration (RHA-mm), and bleeds with distance
  (fragment drag). Product still ≈ `cost × v_res² × caliber`, so the §5 budget is preserved.
This single change subsumes three threads: cone shape (on-axis weighting), fragment penetration
(energy-gated), and fragment drag (distance term). Shell types then vary the energy total + its
on-axis concentration + a fragment-penetration ceiling (see the shell-as-data direction in the
handoff). Other shell types (APHE/APDS/APFSDS/HE/…) stay deferred — AP is the populated point.
- **HE is a separate, penetration-independent mechanic** (later): a fuse triggers on minimum
  steel-equivalent thickness, then after a delay detonates into a much denser, wider cone — the
  penetrator is gone. Not modelled in this slice.

## 6. Component damage — HP per component, never per tank

Every component has its own **HP pool** (e.g. crewman 3, engine 10). A fragment deposits 1 unit; the
**main penetrator transiting** a module deposits many (scaled by the energy it spent crossing it).
This is *local function state*, not a global health bar — the kill condition is still crew < 2.

- **Crew are soft** (low HP) — the kill currency; a graze chips, a faceful of cone or a clean
  transit kills. (HP, not binary, so fire-over-time and near-misses have somewhere to accumulate.)
- **Modules are tougher and repairable** — one stray fragment scratches an engine; a faceful or a
  direct transit wrecks it.
- **Degraded performance (later):** checkpoints preferred (e.g. ≤50% HP → −x% power) over continuous
  `hp% = perf%`, for legibility. Tuning, not slice.

## 7. Crew — positions with capabilities, served by crewmen

Crew are not a counter; each crewman **is a station/function**: gunner (aim), loader (reload), driver
(move), commander (view/command). Capability is never owned by a module alone — it is **served by
whichever living crewman holds that station, at their effectiveness.** Stations **backfill**, the
commander being the universal (degraded) backup:

- Loader down → commander loads, slower.
- Gunner down → commander overrides the optic (modern), or the player falls back to the commander's
  third-person view (old). Lose the commander too and that view is gone.

**The view/control modes are themselves crew functions** — third-person = the commander's eyes out of
the cupola; the gunner's optic (`sight.rs`, in flight) = the gunner's station. So the crew system
will eventually **gate** sight/aim/driving rather than sit beside them. *Flag for the gunner-sight
work:* the optic and third-person toggle are crew-served capabilities, not unconditional.

### 7a. v1 capability model — positions + composition (2026-06-29)

> **Superseded by §7b (2026-06-29).** This records what slice 1 shipped: capability *tags on
> positions*, AND-only requirements, the `Action` name. §7b is the current target — `Capability`
> (renamed), requirement *groups* (Part/Pool/Backup), graded effectiveness, and the Station/Crewman
> split. Kept for history; do not implement from this section.

**Slice 1 (this slice): composed position capabilities + crew death affecting those.** Slice 2 =
backfill and position swapping between crew members upon death (separate slice; see §7b).

**The primitive:** a ballistic volume that is also a crew position carries a `Position` component
with a set of `Capability` tags it grants when staffed by a living crewman. v1 keeps `CrewStation`
as the station-identity label (what seat this volume is — "Commander", "Gunner", …, used for
diagnostic readouts and slice-2 backfill ("who is where")) and adds a parallel `capabilities`
facet (what the position grants). So `CrewStation` is **not** retired in slice 1; slice 1 keeps
it as identity, slice 2 may grow it (see §7b). The fused-model v1 cuts `CrewStation`'s *role as
the gating key* (gating now keys off the `capabilities` set, not the enum variant) but keeps the
enum itself as the station identity. v1 is the **fused model** — the crewman's body and the
position are the same entity (the existing `Ballistic_<Crew>` volume). The separation refactor
(extract occupant identity into a child entity + `Occupies` relationship) lands at slice 2
(backfill), not now.

**v1 capability set:** `Drive`, `Traverse`, `Fire`, `Reload`, `GunnerSight`, `CommanderView`.
- "Traverse" = slewing the turret/gun servo (gates `ServoCommand` writers in `aim.rs` and
  `sight.rs`). Distinct from `GunnerSight` (the optic camera), which the gunner also carries —
  both die together when the gunner dies, but they gate independent systems.
- `Reload` is modeled explicitly as a `Reload { remaining: f32 }` component on the `Gun` entity
  (in seconds, ticking down; `remaining == 0` = fireable), not a singleton resource. Loader-dead
  → no decrement (currently-loading round stays partway through).

**Tiger v1 positions and capabilities (the RON's `volumes` facets):**

| Position | Capabilities |
|---|---|
| Driver | Drive |
| Gunner | Traverse, Fire, GunnerSight |
| Loader | Reload |
| Commander | CommanderView |
| BowGunner | ∅ (body to kill; hull MG capability comes later) |

**v1 staffing query (binary, no efficiency):** "is there any living crewman-position entity with
capability X, owned by this tank?" = `Position { capabilities contains X } AND !Incapacitated AND
VolumeOf(tank)`. The staffing query keys off the `capabilities` set (data), **not** the
`CrewStation` variant — so a future tank could have two Gunner-variant positions with different
capability sets without code change. `CrewStation` is display/identity only. No `Occupies`
relationship in slice 1 (natural assignment only — each crewman is born into its position;
backfill/swap is slice 2).

**Composition with module gates:** a capability is available iff the position is staffed by a
living crewman AND the relevant module HP > 0 (e.g. Drive requires Driver-alive AND Engine-alive
AND Transmission-alive). The existing `function_disabled` gate stays; a crew-staffing gate is
added alongside it. They compose (AND), they don't replace.

**View on crewman death — auto-fallback:** losing the crewman whose view is active auto-switches
the player to the other view if its crewman is alive (CommanderView dead → gunner optic;
GunnerSight dead → third-person). Both dead → dark (pairs naturally with imminent 0-living-crew
death under the §1a model).

**Playtest fork — single-crewman juggling (slice 2, *not* v1):** when only one combat-crewman
remains alive, what happens to the gun? **Default chosen 2026-06-29: hardcore** — the survivor
physically swaps between positions and serves one role at a time (constant no-aim-while-loading /
no-load-while-aiming); the cost is *time*, keeping `Occupies` strictly 1:1 (no efficiency
coefficient needed yet). Arcade variants (dual-occupancy, or remote load) are preserved as
additive layers if the time-cost plays too punishing. Full fork — alternatives, why it's a
playtest call, revert cost — in the register: `.agents/scratch/playtest-forks/README.md` (F1).
Requires the slice-2 position-separation refactor (extract crewman identity from position entity,
add `Occupies`).

### 7b. Backfill + capability model v2 — the swap mechanic (slice 2, designed 2026-06-29)

Designing backfill evolved the capability model past §7a's AND-only requirements and past slice-1's
`Action` naming. **This subsection is the current target; §7a is the slice-1 historical record.**

**Separation refactor — Station vs Crewman.**
- **Station** — the *place*: a fixed, spatial ballistic volume carrying a **role** (the Gunner's
  station grants the gunnery capabilities). Persists whether occupied by a living crewman, a corpse,
  or (transiently) no one. **Role lives on the station, not the occupant** — which is what makes
  "the commander serves the loader's station" expressible (Commander *by specialty*, occupying the
  Loader station *by assignment*).
- **Crewman** — the *human*: HP, `Dead`, a `home` (native station / specialty), and later skill.
- **Bijection invariant:** crew ↔ station is always a perfect matching (N crewmen, N stations). Dead
  crew stay in the matching — their station still absorbs penetrators (unchanged). A swap is a
  **transposition** of two crewmen's station assignments.

**Topology B — occupant-data on seats (chosen 2026-06-29, implemented).** Rather than split station
and crewman into *separate entities* with an `Occupies` relationship (call that topology **C**), the
slice keeps the **fused seat-volume** as the unit and treats the crewman as occupant *data* on it:
the volume carries geometry + `ComponentHealth` (the body's HP) + `CrewStation` (the **seat**'s role)
+ `Crewman { home }` (who currently sits there). A swap **exchanges occupant state** (`home`, HP,
`Dead`) between two seats — so the *living* crewman's killable hitbox moves with the person (shooting
the new seat kills the backfiller), honouring the Q4 honesty decision, while the ballistics march is
**untouched** (it still deposits onto each volume's `ComponentHealth`). `competence(home, seat)`
(`damage.rs`) gates the foreign-seat penalty (native 1.0 / flat 0.6). **Topology C** — first-class
`Station`/`Crewman` entities + an `Occupies` relationship + rerouting the four ballistics deposit
sites to the occupant — is the cleaner long-term form, deferred until a second reason to pay for it
(e.g. crew that visually relocate, or stations with independent geometry); see §9.

**Capability (renamed from `Action`).** The tank-model verb — Drive, Traverse, Fire, Load,
GunnerSight, CommanderView. The player-facing intent verb lives in the future Controls layer
(ROADMAP Phase 2), so "Capability" is free for the model concept *and* carries a **degree**
(effectiveness) that "available action" could not.

**Requirement model — groups.** A capability's requirement is a **list of groups, AND'd** (`min`
across groups). Each group combines its members one of two ways:
- **`Part(x)`** — a single mandatory thing (sugar for a one-member group at 1.0); missing → 0.
- **`Pool([...])`** — *cooperative* redundancy: present contributions **sum, capped at 1.0** (two
  engines at 0.5; two loaders on a heavy gun).
- **`Backup([...])`** — *substitutive* redundancy: the **best** available path wins (`max`) (electric
  vs hand traverse; autoloader vs hand-load). A backup routes around the primary's dependencies, so
  those deps fold into the *primary member's* quality.

A member is `(coeff, [Part])`: `coeff` is its share (Pool) or ceiling (Backup); `[Part]` are the
things it needs, whose qualities **multiply** in. **`Part`** is a single flat enum naming every
referenceable thing (crew stations + module functions, one vocabulary — no `Crew(...)`/`Module(...)`
wrapper). Crew-vs-module is intrinsic to the resolved entity, not the reference.

**Quality** ∈ [0,1], resolved live per `Part`:
- **Module** → condition: 1.0 if HP > 0 (graded damage curve later, §9).
- **Crew** → `competence(crewman, station)` = 1.0 native / flat 0.6 foreign for now; later × skill.
  Competence is **relational** (crewman × station), *not* an attribute of either — the seam the
  skill/training system plugs into.

**Combine (per frame, against the live world):**
```
member  = coeff × Π(quality of each Part it needs)
group   = Part/Pool: min(1, Σ members)   |   Backup: max(members)
effectiveness = min over groups          // 0 = unavailable, 1 = full
```
Effectiveness is a **rate** (`time = base / effectiveness`; reload at 0.3 → ~3.3× as long — or a
speed/power scale). The RON declares only the static *recipe* (structure + coefficients); HP, who's
alive, and backfill assignment are live inputs — death/damage/swap change the inputs, never the
recipe.

**RON shape (flat):**
```ron
capabilities: {
    Fire:     [Gunner, Breech, GunBarrel],
    Drive:    [Driver, Pool([(0.5, [Engine_L]), (0.5, [Engine_R])]), Transmission],
    Load:     [Backup([(1.0, [Autoloader]), (0.15, [Loader])]), Breech],
    Traverse: [Gunner, Backup([(1.0, [TraverseMotor]), (0.1, [])])],
}
```
Tiger needs only bare `Part` (mandatory) groups; Pool/Backup are exercised only by exotic tanks
(the autoloader tank just writes `Load: [Backup([(1.0,[Autoloader])]), Breech]` and drops its Loader
station — no code change). Serde: mixing a bare `Part` and a `Pool(...)` in one `Vec<Group>` likely
needs `#[serde(untagged)]` on `Group` — verify it round-trips against the pinned `ron` first. `Part`
as a **flat** enum (not wrapping CrewStation/FunctionRole) is the robust choice and avoids untagged
at the reference level.

**The swap mechanic (grilled 2026-06-29):**
- **Trigger** — bare **tap-source** (a living crewman) → **tap-target** (a station). Player picks
  both ends; nothing auto-proposed. Distinct from *view* switching (camera; already exists via
  Lshift).
- **Duration** — a **timed transition**; the crewman **keeps manning the source station** until the
  timer completes, then the assignment flips atomically. Trade-with-latency, **no dead window**.
  **Cancellable, not pausable** (cancel discards the timer; he never left). Anti-spam is the timer
  itself — nothing is gained mid-transit. ~3–5s, tunable.
- **Restrictions** — **anyone → any station.** `competence` is the only gate; an impossible pairing
  is just `competence = 0` (dark though "staffed"). No allow-list system. Tiger MVP: 1.0 home / 0.6
  foreign, no zeros yet.
- **Bodies** — the swap is a **1:1 transposition** (the bijection above): the dead occupant takes the
  survivor's vacated station; the living crewman's killable hitbox moves honestly to the new station;
  the corpse's location is free cosmetics (a corpse absorbs identically anywhere).

**Playtest knobs (register F1):** the transition **cost model** (source-live latency vs both-ends-dark
sacrifice) and the **timer duration** are tuned at the controller, not chosen at the desk.

**Slice-2 build scope:** the occupant-data split (topology B: `Crewman { home }` + `competence`); the
`Capability`/`Group`/`Part` evaluator (general `min`/sum/max — Tiger exercises only `Part`; Pool/Backup
get pure-function unit tests); effectiveness as `f32`; the swap mechanic. Deferred (§9): topology C
(entity separation), graded module damage, the skill/training system, `min`-vs-`product` at the AND
level, min-count floors, per-gun keying (multi-turret), `OneOrMany` serde sugar.

**Build status:** steps 1–3 landed and green — capability model + evaluator (behaviour-preserving),
topology-B occupant split + `competence` (behaviour-preserving), and the swap mechanic
(`PendingSwap` + `tick_swaps`, `SWAP_SECONDS = 4`) with a sandbox crew bar (`1`–`5` tap-source →
tap-target; layer toggles moved to `F1`–`F3`). The sandbox readout shows **scalar effectiveness**
per capability (`Load 60%` when backfilled), not a boolean. GUI-verified. Slice-2 core is complete;
remaining work is deferred refinements (§9) and the eventual game-side (non-sandbox) crew UI.

## 8. Catastrophic & environmental

- **Ammunition** — each shell is modelled **individually** as a ballistic volume + HP. Firing
  **depletes** the stowage, so an emptier rack is a smaller target and less catastrophic (the real
  "empty your ready rack" play). A shell's HP → 0 = **cookoff** = all crew dead (kills all crew
  positions → 0 living crew → tank dead under the §1a-A model). Turret launch is triggered off
  `CookedOff` directly, not via `TankKnockedOut` (see §1a — KO is retired as a gameplay gate).
- **Fire** — an **engine hit by a direct penetrator** (not fragments) has an ignition chance. Fire
  does **not spread**; it does range-per-tick damage to nearby crew/components and can be **put out**
  (a crew repair action). A dedicated **fuel** volume comes later.

## 9. Open tabs (deferred, named so they aren't lost)

- Spall budget *driver* is settled (§5: `cost_paid × v_res² × caliber`); remaining tuning = caliber
  exponent (1 vs 2 = hole-area), an overall fragment cap, continuous vs coarse-tiered count, and
  whether to split out a dedicated `spall_factor` (brittleness) from `material_factor`.
- Ricochet threshold dependencies: pure angle now; later velocity/caliber-scaled.
- HE: fuse minimum-thickness trigger + delay + dense wide cone (penetration-independent).
- Numeric magnitudes (k overmatch ratio, ricochet angle, normalization degrees, material factors,
  DeMarre exponent) — tuned live in the sandbox.
- Data homes: ballistic-volume geometry on the model (watertight); material factor + shell specs in
  RON (ADR-0010 spirit — ADR-0010 already flags per-plate armor as a genuinely node-attached case);
  crew/module/station definitions TBD.
- Repair detail (who, how long, occupies which station).
- Player feedback / legibility — how the player reads *what happened* to their tank.
- **Capability model (§7b) deferred edges:**
  - Cross-group combine: `min` (bottleneck, chosen) vs `product` (compounding) — they agree unless a
    capability has *two* simultaneously-degraded groups; settle when that first arises.
  - **Min-count floors** — groups that don't degrade gracefully ("need ≥2 or it's 0"), vs the linear
    `Pool` share model.
  - **Skill / training system** — competence beyond the static native/foreign split: per-crewman
    skill, "last stand", swap-timer modifiers. All are multipliers on the two computed scalars
    (effectiveness, transition time); the model is purely additive when it lands.
  - **Per-gun keying (multi-turret)** — the one axis groups *don't* cover: `Capability` keyed by the
    operated gun entity, not a global enum (M3 Lee, Char B1, T-35).
  - **`OneOrMany` serde sugar** — let single-`Part` `Pool` members write `(0.5, Engine_L)` instead of
    `(0.5, [Engine_L])`.
  - **Topology C (entity separation)** — promote `Station`/`Crewman` to first-class entities with an
    `Occupies` relationship and reroute the four ballistics deposit sites to the occupant. Cleaner
    than topology B (the fused seat-volume), but pay for it only when crew visually relocate or
    stations gain independent geometry (§7b).

## 10. Architecture & the seam

- **`ballistics` — a library feature plugin** (ADR-0002): projectile spawn + integration + the
  world-space march + ballistic-volume cost + spall + HP deposit + inspection hooks. A **deep
  module**: a small interface (fire a shot; a ballistic volume registers itself) over a large hidden
  implementation. Consumed by *both* front-ends.
- **Split today's `shooting.rs`:** the ballistics/march moves into the shared `ballistics` module;
  `shooting` keeps only the game-specific **gun control** (fire-on-click, reload, recoil), feeding
  `ballistics`. Same mechanic, two triggers (player's gun; sandbox's camera).
- **The sandbox is a second binary in the same crate**, not a separate crate (perpetual sync
  burden) and not an in-game `AppState` (pollutes the shipping app with sandbox-only systems). It
  composes a *subset* of the library's feature plugins + one sandbox plugin. This matches `main.rs`'s
  own note that features live in `GamePlugin` so they can be mounted on an alternate App.

## 11. Armor sandbox v1 — the build

An isolated tool to develop and *tune* the march deterministically, decoupled from driving/aiming.

**Binary:** `src/bin/armor_sandbox.rs`, run with `cargo run --bin armor_sandbox`. App composition:

```
DefaultPlugins + PhysicsPlugins          // runtime + sim
+ spec::plugin + tank::plugin            // the target tank (reused as-is)
+ ballistics::plugin                     // the shared mechanic (new lib module)
+ <sandbox plugin>                       // camera-as-gun, free-fly, time, inspection
```
Deliberately **no** `driving`, `aim`, `camera`, `sight`, `shooting`.

**Sandbox plugin:**

- **Free-fly camera that *is* the gun** — WASD + Ctrl/Shift to float; the shell spawns at camera
  centre and fires straight down the view axis. Inspection camera and firing solution are one object.
- Keys to set **muzzle velocity** and cycle **shell type** (non-positional inputs).
- **Time controls** — pause / slow-motion / single-step.
- **Inspection draw** — the path segments, entry/exit points, per-volume cost, residual velocity at
  each stage, the spall cones, and HP deposited; the **last shot's path frozen** on screen to A/B an
  angle.

**v1 simplifications:** modules/crew are just *named ballistic volumes with HP* — no function, no
backfill, no fire, no cookoff, no HE. Those are §§7–8 and arrive after the march itself feels right.

**API discipline (AGENTS.md):** verify every Bevy 0.19 / avian3d 0.7 API against versioned docs
(`docs.rs/bevy/0.19.0`, `docs.rs/avian3d/0.7.0`, or the `v0.19.0` / `v0.7.0` tags) *before* writing
it. Do not write engine code from memory.

## 12. Model↔code binding contract (LOCKED, 2026-06-27)

The seam between the two parallel workstreams (model authoring vs the `ballistics` march). **Model
side owns geometry** (`.blend` / `.glb`); **code side owns the material/HP scalars** (RON). They do
not edit the same files. Authored to by the model handoff and bound to by the code.

- **Ballistic volumes are named nodes parented to their rig part** (Hull / Turret / Gun), inheriting
  its motion exactly like `*_Collider` (ADR-0008). Turret armor parents under `Turret`.
- **The RON `volumes` map (keyed by node name) is the source of truth** (updated 2026-06-28 —
  *composition, not prefixes*). A node is a ballistic volume **iff it is a key** in
  `<tank>.tank.ron`'s `volumes`. Each entry: `material_factor` (always — shell-resistance per metre)
  plus **optional facets** that layer roles on top; today the only facet is `hp` (present → a
  damageable `ComponentHealth`; absent → pure armour). Composition over a `kind` enum (§2): future
  consequences (cookoff, crew station, fire) add *more* optional facets → each its own ECS component,
  never a central enum. Role and resistance are **independent data**, so a steel barrel is a
  `Module_` with `material_factor: 1000` *and* an `hp`.
- **One naming convention, NOT parsed for behaviour** (updated 2026-06-28): every ballistic volume
  is `Ballistic_<part>` (e.g. `Ballistic_Hull_UFP`, `Ballistic_Engine`, `Ballistic_Gunner`,
  `Ballistic_Ammo_L_0`, `Ballistic_Gun_Barrel`). The old `Armor_/Module_/Crew_/Ammo_` role-prefixes
  are **gone** — role lives in the RON facets, so encoding it in the name too was a second source of
  truth. The single prefix only marks "this mesh is a hitbox" (vs visual skin / `*_Collider` / rig
  nodes — the model's mesh kinds split by *purpose*, not role) and powers the **drift lint** (a
  `Ballistic_*` node absent from `volumes` warns). The code reads the RON, never the prefix; every
  `volumes` key must have a matching node (asserted at bind).
- **Mesh:** watertight / **manifold** solids; convex *not* required (penetration is a raycast query,
  not the physics solver); non-rendering (the sandbox visualises them itself).
- **No numbers in the model.** `material_factor`, `hp`, and future facets live in RON keyed by node
  name (ADR-0010). Model = named manifold solids; code (RON) = all semantics.
- **Mesh kinds split by purpose, not role** (restructure 2026-06-28): the rig is a **skeleton of
  Empties** (`Hull`, `Turret`, `Gun`, `Gun_Barrel`, `Muzzle`, `Wheel_*`, `Track_*`, `Center_Of_Mass`
  — plain names, bound by the rig contract; their origin *is* the pivot). Every mesh is a **leaf**
  under a rig empty, prefixed by purpose: `Visual_*` (rendered skin, no gameplay), `Ballistic_*`
  (the march's hitboxes, concave OK), `Collider_*` (convex physics proxy, Vehicle layer). No mesh is
  ever the parent of another mesh or a rig node, so the art carries no mechanism and the three shapes
  stay independent. `Collider_*` is matched by prefix in `on_tank_ready`.

## Build status (2026-06-27)

- **Done & verified:** `shooting` → `ballistics` split (shared mechanic, trigger-agnostic `FireShell`
  event); `bin/armor_sandbox` + `sandbox.rs` — free-fly camera-gun (heading-relative WASD,
  Shift/Ctrl altitude), pause + 5-step slow-mo + `[`/`]` fine time control, shell tracer gizmo.
  **Geometric march cut:** `Layer::Armor` + `BallisticVolume`; the step march crosses volumes
  recording entry/exit (geometric line-of-sight thickness via a `solid=false` exit probe restricted
  to the same entity), stops at terrain, handles multiple volumes per shot; `PenetrationMarks`
  inspection gizmos (entry/exit/through-span). `cargo test` green.
- **Velocity cost cut:** `capability(speed) = K·speed^N` (DeMarre) and its inverse; a crossing spends
  `cost = LOS_metres × material_factor` and drops to the residual speed (Lambert–Jonas shape). When
  capability ≤ cost the shell **embeds** partway and stops. Placeholder plates now show
  perforate-then-embed. `0` joins `P` as a pause key.
- **Path bending cut:** `integrate_projectiles` is now a true ray-march carrying position +
  direction + speed, so bends survive across the step. **Normalization** straightens the round toward
  the inward normal on entry (a share of the incidence); **ricochet** deflects (specular, speed
  bled) off faces past ~70° without entering — the deflected segment lives in world space and can hit
  the next surface. Ricochet points drawn as cyan markers.
- **Information layer:** `ballistics` exposes a per-shell `ShellReadout` (speed, remaining
  capability); the sandbox draws a keybindings legend, a status line (time scale + shell count), and
  pooled labels floating beside each shell (speed / capability / plates crossed) via
  `world_to_viewport`. "Slower" is now a number.
- **Overmatch cut:** the shell carries a `caliber`; at each crossing the march probes the plate's
  thickness *along its normal* and, when `caliber ≥ 3 × thickness`, suppresses ricochet and nearly
  fully normalizes (cancelling slope). Sandbox plates are now a steel thickness ladder
  (15/50/100/300 mm); overmatched crossings draw magenta.
- **Real model bound:** `on_tank_ready` attaches a query-only trimesh collider (`Armor` layer,
  `filters = NONE`, so no physics response) + a `BallisticVolume` to each `Armor_/Module_/Crew_/Ammo_`
  node; the march resolves the volume by walking up from the hit mesh-primitive to the named parent
  (`ChildOf`). Material factor is **provisional, role-keyed** (`ballistics::material_factor`) pending
  RON authoring. The sandbox now spawns the real Tiger as a **static** target (reusing `on_tank_ready`
  via `spec`), alongside the placeholder slabs. Game + sandbox bind with no panic.
- **Spall + HP cut (2026-06-28):** per-component HP pools (`ComponentHealth`, role-keyed
  `component_hp` — crew 3 / module 10 / ammo 2; armor 0), bound to `Component` nodes in
  `on_tank_ready`. On every perforation exit the march throws a fixed-shape cone (symmetric, exit-dir
  axis, deterministic golden-angle fragment pattern denser on-axis); budget `= MAX × (cost/ref) ×
  (v_res/ref)² × (caliber/ref)` (§5 product model). Each fragment ray-casts to the first ballistic
  volume, deposits 1 HP, and stops (armor shadows for free). The main penetrator's transit deposits
  `cost × TRANSIT_K` into a crossed component (embed deposits `cap`). Sandbox draws spall cones +
  fragment rays (hot = HP deposited, grey = shadowed) and floats HP labels over damaged components;
  `c` resets component HP. `cargo test` green, sandbox runs clean. Constants are sandbox knobs.
- **Air drag cut (2026-06-28):** shells now bleed speed in flight — quadratic drag `dv/dt = −k·v²`
  integrated analytically (`v ← v/(1 + k·v·dt)`, stable), lumped const `DRAG_K` (sandbox-tunable). The
  point is **range-dependent penetration**: `capability ∝ vⁿ` now falls with distance, so a far shot
  can bounce where a near one perforates — visible live via the shell's speed/capability label. One AP
  value for now; the per-shell ballistic coefficient (the APCR-vs-APDS range-falloff differentiator)
  joins the shell-data struct later. Fragments stay hitscan — their drag will be a distance term in
  the future energy-packet model, not a flight integrator. Deliberately *not* doing full exterior
  ballistics (wind, altitude density, spin drift) — out of slice.
- **Energy-packet fragment cut (2026-06-28):** spall fragments upgraded from 1-HP/no-pen tokens to
  energy packets. `spall_directions` now returns each fragment's on-axis position `t`; count scales
  with `cost × caliber`, per-fragment birth penetration = `FRAG_PEN_MAX × shot_energy(v_res²) ×
  (1−t)`. New `cast_spall_fragment` marches each as a mini-penetrator: deposits `pen ×
  FRAG_DMG_PER_MM` HP on a component, then punches through thin volumes (spending `span × factor`) or
  stops in thick ones; `pen` bleeds with distance via `FRAG_DRAG`. Thick volumes still shadow; thin
  ones can be defeated; strong fragments can exit + reach another tank. Sandbox draw unchanged (reads
  end + deposited; rays now run longer when a fragment penetrates). Consts `FRAG_PEN_MAX/DRAG/
  DMG_PER_MM` are sandbox knobs. `cargo test` green, sandbox runs clean.
- **Mass-driven penetration cut (2026-06-28):** capability went from speed-only to
  `PEN_K · mass^MASS_EXP · speed^PEN_N` (MASS_EXP 0.5) — mass is the primary driver, caliber stays for
  overmatch/spall only. `FireShell`/`Projectile` carry `mass`; `capability`/`speed_for` take it.
  Re-calibrated `PEN_K` (0.01853→0.0058) so the 88 (10.2 kg @773) = 250 mm *unchanged* (zero
  regression); a 13 g MG round now lands ~9 mm RHA, so small arms can't defeat real armour but chip
  exposed modules. The linchpin for small-arms-vs-main-gun tiering and future shell types. `cargo
  test` green, sandbox clean.
- **Ricochet-shock cut (2026-06-28):** a ricochet off a *component* (not armor) now deposits shock
  damage `SHOCK_K · capability · cos(incidence)` — scaled by impact energy and squareness. So a
  grazing main-gun bounce chips an exposed module's integrity without one-shotting it (~3.7 of a 10-HP
  module at a 71° graze, ~0.4 at 88°), a bullet graze barely registers (~0.1), and armor (no HP) takes
  nothing. Completes the gun-barrel damage story (direct = kill, graze = chip, fragment = energy-
  scaled). `SHOCK_K` is a sandbox knob. Not yet drawn distinctly (HP labels show the effect).
- **Volume data → RON (composition) cut (2026-06-28):** retired the prefix-keyed `material_factor`/
  `component_hp` functions; the per-tank `volumes` RON map is now the source of truth. `VolumeSpec`
  = `material_factor` + optional `hp` facet (composition, not a `kind` enum — §12). `on_tank_ready`
  binds by iterating `spec.volumes` (node is a volume iff a key), `hp` present → `ComponentHealth`,
  absent → `ArmorVolume`; prefixes demoted to a drift lint; every RON key asserted to have a node.
  All 45 Tiger volumes authored + bound clean; resistance now decoupled from role (barrel = steel
  1000 *and* a module). Schema test covers it. **Consequences (§§7–8) now slot in as new facets.**
- **Single `Ballistic_` prefix cut (2026-06-28):** retired the four `Armor_/Module_/Crew_/Ammo_`
  role-prefixes (a second source of truth for role, now owned by RON) → all 44 volumes are
  `Ballistic_<part>`. Renamed across `.blend` (headless Blender), `.glb` (surgical JSON node-name
  patch — visual untouched, no re-export), the RON keys, and the bind's drift lint. The model's mesh
  kinds now split by *purpose* (ballistic / visual / collider / rig), not role. Bonus: the merged
  barrel mesh (Blender kept the name `Armor_Gun_Barrel`) became `Ballistic_Gun_Barrel` and the RON's
  `hp` makes it a module — name no longer matters. Binds clean, zero drift.
- **Model restructure cut (2026-06-28):** rig nodes (`Hull`, `Turret`, `Gun`, `Gun_Barrel`, 16
  `Wheel_*`) were meshes doing double duty as visual + pivot; converted to **Empties** (pivots) with
  their geometry demoted to `Visual_*` leaves — done headless via Blender (`mesh→empty` + transform-
  preserving re-parent), re-exported, verified (74 meshes / 3 materials / 3 images unchanged, modifiers
  0). Running gear → `Visual_*`; `Hull_Collider`→`Collider_Hull`, dormant `Turret_Collision`→
  `Collider_Turret` (now a proper convex proxy). Code: collider match → `Collider_` prefix. Three
  shape kinds (Visual / Ballistic / Collider) now split by purpose; rig is a pure empty skeleton. Binds
  clean. **Visual skin needs a human eyeball in the sandbox** (counts match, so geometry should be
  intact; backups in scratchpad).
- **Next:** crew position capabilities + per-crew capability gating (§7a). `CrewStation` (enum)
  → `Position { capabilities }` (composition). v1 capability set = {Drive, Traverse, Fire, Reload,
  GunnerSight, CommanderView}. Capability available iff staffed by a living crewman AND relevant
  module HP > 0. `Reload` becomes a `Gun` component (per-gun, not a singleton resource). View
  auto-fallback on view-crewman death. `TankKnockedOut` retired as a gameplay gate (§1a-A); the
  cookoff turret-launch hook moves from `TankKnockedOut`-triggered to `CookedOff`-triggered. Slice
  2 (§7b — backfill/position swapping) is the next design interview.
