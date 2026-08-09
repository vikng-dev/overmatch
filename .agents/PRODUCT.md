# Overmatch product target

The single top-level authority on what this game is, where it stands, and what it will not do. Accepted implementation decisions live in [`docs/adr/`](docs/adr/); feel decisions that remain open by design live in [`scratch/playtest-forks/`](scratch/playtest-forks/); research and implementation logs are evidence rather than authority. History is git.

## Now

Milestone: **core loop feature-complete** — map detail, a destructible and affectible world, sound, sky — then a Steam early-access build people can play, then the meta loop (Garage, Progression, more tanks).

In flight: the rebuilt Tiger asset + 5-level LOD ladder + watertight ray kernel (one PR, in adversarial review); pipeline generalization per ADR-0034; level design (external lane, below). Landed: the union-field walk with its 10⁶-ray fuzzer gate, geometry/terrain LOD, display settings.

Next: land the asset PR; coincident-shell refinement (unconditional fuzzer zero); Houdini map-pipeline direction session; building-destruction implementation research.

## Values

- **Vehicle inclusion.** A vehicle ships only if a full-size metal prototype moved under its own power with its intended weapon physically mounted.
- **Realism vs fun.** Replicate real conditions as closely as we can, but never at the cost of frustrating the player in a way that adds nothing to play.
- **No pay-to-win.** Payment buys cosmetics and progression speed, never gameplay advantage. A free player can always reach the same battle effectiveness as a paying one.

## Core product

Overmatch is an official-server-hosted online PvP tank game. Players connect as clients to a dedicated authoritative server and play a finite **Battle** to completion. The **Game mode** supplies team assignment, admission, spawn and respawn rules, eligible content, and the victory condition.

After a Battle produces a winner, the player returns to the **Garage**. The Garage owns the meta loop: improving tank configurations, developing crews, unlocking tanks, and advancing through the tech tree. The Battle produces an authoritative result; durable **Progression** is not owned by the Battle simulation or the client.

## Player-facing runtime modes

- **Online Battle:** the dedicated server owns gameplay truth; clients submit intent, predict their own actions for responsiveness, and reconcile to authority.
- **Shooting range:** the normal client connects to the same authority runtime launched locally. It may admit locked tanks and inactive or scripted targets, but it uses standard Battle rules and produces no Progression.
- **Armor inspection:** a future analytical adapter over the same tank and ballistic rules. It may drive simulation directly and expose privileged diagnostic truth; it is not an alternate gameplay implementation.
- **Replay:** a future adapter over authoritative Battle history. Input-centric replay is the target once replay determinism is proven.
- **Spectating:** a future no-input client role, initially intended for following teammates. Live disclosure remains server-controlled and is distinct from post-Battle replay access.

Player-hosted online Battles are deferred. The foundation does not need listen-server mode, host migration, or NAT traversal.

## Authority, prediction, and feedback

The client predicts causes; the server confirms consequences.

- Controls, weapon response, recoil, audio, muzzle effects, and the beginning of a local shot happen immediately.
- Penetration, ricochet, damage, crew and tank-module effects, knockout, and Progression consequences are authoritative.
- **Damage confirmation** comes only from authoritative damage. The current hit-marker-like presentation is disposable; the amount of information disclosed is a playtest fork.
- The server filters live combat disclosure before transmission. A client must not receive privileged internal damage truth merely because the current UI hides it.

## Battle scale and world

- **DERIVED product target:** a normal Battle contains roughly 20–30 active tanks.
- **DERIVED product target:** a map covers roughly 1–4 km².
- One authoritative world owns a complete Battle; distributed gameplay authority and world sharding are not target architecture.
- There is no abstract spotting mechanic. If physical geometry, foliage, weather, and rendering allow a player to see a tank, distance alone does not hide it.
- Terrain topology is simulation-static **today**: track marks, impact scars, scorch, dust, and shallow craters are **Surface evidence** with no gameplay effect. Whether ground deformation stays cosmetic scarring or becomes authoritative heightmap change is an open fork, settled by dedicated research at the world-detail milestone — not by whichever is easier when a crater is first drawn.
- **Battlefield destruction is a core feature, not late polish.** Buildings must be destructible before early access; it changes collision, cover, line of sight, and traversal authoritatively, and implementation research is still pending. Fallen trees remain the first piece built.
- **Level design is an external parallel lane:** a colleague authors maps through a Houdini export pipeline. We owe that lane its interface direction — what the pipeline must emit, and what the runtime guarantees about it.

Replication frequency, rendering LOD, occlusion, and interest management are optimizations. They must not silently become spotting or concealment rules.

## Determinism, replay, and supported targets

The simulation is intended to reach forward and replay determinism across macOS ARM, Linux, and Windows x86. This is a staged engineering target, not a current claim.

Input-centric replay requires an initial authoritative state plus every non-derived decision: accepted player and bot commands, admission and departure, spawns, team assignment, game-mode decisions, random seeds, and external administrative events. Content, configuration, protocol, and executable identity remain part of the replay contract. Checkpoints remain useful for seeking and validation even when inputs are sufficient for correctness.

Determinism is proven by canonical state digests: fresh runs agree, rollback replay rejoins the original path, client and server agree when given identical state and inputs, and supported platforms eventually agree. Server authority remains the correctness anchor while those proofs are incomplete.

## Explicitly deferred

- Join-in-progress policy.
- Player-hosted and community-hosted online Battles.
- PvE Battles and production AI; current scripted actors remain test/scenario tools.
- Player-created gameplay mods and custom content.
- The exact combat damage-feedback presentation and disclosure level.
- Detailed X-ray armor inspection.
- Garage economy details such as persistent damage, repair cost, and ammunition economy.

Deferred capabilities must not acquire speculative frameworks. Preserve the shared game rules and real seams they would use; add their implementations when the product reaches them.
