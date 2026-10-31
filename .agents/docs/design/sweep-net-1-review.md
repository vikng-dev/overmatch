# Net sweep 1: findings and disposition

Subject: the game's netcode, match flow and damage model; game state only.

Base: `9c0e716e`. Lane: `lane/sweep-net-1`, bay-2. The MEASURED start was
2026-11-01 05:45:30 +07; the DERIVED hard stop is 07:15:30 +07. No subagents, captures,
pricing runs, deployment calls, external-repository writes or pushes were used.
Builds and tests used the serial queue, CI features, unattended/headless settings.

## battle-243 review first

The implementation and local upstream draft were read against the error-handling,
code-branching and base-truths lenses. The supplied dispatch was the review specification.

| Finding | Disposition | Evidence / implementation |
|---|---|---|
| B243-clock, P2 | Fixed | Input time intentionally leads the server; acknowledging that lead without a bound did not justify using it for the match display. `battle_hud` now uses the shared presentation clock and waits for synchronization. Phase and result still come exclusively from replicated `BattleState`. Its fixture deliberately puts input time ahead of presentation time. |
| B243-roster, new P2 | Fixed | The unbounded table could extend below the screen. One bounded viewport keeps columns aligned, with Page Up/Down controls, scaled logical scroll coordinates and clamping. The card's bound also includes its title/hint, and a hidden or replacement view resets the scroll position. |
| B240-hosted-wire, supplied P3 | Fixed | `Hosted::install` requires the admission resource instead of constructing a new gate. The server root supplies the gate shared with netcode; bare hosting fixtures now provide theirs explicitly. |
| B240-results-respawn, supplied P3 | Fixed | The replicated phase cancels the pending respawn and makes its key inert outside combat, including while the old knocked-out replica remains. The regression keeps that replica through results. |
| B243-derived result / score | No finding | The client does not advance a phase or pick a winner. Rows use the authority's rank comparator; the title and winner mark use the replicated outcome, including a fixture whose named winner is below another row. |
| B243-font | No finding | Rendered strings pass the bundled font-coverage gate. Added paging text is ASCII. |
| B243-upstream draft | Accurate to patch | The getter preserves the authenticated denial across terminal reset and clears it in `connect`, also used by address failover. The response-stage admission recheck follows the capacity recheck and precedes connection confirmation. The draft states its stateless-gate limitation and the retained pending-cache property. Nothing was filed externally. |

No P1 was found in battle-243. Its supplied P3s are fixed above; no additional
battle-243 P3 remains open.

## Assigned inventory

| Finding ID | Disposition | Evidence / implementation |
|---|---|---|
| EH-N1 shared lever parser and every caller | Fixed | `LeverError` distinguishes absent, malformed and non-UTF-8 values. Flags have an explicit vocabulary; numbers cannot silently become unset. Composition adapters visibly refuse startup with status 2. All callers, including generic and outside-net callers, were updated. |
| EH-N1 aim token dropping / zero spawn quaternion | Already fixed in the base; strengthened | The RON harness had replaced the old token-dropping parser and rejected non-finite inputs and zero rotations. Spawn poses now normalize once at the parsing edge using magnitude scaling; large and tiny finite rotations cannot overflow or underflow that normalization. Script syntax, domain and IO errors are typed and refuse startup. Private pose fields prevent callers from bypassing its checked constructor. |
| EH-N1 bot count | Fixed | Only absence, a `u32` count, or documented bare empty/on forms are accepted; malformed values no longer spawn one bot. |
| EH-N1 stats period | Fixed; partly disagreed | Malformed, negative, non-finite, unrepresentable or rounded-to-zero periods are refused. Literal `0` and `false` were documented off forms, so their deliberate disabling was retained; `off` is explicit too. Empty/on retains the documented default. |
| EH-N1 bind and other environment UTF-8 sites | Fixed | Bind, direct server/API, bot, debris and stats reads no longer turn non-UTF-8 input into absence. Bind keeps its documented unset/empty wildcard form. |
| EH-N1 client refused startup exits successfully | Fixed | Link setup retains typed syntax/DNS/lever causes and exits with status 2 on refusal. |
| EH-N1 spawn-pose parsing doc | Already fixed in the base; strengthened | `Script` was already cached in `OnceLock`. The pose now also carries parsed position/unit rotation rather than normalizing on every spawn. |
| BT-N1 incomplete extrapolation horizon | Fixed | Unknown contact/recoil impulses, harder-gripping rubble, unsupported motion and angular motion invalidate the traction-only certificate. Projection, its blend-back, the delay discount and dead helpers were removed. `frontier` is read-only; the buffer and debug readout cover the full observed receipt spread. The canonical rationale and tradeoff are in `net-motion-contract.md`, linked from ADR-0037. |
| BT-N2 offline transmission override | Already fixed in the base | `run_offline` composes the normal game plugins and does not override the declared transmission. No obsolete transmission-selection path remains in `lib.rs`. |

## New findings resolved

- Partial periodic stalls, an overflowing period width, and Lightyear's signed-millisecond
  narrowing could previously disable a workload or panic. A checked `Burst` and explicit
  conditioner-domain refusal close those edges.
- The spawn-map radius, bot/harness lane spacing and debris-fixture flank offset assumed a
  fixed construction. They now use `TankContent::contact_radius`, derived from declared proxy
  vertices/transforms and prepared running-gear shapes. The sphere remains conservative under
  rotations and nonuniform scale. Automatic placement applies the greater of mode and physical
  separation. The authority's current single immutable blueprint supplies both occupant and
  candidate radii; that invariant is named at the resolver.
- The construction fixture cross-checks the bound against actual spawned contact-hull vertices.
  Ray-query surfaces have no contact filters and are not contact geometry.

## Verification and owner follow-up

MEASURED green batches: lever/adapters **91** tests; stream/horizon **27** tests;
construction/placement/headless **62** tests; final UI/admission/composition **54** tests;
closing parser/delay/citation **19** tests. Each implementation commit passed formatting,
locked workspace/all-target CI clippy with `dev_ui,bitprobe`, its targeted tests and
`doc_citations`. UI/font/ownership, admission real-UDP probes and kill-cam composition also pass.
The live hosted smoke stayed ignored because it calls the Worker and can deploy; three
measurement beds stayed ignored. No full suite or performance measurement was run.

The owner should batch feel review and pricing of the full receipt-spread buffer. Clean links
keep the interval floor; variable receipt streams can require more presentation delay. This is
a deliberate removal of an unsupported error certificate, not a new feel certification. Later
inventory lanes EH-N2–N4 remain outside this dispatch and were not swept into this change.

MEASURED closing clock: 2026-11-01 06:49:39 +0700. DERIVED elapsed from the clock reads: 64.2 minutes,
within the dispatched limit. The lane is committed for orchestrator merge; it never pushes or
releases its bay.
