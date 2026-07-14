# Practitioner consensus: high-rate shooting replication

**Question.** What do shipped-game engineers and experienced multiplayer
practitioners recommend for high-rate weapon fire, projectile/tracer cosmetics,
ricochets, impact outcomes, and authoritative damage feedback in a roughly
20–40-player server-authoritative shooter?

**Scope and method.** This is the deliberately broad “wisdom of practitioners”
track, not an API or standards audit. I searched official engine/transport
documentation, GDC and Game Developer material authored by shipped-game
engineers, and explicitly-labelled community discussions. Repository documents
were not used as evidence. Sources were checked on 2026-07-13. “Consensus” below
means a repeated design pattern across independent production accounts and engine
guidance; it is not a claim that every game or transport implements it identically.

## Bottom line

**No: practitioner consensus does not support sending every visible MG round as
an individually reliable event to every visible player.** The recurring split is:

1. Keep *gameplay truth* on the authority. Validate firing, simulate the shot,
   decide collision/damage, and make durable state converge from the server.
2. Make the firing player feel immediate through local prediction (muzzle, sound,
   recoil, and an explicitly provisional tracer), then correct/reconcile from the
   authority.
3. Represent sustained automatic fire as a compact state/counter/cadence plus
   occasional best-effort visual detail, not an ACK-required bullet stream.
4. Spend reliable delivery on sparse, meaningful transitions: fire start/stop or
   a repair state, cannon spawn/bounce/terminal outcome where its absence would be
   conspicuous, and owner-facing authoritative damage confirmation. Give these
   independent lanes/priorities where the transport permits it.
5. Use `ShotId`/generation/segment IDs, acknowledgements or snapshots, and a
   bounded stale-event policy. Reliability is not permission to render an old
   event late.

The recommendation is therefore a hybrid, not “everything unreliable”: a
loss-tolerant cosmetic carrier plus small reliable authoritative consequences and
repair. It preserves Overmatch’s server authority and stable `ShotId` dedup while
making visual fidelity an intentionally playtested product choice.

## Why the per-round reliable design is the wrong default

### Scale check (all figures **DERIVED**, not measured)

The stated maximum is two 750-rpm MGs per tank:

| Assumption | Derivation | Result |
|---|---:|---:|
| One tank’s two MGs | `2 × 750 / 60` | **25 rounds/s** |
| 20 firing tanks | `20 × 25` | **500 rounds/s** |
| 30 firing tanks | `30 × 25` | **750 rounds/s** |
| 20 / 30 tanks at 64 Hz | `500 / 64`; `750 / 64` | **7.81 / 11.72 rounds per tick on average** |
| 30 tanks, every round to every *other* tank | `750 × 29` | **21,750 recipient-deliveries/s** |
| A 60-round same-tick burst fan-out | `60 × 29` | **1,740 recipient-deliveries in that tick** |

The last row does not assert that all 29 recipients are interested; it shows why
interest filtering is decisive. Nor does it estimate wire bandwidth: serialization,
packet headers, encryption, ACKs, retransmissions, coalescing, and actual
visibility determine that. As a deliberately small illustration, a hypothetical
16-byte per-round payload alone would be `60 × 16 = 960` bytes to **one** recipient
in the burst tick, before any framing. It leaves very little headroom before a
typical datagram-sized packet and says nothing about reliable retries.

