# Design sketch: RON-authored rig contract & composable tank primitives

**Status: SUPERSEDED (built 2026-06-30).** The decided parts graduated to
[`0012-spec-driven-rig-binder`](../adr/0012-spec-driven-rig-binder.md) (iterate-the-spec binder,
naming contract, bidirectional fail-fast) and
[`0013-composable-rig-control`](../adr/0013-composable-rig-control.md) (servos ⊥ weapons ⊥ views,
pure servos on one aim point, layered capability gates). This file is kept for the design *reasoning*
(the seam analysis below), but the ADRs are now authoritative — and note one deliberate **deviation**:
"axis-as-node" (§ below) was **rejected**; a servo's axis is derived from a RON-declared `role`, not
from node orientation (see 0012, Considered options).

---

**Original status (PROVISIONAL — idea, not a decision).** Recorded 2026-06-26 so the design thinking
isn't lost; **deliberately deferred** behind the single-player vertical slice. Expect parts of
this to be wrong once we actually build it — we have exactly one tank (Tiger I) to reason from,
and you can't design good primitives from one example. This is a starting hypothesis, not a spec.

Vocabulary from `.agents/skills/codebase-design` (seam, depth, leverage) is used deliberately.

## Problem (today)

The rig bind in `on_tank_ready` (`src/tank.rs`) holds the **join** between three artifacts in
*code*: a hardcoded `match name { "Turret" => … }` maps node names to behaviour, and a separate
`REQUIRED` array lists the contract. That join is:

- **Duplicated three ways** — the match arms, the `REQUIRED` list, and (implicitly) the RON's
  `turret`/`gun` field names must be kept in sync by hand.
- **Singleton-bound** — it hardcodes "a tank has *a* turret, *a* gun." Code can't enumerate the
  parts of an *arbitrary* tank, so composability is structurally blocked.
- **Untested** — verified only by `cargo run` + log-greps.

## The move: get the join out of code, into data

Re-home the three concerns so the **node name/path is the join key**:

- **Model (Blender)** owns geometry, the node tree, and **axes-as-orientation** (a pivot's own
  orientation *is* its hinge axis — see below).
- **RON** owns the **part list + each part's role + its scalar data** — "there is a yaw servo
  driving node `Turret_Main` (limits …); a pitch servo on `Gun_Main`; N contact stations/side."
  RON **names the actual node** it binds to.
- **Code** owns **primitive behaviours** (servo, recoil, suspension, drive) that attach to any
  node the RON tags — a small interface (a primitive) behind which a lot of behaviour sits.

This keeps ADR-0002's spirit ("name = the contract," reactive attachment by name): the model
still owns names. What moves is the **role assignment and required-set**, from hardcoded-in-code
to declared-in-RON. The rig contract becomes **one declarative list with three consumers**: the
bind iterates it, the load-time assert checks it (fatal, per ADR-0011), and the test joins it
against the model. One source of truth, no hand-sync.

### Decided-for-now: RON is authoritative

When RON and the model disagree about *what parts exist*, **RON wins** — it is the SOT for *what
the tank is*; the model is the SOT for *geometry / where things are*; the test proves they agree.
"Compose a tank" = write RON against a library of model primitives. (Caveat: this is the user's
call as of 2026-06-26, made before building. Revisit if composing-in-Blender turns out to feel
more natural in practice.)

## Axis-as-node (highest-leverage single change)

Split each pivot into its own node and read the rotation axis from that node's **orientation**
(convention: a pivot hinges about its local +Y, say), instead of the `Axis::X/Y/Z` enum in
`ServoSpec`. Earns four things at once:

- **Simplification** — the `Axis` enum disappears. (Its own comment admits it can't express a
  canted mount; node orientation does, for free.)
- **Composability** — 3-axis mounts (Turm III) are three nested pivot nodes; Bevy's hierarchical
  transforms compose them. `drive_servos` is *already* a query over N servos. Zero gun servos
  (Strv 103, gun fixed to hull) is just *no servo node*.
- **SOT clarity** — the axis lives where the geometry lives.
- **Testability** — "pivot node exists and is oriented sanely" is checkable.

Unifies with recoil: the glossary already calls recoil "the bore-axis cousin of the Servo." A
pivot's *rotation* axis and a slider's *translation* axis are both "the node's local axis" → one
1-DOF-motor primitive, two flavours. Pivot/collider split (pivot = rotating frame, collider as
its child) matches GLOSSARY.md's "part layers compose on the part."

## Validation: test the path that ships

The "RON matches the model" test should run the **real bind headlessly** (MinimalPlugins + the
actual `on_tank_ready` against the real `.glb`/`.ron`), asserting markers land, the contract
holds, and `ComputedMass`/COM/inertia equal authored values. **Do not** write a second glTF
parser to read node names — Bevy mangles glTF names and splits mesh primitives, so a separate
parser tests a *different* name-resolution path than ships: it would pass while the game panics.

## Realism checks (don't over-believe the dream)

- **"Any tank from primitives" is a north star, not a milestone.** Good primitive boundaries only
  appear under the *tension* of tanks that genuinely disagree: Strv 103 (no gun servo; aiming is
  hull pitch + yaw — a different *control scheme*), a multi-turret pre-war heavy (Char 2C / T-35),
  a wheeled vehicle (Ackermann steer ≠ skid steer — a different *drive model*). **First real step
  after the prerequisite: add one deliberately-weird second tank and let it reveal the seams** —
  don't abstract around the Tiger.
- **Rig ≠ control layer.** The rig holding N parts is the easy half (servos are already a query).
  The hard half — *which* weapon the player's sight drives vs. AI/secondary, and the singleton
  assumptions in `apply_drive`/`apply_suspension` (`body.single()`) and the aim system — is
  control-layer work. The rig redesign should leave a clean seam for control to pick which parts
  it drives, and **defer** "who aims the second turret."

## Provisional glossary terms (NOT yet in GLOSSARY.md)

Parked here, not promoted — GLOSSARY.md is glossary-for-the-built-model only. Promote on build.

- **Primitive** — a reusable rig building block with one behaviour (servo/recoil/suspension/drive
  station) that attaches to a tagged node.
- **Part declaration** — a RON entry naming a model node and the primitive + data to bind to it.
- **Rig contract (data-driven)** — refinement of GLOSSARY.md's "rig contract": the set of part
  declarations a tank's RON requires, checked against the model at load and in tests.

## First concrete step (the prerequisite — when the slice is done)

1. Collapse the contract into one declarative list (RON declares parts+roles+required-ness; bind
   iterates it; assert and test consume the same list). Dissolves the `match` arms and `REQUIRED`.
2. Axis-from-node; drop the `Axis` enum — first proof a primitive can be structure-driven.
3. The headless asset-validation test against the real bind.

Then add the weird second tank and let it tell you where the primitives actually are.