This pressure matches production and engine guidance rather than merely a paper
calculation. Source networking says it cannot send every world change to every
client and uses snapshots; Unreal warns that high-frequency reliable RPCs can
overflow their queue and disconnect a player; Unity’s sample explicitly says a
single lost attack VFX is usually acceptable because later state supplies the
consequence. [Valve, *Source Multiplayer Networking*, live documentation, accessed
2026-07-13](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking)
[Epic, *Networking Overview*, UE 4.27 documentation, accessed
2026-07-13](https://dev.epicgames.com/documentation/en-us/unreal-engine/networking-overview?application_version=4.27)
[Unity, *Optimizing Boss Room Performance*, 2023-02-01](https://docs-multiplayer.unity3d.com/netcode/1.3.0/learn/bossroom/optimizing-bossroom/)

## Evidence, weighted by provenance

### Shipped-game engineers and production material

| Source | What it actually shows | Weight and limitation |
|---|---|---|
| Dan Reed (Blizzard), [*Networking Scripted Weapons and Abilities in Overwatch* (GDC 2017)](https://media.gdcvault.com/gdc2017/Presentations/Reed_Dan_NetworkingScriptedWeapons.pdf) | A senior gameplay engineer describes server-authoritative weapon/ability scripting: client button/aim input, server simulation and deltas, local prediction with rollback/replay, redundant/out-of-order packet rejection, and remote-specific sync selection. His measured example reports **0.4 Kb/s remote** versus 1.0 Kb/s local for Tracer firing a full clip and reloading over exactly two seconds. That is a compact behavioral sync result, not a claim about per-bullet packets. | High: direct, named production engineer and primary slides. It explicitly excludes projectile and hit-registration implementation details, so it supports the *state/prediction/dedup pattern*, not an exact weapon protocol. |
| Peter Kao (Insomniac), [*Always Online*, *Game Developer*, February 2012](https://media.gdcvault.com/GD_Mag_Archives/GDM_February_2012.pdf) | In the Overstrike/Fuse development account, Kao says most bullets were not synchronization objects because they were short-lived and had no state to update after creation; projectiles were created locally and broadcast with basic messages. The article says projectiles’ quantity made bandwidth optimization necessary, while damage was synchronized as event-driven basic messages. | High for a historically real studio practice, though it is a four-player co-op project, pre-release material, and not a prescription for a competitive 30-player tank game. Its useful lesson is the separation of short-lived projectile/view handling from durable game state. |
| [*Source Multiplayer Networking* (Valve Developer Community), live docs](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking) | Documents authoritative client-server play, small high-frequency packets, snapshots instead of a packet for each world change, prediction/interpolation, compression, and lag compensation. | Medium-high: maintained technical community documentation for a shipped engine; broad architecture rather than a weapon-specific postmortem. |

The two production accounts converge on a crucial distinction: a player’s immediate
experience may be predicted, while network representation is selected for semantic
state and efficiency. Reed’s remote/local asymmetry is especially relevant: remote
observers need not receive everything the owning player needs.

### Engine and transport guidance: repeated division by consequence

| Source | Relevant guidance | Implication here |
|---|---|---|
| [Epic, *Networking Overview*, UE 4.27](https://dev.epicgames.com/documentation/en-us/unreal-engine/networking-overview?application_version=4.27) | Unreliable RPCs suit frequent/noncritical work; reliable RPCs suit infrequent critical events, including firing start/end. It warns that reliable calls every frame can overflow the queue and force a disconnect. | A continuous MG needs a state transition/cadence representation, not a reliable RPC per round. |
| [Epic, *RPCs*, UE 4.27](https://dev.epicgames.com/documentation/unreal-engine/rpcs?application_version=4.27&lang=en-US) | Names sound, particles, and other temporary effects as the primary use for unreliable events. | A missed isolated tracer, spark, or audio tick is an intended loss-tolerant category if later authoritative state remains clear. |
| [Unity, *Reliability*, Netcode for GameObjects 1.0, 2023-02-01](https://docs-multiplayer.unity3d.com/netcode/1.0.0/advanced-topics/message-system/reliability/) | RPCs are reliable by default, but Unity names particles/sounds as common noncritical unreliable work, warns that retries add bandwidth under bad networks, and suggests splitting critical and noncritical fields. Reliable order is only per `NetworkObject`. | Do not put cosmetic MG traffic behind an ordered reliable stream shared with consequences. Split payload/classes and test loss, rather than trusting LAN tests. |
| [Unity, *Optimizing Boss Room Performance*, 2023-02-01](https://docs-multiplayer.unity3d.com/netcode/1.3.0/learn/bossroom/optimizing-bossroom/) | The authors say missing an individual attack VFX need not matter because state/hit points arrive later; they recommend unreliable RPCs where possible and show queue/tick-rate tradeoffs. | This is the clearest official statement of the visual concession: a lost isolated effect is acceptable; lost outcome state is not. |
| [Godot, *MultiplayerPeer*, 4.3 docs](https://docs.godotengine.org/en/4.3/classes/class_multiplayerpeer.html) and [*High-level multiplayer*, 4.2 docs](https://docs.godotengine.org/en/4.2/tutorials/networking/high_level_multiplayer.html) | Separates unreliable, unreliable-ordered, and reliable modes; advises reliable sparingly for critical events and separate channels so an acknowledged chat/control packet does not stall gameplay. | Put consequence/control and rapid cosmetic/state paths on distinct channels/lane classes. Avoid assuming ordering across them. |
| [Valve, *ISteamNetworkingSockets*, live Steamworks docs](https://partner.steamgames.com/doc/api/ISteamnetworkingSockets) | Reliable messages arrive ordered on a lane; unreliable messages can be dropped/out of order relative to either class. Lanes exist to control head-of-line blocking and priority/weight. Nagle-style buffering may coalesce small messages. | Stable IDs and explicit receive rules remain necessary. Priority/lane use is a design feature, not a reason to turn MG bullets reliable. |
| [Lightyear, *Replication Logic*, current book](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/replication_logic.html) | Entity actions are ordered reliable; entity updates are sequenced unreliable, discard older updates, and carry monotonically increasing message IDs to preserve a consistent group. | This is a directly relevant modern precedent for “reliable structure + newest-wins state,” and for discarding stale arrivals. It is explanatory documentation, not an approved Overmatch API design. |
| [Lightyear 0.28 crate docs](https://docs.rs/lightyear/latest/lightyear/) | The library supports server-client authority, UDP/netcode IO, channels, prediction/rollback, interpolation, messages, and replication as separate layers. | The desired representation fits the library’s conceptual layers. Exact 0.28 APIs/settings must still be verified in the implementation task. |

Representative wording is unusually consistent. Unreal says reliable functions are for
events that are “critical … but do not get called very frequently”; Unity calls
particle/sound events noncritical; Godot calls reliable “potentially the slowest” and
asks whether data is really critical. Those are guidelines, not hard rate limits.

### Community evidence — useful pattern signal, not proof

| Discussion | Contribution | Credibility treatment |
|---|---|---|
| [Epic forums, *Bullet projectiles* (2015)](https://forums.unrealengine.com/t/bullet-projectiles/47950) | A recurring community proposal is reliable trigger press/release plus local timer/cadence for continuous fire, rather than a fire RPC for each shot. | Low-medium. This is an experienced forum answer but not an audited production statement; the thread’s game-specific claims are not relied on. It independently matches the official Unreal advice. |
| [GameDev Stack Exchange, *Shooting bullets without players being able to cheat* (2018)](https://gamedev.stackexchange.com/questions/163592/shooting-bullets-without-players-being-able-to-cheat) | Recommends immediate local bullet/animation feedback while the server alone decides damage and tells clients the outcome. | Medium for a reasoned, reviewed explanation, not evidence that a particular shipped title uses it. It triangulates the authoritative-outcome/predicted-feedback split. |
| [r/gamedev, *A few basic questions about RPCs* (2022)](https://www.reddit.com/r/gamedev/comments/yvy085/) | A self-described AAA networking practitioner says tracers and muzzle flashes are generally lossy/high-throughput, while authority remains elsewhere. | Low. Anonymous and unverified, so it carries no unique conclusion; included only because it independently echoes the engine and production sources. |

The community material is therefore not the basis for the recommendation. Its value is
that it repeats the same separation without contradicting higher-quality sources.

## What is consensus, and what is conditional

### Strong convergence

- **Authoritative damage and server validation.** The client may request/forecast
  a shot, but it does not decide whether a target was damaged. This supports
  anti-cheat, reconciliation, and a dependable damage confirmation path.
- **Prediction for owner feel.** A round-trip before muzzle response is visibly
  poor. Overwatch’s talk gives the production pattern: retain local input/history,
  accept server state, then rollback/replicate/simulate if necessary.
- **Loss tolerance for high-rate transient visuals.** One missing member of a
  sustained MG stream is normally less harmful than retransmit queue growth or
  head-of-line delay. Unity’s example explicitly accepts a lost attack visual.
- **Snapshot/counter repair and stale rejection.** Source uses snapshots; Lightyear
  describes sequenced newest-wins updates and message IDs; Reed describes ignoring
  redundant/out-of-order packets. A receiver must know which event/state supersedes
  another.
- **Interest/priority matters.** Source’s snapshot constraint and Photon’s
  [interest-management documentation](https://doc.photonengine.com/fusion/current/manual/advanced/interest-management)
  both make selective replication a first-class scale lever. Note Photon cautions
  that state interest does not automatically filter RPCs, so a game must apply its
  own recipient policy for event traffic.
- **Avoid a single reliable ordered bottleneck.** Fiedler explains why waiting for
  a loss makes newer time-sensitive data stale; Steam and Godot provide lanes/channels
  to isolate independent traffic. [Glenn Fiedler, *Reliability and Congestion
  Avoidance over UDP*, 2008-10-20](https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/)
  [Glenn Fiedler, *Client Server Connection*, 2016-09-28](https://www.gafferongames.com/post/client_server_connection/)

### Contested or genre-dependent choices

- **Every projectile as a synchronized entity vs. a deterministic visual.** Slow,
  collision-relevant rockets/cannon shells can warrant a networked authoritative
  entity or explicit continuation updates. Very fast bullets often do not. There is
  no universal cut-over: speed, travel time, visible trajectory, bounce count,
  collision rules, and desired spectator fidelity decide it.
- **Exact visual round count.** Mil-sim/replay/anti-cheat presentation may require
  more exact tracer accounting than an arcade tank battle. That requirement should
  be written as a product invariant, then budgeted; it is not supplied by generic
  netcode advice.
- **Reliable fire start/stop versus replicated firing state.** Both can work.
  A reliable transition is intuitive for an infrequent input edge; a replicated
  generation/counter is better for join-in-progress and repair. The robust design
  can use both semantics without sending each bullet reliably.
- **Visual truth under loss.** The sources tolerate a missing temporary effect,
  not a player being unable to understand a cannon’s decisive, visible ricochet.
  How much discontinuity is acceptable is a playtest decision.

## Practical recommendation for Overmatch (proposal, not implementation)

Use the following message *semantics*. Channel names and exact Lightyear 0.28 APIs
are intentionally not prescribed here; a versioned API audit should do that before
code is written.

| Concern | Suggested representation | Delivery/receive policy | If a packet is lost |
|---|---|---|---|
| Owner presses/holds MG trigger | Existing input stream carries held state plus input tick; client immediately predicts local presentation. Server enforces rate/ammo/aim and runs the actual shots. | Input/prediction system; server remains sole damage authority. | Later inputs/state correct held state; owner never waits to show muzzle/recoil. |
| Remote sustained MG | `FireStream { weapon, generation, start_tick, cadence/preset, seed, last_authoritative_shot_or_count }`; optional compact per-tick `FireVisualBatch` for near/visible recipients. Remote emits scheduled tracer/muzzle/sound cosmetically. | The stream state/generation is repairable (reliable or snapshot/counter); individual visual batches are best-effort and expiry-bounded. Key by shooter/weapon/generation. | A remote may miss a tracer or short sound tick, then resume on next state/counter/batch. It does **not** invent damage. |
| MG impact sparks/dust | A budgeted, interest-filtered visual batch or derive from the remote’s local visual tracer only where honest enough. | Unreliable/newest-wins; no old event rendered after a short TTL. | A small hole in a sustained stream is acceptable; the next rounds cover it. |
| Owner hit/damage feedback | `DamageConfirmed { ShotId, result, authoritative_tick, target/effect, resulting durable state/version }`; the client dedups by `ShotId` and displays feedback even if it had a different prediction. | High-priority reliable semantic consequence, independent of MG visual traffic; durable health/ammo state also replicates as repair. | Retransmit/repair ensures confirmation. UI must not double-play on duplicate arrival. |
| Cannon shot begins | Stable `ShotId`, spawn tick, origin, initial velocity, and visual parameters. Owner predicts immediately; interested remotes initialize a view trajectory. | Reliable sparse spawn or state repair, plus an optional immediate best-effort cue. Late receipt creates/catches up only if still within TTL. | The optional cue may be absent, but the repair/spawn lets a remote see the shell if it is still relevant. |
| Cannon ricochet/bounce continuation | `ProjectileContinuation { ShotId, segment_index, server_tick, position, velocity, material/outcome }`. A new segment supersedes prior trajectory. | Sparse reliable, preferably independent from chat/other bulk ordered traffic; accept idempotently only if `segment_index` is newer and within visual relevance. | Re-send/repair preserves the next visible segment. If it arrives after expiry, skip the old sparkle and snap/lerp to the latest still-relevant state rather than replaying history. |
| Cannon terminal impact/damage | Separate visual impact cue from authoritative consequence. Include the relevant `ShotId` and result in the owner confirmation; send durable damaged-state snapshots to all relevant clients. | Consequence reliable; visual may be immediate/best-effort followed by reliable repair when it is important and still timely. | Damage and owner feedback converge; remotes may lose a particle, but must not see an old bounce/impact played seconds late. |

### Identity, deduplication, and expiration

Use a stable server-issued or server-confirmed `ShotId` as the correlation key for
all cannon segments and owner consequences; a `(shooter, weapon, fire-generation,
shot-sequence)` form can provide the same identity for MG bookkeeping. Store a
bounded recent-ID window, and independently track the newest generation/segment
received. These are separate tests:

1. **Duplicate?** Do not replay sound/VFX/UI or apply a consequence twice.
2. **Superseded?** Do not replace a later projectile segment with an earlier one.
3. **Expired/irrelevant?** Do not render a reliable retransmit as a fresh visual
   event after its presentation TTL or after it left client interest.

This is the practical meaning of “reliable repair”: arrival is valuable for state
and acknowledgement, but visual time is not reversible. It is consistent with
Steam’s explicit unreliable reordering, Lightyear’s sequenced update rejection,
and Reed’s redundant/out-of-order suppression.

### Interest and packet construction

Determine recipients before serializing detailed shooting visuals: at minimum by
distance/visibility/team/spectator relevance and at a second level by LOD. A distant
observer may need only a firing state/audio cadence; a near visible observer may
receive a denser visual batch; the owner receives immediate local presentation and
authoritative confirmation. Never assume a replication framework’s entity interest
also filters standalone events.

Batch per-recipient low-priority visual records once per network tick, cap both
records and bytes, and omit/degrade detail when the cap is reached. Keep packets
comfortably below the transport path’s practical datagram size; do not rely on
fragmentation for bursty MG cosmetics. SteamNetworkingSockets and older Steam P2P
docs explicitly note that large messages are split/reassembled, and the older API
calls 1200 bytes a typical MTU-sized unreliable packet limit. [Steamworks,
*ISteamNetworking*, live documentation, accessed
2026-07-13](https://partner.steamgames.com/doc/api/ISteamNetworking)

## Product concessions: what loss should look like

Designers can plausibly accept the following **only after playtest**:

- a remote observer misses one tracer, one MG impact spark, or a tiny slice of
  continuous gun audio, then the continuing cadence resumes cleanly;
- distant combat uses a lower tracer density or a coarser firing representation;
- an old retransmitted visual event is silently discarded rather than appearing
  late and confusing causality.

They should normally reject:

- the firing owner waits a round-trip before any muzzle/recoil/fire response;
- the owner sees a hit/damage result twice, never receives it, or receives it
  without a `ShotId`-correlated explanation;
- a remotely visible cannon silently stops at a bounce, then a later retransmit
  plays the old bounce after the shell is already elsewhere;
- dropped cosmetic data changes gameplay truth, or client cosmetic collision is
  treated as authoritative damage.

Those are a proposed experience contract, not a statement that all games share the
same taste. The actual acceptable MG density and cannon discontinuity must be
settled by the forks below.

## Anti-patterns indicated by the evidence

- **Reliable ordered event per MG round per recipient.** It multiplies fan-out,
  retransmit work, queue occupancy, and head-of-line risk for details that soon
  become obsolete. Unreal’s explicit warning is decisive here.
- **One global reliable lane for cosmetic fire, damage, UI, and control.** A loss
  in the noisy class delays semantically independent data. Use at least separate
  classes/lanes where supported.
- **Replicating a network entity for every short-lived bullet by habit.** The
  Insomniac account is a direct warning that short-lived projectiles can dominate
  traffic without gaining useful persistent state.
- **No repair state.** Purely unreliable fire start/stop can leave a remote in the
  wrong long-lived firing state after loss or join-in-progress.
- **No IDs or expiry policy.** Dedup alone does not prevent stale visual replay;
  ordering alone does not make an old effect visually valid.
- **Broadcast-before-interest.** A server that serializes every visual round to
  every connected player has already paid most of the cost, even if clients later
  discard it.
- **Treating the local predicted tracer as a hit claim.** The server’s result is
  the authoritative gameplay event; the view may be corrected or contradicted.

## Playtest and measurement forks

These choices are intentionally provisional. Each must log both network behavior
and what players actually perceive under injected latency/loss/reorder.

| Fork | Default to test first | Preserved alternative | What resolves it |
|---|---|---|---|
| Remote MG representation | Fire start/stop plus cadence/seed/counter; optional best-effort visual batches, with LOD. | Every visually important MG round has an individual best-effort event. | Can players identify direction, suppression, and cover impacts at realistic combat ranges without exact remote round count? |
| Tracer density | Near observers get a sampled/pooled tracer cadence; distant observers get lower density. | Full visual cadence within a strict interest radius. | Blind A/B clips and playtests: readability/feel versus bytes, packets, and render cost. |
| MG loss repair | Periodic authoritative firing generation/count state repairs a missed edge. | Repeat fire-start state at a small fixed cadence while held. | Time-to-correct a deliberately dropped start/stop, false-start/false-stop rate, and visual popping. |
| Cannon continuity | Reliable, idempotent `ShotId` + segment continuation plus TTL-aware late handling. | Replicate a short-lived projectile state at a higher update rate while visible. | Do remote observers notice a bounce discontinuity, and does either path stay within burst/network budgets? |
| Cannon impact presentation | Immediate best-effort visual cue with reliable semantic continuation/repair. | Make every visible cannon bounce/impact VFX reliable, separately prioritized. | Under loss, do users prefer an occasional missing spark or a delayed but exact event? |
| Owner confirmation | Reliable `ShotId`-correlated result plus durable health state repair. | Derive feedback only from replicated health/score counters. | Does the owner understand hit/armor/ricochet outcomes quickly and unambiguously under loss/reorder? |
| Interest policy | View/visibility and distance tiers before serialization. | Distance-only, then later visibility refinement. | Worst-teamfight CPU, outgoing bytes, and whether visibility omission creates unfair informational loss. |

### Minimum instrumentation for every fork

Record by client, weapon class, and recipient tier:

- **MEASURED:** outgoing/incoming bytes, packets, message records, and p50/p95/p99
  bytes per 64 Hz tick; packet payload sizes and any fragmentation;
- **MEASURED:** reliable queue depth, resend count/age, acknowledgement delay,
  lane blockage, dropped best-effort visual records, and budget culls;
- **MEASURED:** `ShotId` duplicate count, stale/superseded/TTL-discard count, and
  mismatch between predicted owner feedback and authoritative result;
- **MEASURED:** server simulation time for MG/cannon shots and recipient filtering;
- **MEASURED:** time from server cannon bounce to first correct remote continuation,
  and time from authoritative hit to owner confirmation;
- **MEASURED:** scenarios with 20 and 30 tanks, both MGs held, the stated 60-event
  same-tick burst, near/far/occluded recipients, and injected loss/reorder/jitter.

Do not declare a byte budget, acceptable loss rate, or visual TTL “solved” until
these are captured from Overmatch’s actual Lightyear 0.28 UDP path and reviewed
alongside player observation. Unity’s own sample reached its 30-tick choice by
comparing captures, not by assuming a theoretical optimum; the same discipline is
appropriate here.

## Source index

All sources are URL-linked above; this index states provenance and date so readers
can quickly distinguish production accounts, vendor guidance, and community signal.

1. Dan Reed, Blizzard Entertainment, *Networking Scripted Weapons and Abilities
   in Overwatch*, GDC 2017. Primary presentation by a senior gameplay engineer.
2. Peter Kao, Insomniac Games, *Always Online*, *Game Developer*, February 2012,
   pp. 19–25 in the archived issue. Primary studio engineering account.
3. Valve Developer Community, *Source Multiplayer Networking*, live documentation,
   accessed 2026-07-13. Engine documentation/community-maintained reference.
4. Epic Games, *Networking Overview* and *RPCs*, UE 4.27 documentation, accessed
   2026-07-13. Vendor documentation.
5. Unity Technologies, *Reliability*, Netcode for GameObjects 1.0, last updated
   2023-02-01; and *Optimizing Boss Room Performance*, accessed 2026-07-13. Vendor
   documentation and sample postmortem.
6. Godot Engine, *MultiplayerPeer* 4.3 and *High-level multiplayer* 4.2, accessed
   2026-07-13. Vendor documentation.
7. Valve, *ISteamNetworkingSockets* and *ISteamNetworking*, Steamworks docs,
   accessed 2026-07-13. Transport vendor documentation.
8. Lightyear, *Replication Logic* book and 0.28 crate documentation, accessed
   2026-07-13. Project documentation; relevant to current tooling but not a
   substitute for a versioned API audit.
9. Glenn Fiedler, *Reliability and Congestion Avoidance over UDP*, 2008-10-20; and
   *Client Server Connection*, 2016-09-28. Widely used practitioner explanations,
   not vendor specifications.
10. Epic Developer Community forum (2015), GameDev Stack Exchange (2018), and
    r/gamedev (2022), linked in the community table. Supporting practitioner signal
    only; no conclusion depends solely on them.
