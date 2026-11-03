**The knee is still unmeasured. The best place to look first is how long pieces stay active, how faithfully their contact shapes follow their outlines, and how finely loose groups keep splitting.** Those can move cost without immediately making the initial break look coarse.

Fracture count combines several different choices. A structure can have detailed broken edges, a coarse support calculation, fewer independently moving groups, simpler collision shapes, sparse pose updates and inexpensive visual splinters. Sweeping those together would hide the useful compromise.

First remove repeated work and incorrect waking. Then sweep the visible concessions separately. My starting preference is to preserve immediate reaction, the main collapse silhouette and lasting movable rubble; spend subtle settling motion and invisible contact detail first. That is a hypothesis for the owner’s comparison, not a measured optimum.

The supplied ram reached **MEASURED 32.0 ms server tick time with 72 awake pieces**, against **MEASURED about 2.8 ms before the ram** ([raw stats](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/owner-play-2026-11-03/ram/stats.txt:1)). But much of that was repeated construction work. The landed no-live-joint check reduced a matched Mac window from **MEASURED 8.392 to 5.404 ms**, with almost unchanged physics time. Neither result prices the remaining choices on the required two-core server ([report](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/report.md:1)).

This research made no edits, builds, tests or git changes.

**Reading the inventory**

Current values below are **MEASURED source transcriptions**, not new runtime measurements. Parentheses name their existing label: `Resolution`, `Taste`, `Derived`, `Unknown`, or **bare** where no such label exists. Inherited engine defaults are **DERIVED to apply** from the production plugin configuration and absence of overrides.

Every cost relationship below is **DERIVED**, with its inputs stated. It predicts which work changes, not a speedup.

Source abbreviations:

- **B** — [building breakage](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1).
- **F** — [construction laws](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:1).
- **BV** — [building view](/Users/Yan/github/vikng-dev/overmatch/src/buildings/view.rs:1).
- **T / TM / TV** — [trees](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:1), [tree motion](/Users/Yan/github/vikng-dev/overmatch/src/trees/motion.rs:1), [tree view](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:1).
- **V/** — `vendor/avian3d-0.7.0-child-pose/src/`. This is the dependency actually used by [Cargo.toml:145](/Users/Yan/github/vikng-dev/overmatch/Cargo.toml:145). Registry Avian’s relevant defaults match it.
- **Build** means implementation is needed. **API** means the engine exposes it, but Overmatch has no shipped destruction-quality knob. **Data** means an existing bake/data choice.

The useful accounting is:

- **Active cost:** assessments, active bodies, collision candidates, contact points and solver substeps.
- **Accumulated cost:** active body-seconds, retained colliders, carriers, meshes and history.
- **Peak cost:** simultaneous assessments, splits, collider creation, mesh uploads and sends.
- **Client cost:** presentation work, visible triangles, shaded pixels, shadow work and newly exposed scenery.

A sleeping body, an independently moving body, a collision island and an authored chunk are different accounting units.

**Compute and structural resolution**

| Lever: plain meaning | Where it lives today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Assessment scope:** examine only the loose group’s own construction | B:513 clones mutable state over the source construction; F:1066,1275,2100 traverse its graph. No-live-joint return at F:2110 is landed | Nothing if results remain equivalent | Traversal inputs shrink from source chunks/joints to owned chunks/joints. Repeated passes and allocations also matter. **Build; T27. An optimization prerequisite, not a quality concession** |
| **Structural graph detail:** combine fine chunks into fewer load-bearing nodes | F:307,1275; graph follows baked chunks/joints | Less exact failure location and load sharing; larger parts release together | Smaller graph reduces adjacency, connectivity and section work. **Build**; separate from visual mesh detail |
| **Section-reading resolution:** how accurately remaining support geometry is assessed | F:2887 uses bordering joint geometry; F:982 has a hard-coded **7-point** weighted disc for some opening calculations (**bare**). T14 proposes an independent solid-section reader | Small supports/openings can be misread; breaks move or occur too early/late | Sampling work follows assessed sections × samples × geometry/removal-query cost. **Build**; the planned sampling resolution is not implemented |
| **Assessment frequency:** reconsider loads less often | B:684,1762; active contact inputs reach laws each physics tick; B:1811 compares presses exactly | Delayed recognition of support/load changes | Assessment work per second follows assessment frequency × eligible constructions × work per assessment. **Build** |
| **Assessment sensitivity:** ignore sufficiently small load/pose changes | B:1807–1825; exact press comparison, moving-frame recomputation | Small pushes or accumulating changes react later | Saves work only when changes fall inside a justified error bound. Requires invalidation on topology, support loss and accumulated change. **Build** |
| **Cascade completion budget:** finish only part of a structural cascade now | F:1709,2100 loops through failures to completion | Collapse progresses in computational stages; unsupported parts may wait | Bounds per-tick graph work; may leave total work unchanged. **Build**, with explicit intermediate-state semantics |
| **Physical-model scope:** omit a class of deformation or load coupling | F:1709,2100; attachments in `src/buildings/attachments.rs:1`; trees use F through T:851 | Some failures or secondary reactions disappear | Savings follow omitted evaluations and resulting bodies. **Build/design choice**. Material strength is not a performance knob |
| **Precompute versus runtime work:** bake graphs, geometry and reusable results | `schema.rs:50`; B:126; `SectionCache` at F:1617 | Nothing for equivalent precomputation; less adaptability if results are baked too specifically | Trades load/storage cost for avoided runtime reconstruction. **Partly exists; Build for remaining caches** |

T27’s exact reuse should precede experiments that deliberately assess less accurately. Otherwise the experiment confuses an expensive implementation with an expensive physical model.

**Fracture and independently moving pieces**

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Masonry cut spacing** | [house specification:77](/Users/Yan/github/vikng-dev/overmatch/tools/buildings/house_1.house.ron:77): rows every **3 courses**, course **0.075 m**; horizontal module **0.26 m** (**bare authored numbers**) | Larger steps in broken edges and larger falling pieces | For a thin wall, chunk count approximately follows wall area ÷ cut-cell area; joints and contact proxies follow resulting topology. **Data**, but horizontal fracture spacing is currently tied to construction module |
| **Timber cut length** | Same file:87: boards **0.9–1.6 m**, plates **1.2–2.0 m** (**bare**) | Longer intact sticks, fewer short splinters | Longitudinal piece count approximately length ÷ chosen piece length, subject to end handling. **Data** |
| **Tree longitudinal resolution** | [plan.py:18](/Users/Yan/github/vikng-dev/overmatch/scripts/prop/plan.py:18): **1.5–2.0 m**, target **1.75 m** (**bare**); current tree asset contains **15 segments**, source census | Fewer possible break heights; longer trunk sections | Segment/contact/graph population approximately trunk length ÷ segment length. **Generator edit today**; common cutter is planned |
| **Cut pattern and irregularity** | House spec:82: crack chance **0.2**, shift **0.09 m**, minimum width **0.08 m** (**bare**) | Straighter, more repetitive fracture language | Pattern changes hull features, slivers and adjacency—not just count. Cost is non-monotonic. **Data** |
| **Independent-body resolution:** retain several visible chunks as one body | B:2050 already makes one body per connected loose group | Parts move as larger slabs; less internal movement | Reduces bodies and pose streams. Contacts remain expensive if every member keeps a collider. **Existing grouping; Build for a controlled coarser hierarchy** |
| **Further splitting after release** | Moving groups retain `Breakage`, B:2079; splits at B:2139 | Falling slabs remain whole through later contacts | Avoids subsequent law work, body creation and growing contact population. **Build**; must be a generic resolution rule |
| **Progressive release depth:** expose finer breakup only where needed | No independent hierarchy control in current runtime | Fine breakup appears later or never | Reduces the simultaneous independent population; eventual cost depends on permitted depth. **Build** |
| **Physical versus decorative fine debris** | Current wall/tree carriers represent authoritative physical groups; B:2050, TM:133 | Fine fragments cease stacking, pushing or remaining interactable | Server savings follow physical bodies/colliders replaced; client effects add their own cost. **Build and explicit presentation-policy choice** |
| **Baked versus computed fracture/fall** | Cuts are baked; release motion is computed, B:1985,2050 | Cached falls repeat and respond poorly to unexpected interference | Replaces runtime fracture/solving with stored geometry/motion playback. **Build**; much larger loss of systemic behaviour |

Changing cut resolution currently changes both the law graph and the physical pieces. That coupling makes it a poor first sweep unless the lane reports both effects.

**Collision and solver resolution**

Avian’s construction “joints” are not the building’s graph bonds. No Avian `FixedJoint`/`RevoluteJoint` construction was found in these building/tree paths. A generic engine-joint iteration sweep would miss today’s structural cost.

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Contact-shape detail** | B:1946 builds convex hulls; T:685 uses a **12-sided** ring (**bare**) | Blockier rolling, catches, gaps or apparent floating | Narrow-phase cost depends on candidate pairs and hull features, independently of render triangles. **API; Build for certified proxy variants** |
| **Colliders per moving group** | B:2097 and TM:175 retain member contact hulls | A combined hull can bridge real gaps and fill concavities | Fewer proxies and pair queries, potentially offset by larger bounds and false positives. **Build; T26 overlaps this** |
| **When intact proxies are replaced** | B:1844 derives swaps from first affected chunks; BV:180 follows swapped elements | Delaying replacement can retain a visibly inaccurate contact outline | Avoids fine-proxy creation and maintenance until necessary. **Build; T26 plans departure-based replacement** |
| **Which pieces collide** | B:2188: debris meets terrain, vehicles, debris and track sides | Removing mutual contacts loses stacking and piles; removing vehicle contact changes driving | Saves eligible candidate pairs, manifolds, constraints and island links. Actual pair count—not a linear body coefficient—sets benefit. **API; Build for generic size tiers** |
| **Contact inset/skin** | B:1915: inset **0.001 m**, minimum scale **0.2** (`Resolution`); absent Avian margin defaults to **0** (**bare**) | More inset allows visible overlap; more skin can make pieces float | Changes contacts and stability. **No monotonic saving is established. API/code constants** |
| **Speculative contact horizon** | V/collision/narrow_phase/mod.rs:249: margin cap `Scalar::MAX`; effective horizon uses relative speed × timestep | Less anticipation can miss fast contact with thin geometry | Candidate/manifold work follows the enlarged contact horizon. **API** |
| **Contact tolerance** | Same block: **0.005 m** (**bare**) | Smaller tolerance can destabilize resting contact; larger values can separate visible surfaces | Affects admitted near pairs and contact persistence; either direction can cost more. **API** |
| **Contact points/manifolds retained** | V/collision/narrow_phase/system_param.rs:240,695; no exposed point-budget knob found | Flats tip or rock; stacking/friction become less faithful | Solver work follows retained contact points × substeps. Generation may remain expensive. **Engine/backend work** |
| **Solver substeps** | V/dynamics/solver/schedule.rs:185: **6** (**bare**), inherited by `src/net/physics.rs:31` | Softer contacts, penetration, jitter or unstable thin pieces | Repeated integration and contact-solving work roughly follows substeps. **Broad/narrow phase still run once per outer tick. API** |
| **Warm-start matching** | V/collision/narrow_phase/mod.rs:254: matching enabled; solver/plugin.rs:297: coefficient **1.0** (**bare**) | Worse convergence and resting stability if reduced | Saves matching/application work but may require more substeps or lengthen settling. **API; low-priority sweep** |
| **Correction softness and speed** | V/dynamics/solver/plugin.rs:294: damping ratio **10**, frequency factor **1.5**, overlap correction cap **4 m/s** (**bare**) | More compliant contacts or slower separation; possible springiness | Same solver passes still execute; savings are indirect through stability and active lifetime. **API**, supporting controls rather than direct work budgets |
| **Bounce treatment** | Same file:298: restitution threshold **1 m/s**, iterations **1** (**bare**) | Smaller bounces disappear | Work follows eligible bouncing contacts × restitution passes. Little downward iteration range remains. **API** |
| **Swept collision detection** | V/dynamics/ccd/mod.rs:389; opt-in, absent from production environment paths | Omitting it can miss fast/rotating contacts | Work follows swept bodies and candidate geometry. **API; no present saving because it is already absent** |
| **Contact-query persistence** | V/collision/narrow_phase/system_param.rs:695 clones old manifolds; persistent Parry queries are not reused | Nothing if equivalently cached | Avoids repeated query setup/manifold reconstruction. **Engine work; optimization prerequisite** |

**Rate, reaction and peak work**

“Responsiveness” contains several delays. They should have separate measurements.

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Whole authoritative tick rate** | [server.rs:46](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:46): **15.625 ms** (**bare**), hence **DERIVED 64 Hz** | Later contacts and structural responses; greater travel between collision checks | Work per second approximately frequency × work per tick; work per tick itself changes. **Coordinated code change**, affecting the whole game |
| **Separate debris update rate** | Avian has one global physics step | Slower rubble response when vehicles contact it | Could reduce debris evaluations per second, but cross-rate contact exchange and peak scheduling need design. **Build**, not a resource setting |
| **Failure recognition delay** | B:1762 runs laws on current queued inputs | Structure waits before beginning to fail | Reduces assessment rate or distributes it over time. **Build** |
| **Split/body-creation budget** | B:1427,2050,2139 creates resulting groups without a destruction admission quota | A break releases in stages | Bounds creation spikes; does not cap sustained awake population. **Build** |
| **Collider/mesh preparation budget** | B:1900,2050; BV:488,592; work completes synchronously in these paths | Collision or appearance arrives later | Redistributes preparation/upload work; caching can also reduce total work. **Build**, preserving one coherent authoritative transition |
| **Server packet-polling cadence** | Server:52: **4 ms** frame (**bare**) | More input/network scheduling delay and potentially wider presentation buffer | Frame maintenance follows polling frequency; fixed physics work does not disappear. **Code change** |
| **Adaptive importance/budgets** | No destruction importance controller; carriers target all clients | Peripheral or old destruction gets less detail/reactivity | Caps selected work according to distance, projected size, age or interaction importance. **Build**; authoritative decisions must account for all players |

Asynchronous execution, work queues and additional threads can reduce stalls without intentionally losing appearance. They do not inherently reduce total work, and their benefit is bounded by the required two-core allocation.

**Lifetime and liveliness**

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Linear sleep threshold** | V/dynamics/rigid_body/sleeping.rs:103: **0.15 m/s** (**bare**) | Subtle sliding stops sooner | Saves active body-time once every island member qualifies. **API** |
| **Angular sleep threshold** | Same location: **0.15 rad/s** (**bare**) | Subtle rocking/rolling stops sooner | Same island condition; independently controls rotational tail. **API** |
| **Quiet time before sleep** | Same file:149: **0.5 s** (**bare**) | Motion stops after a shorter quiet interval | Avoided active body-time follows qualifying bodies × shortened wait, unless another member blocks sleep. **API** |
| **Wake propagation/island size** | V/collision/narrow_phase/system_param.rs:247; solver/islands/sleeping.rs:217 | Suppressed legitimate wakes make piles less responsive | One moving member can keep its connected island active. **Existing island mechanism; altered policy needs engine work** |
| **False-wake prevention** | `src/debris.rs:94–179`; T4 investigates the engine contact-wake boundary | Nothing should be lost | Avoids unnecessary island wake/reset work. **Correctness work**, not permission to suppress impacts/support loss |
| **Extra damping** | V/dynamics/rigid_body/mod.rs:583: default linear/angular damping **0** (**bare**) | Pieces lose slide/spin; heavy values make falls look dragged | Usually reduces active lifetime rather than per-step arithmetic. **API**, but an explicit physical/game-model choice |
| **Speed/spin caps** | Same file:443,473: infinity (**bare**) | Fast pieces travel or spin less | Can limit speculative horizon, travel and contact churn; benefit is scene-dependent. **API** |
| **Physical lifetime** | [debris.rs:8–42](/Users/Yan/github/vikng-dev/overmatch/src/debris.rs:8): dynamic pieces remain all match | Removal erases rubble; disabling prevents later interaction | Retained population follows creation rate × residence time; savings depend on which colliders/meshes survive. **Build** |
| **Static/merged settled rubble** | Not current: sleep retains finite-mass dynamic bodies | Rubble becomes immovable or reacts only after thawing; contact can feel wrong | Can reduce retained-body work but incurs migration/thaw cost. **Build; substantial behaviour change** |
| **Historical carrier lifetime** | B:2121; `net/debris_stress.rs:268`; BV:658 retains split ancestry for cursor handover | Safe reclamation loses nothing; premature reclamation causes missing/duplicate pieces or broken resync | Memory and presentation scans follow retained carrier/chunk references. **Build**, tied to interpolation/recovery guarantees |

**Sleep has a floor.** Avian moves touching pairs into its sleeping contact set; bounds-overlapping, non-touching pairs can remain active and be queried again. Faster sleeping therefore does not guarantee that all pile-related narrow-phase cost vanishes: V/collision/contact_types/contact_graph.rs:796 and narrow_phase/system_param.rs:480.

**Transport to clients**

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Pose precision** | [DebrisPose:125](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:125): `Vec3 + Quat`, default serialization; **DERIVED 28 bytes of floating-point fields**, before framing | Quantized position/rotation can shimmer, especially at long piece tips | Payload follows bits per pose × changed poses × recipients. **Build** |
| **Pose update frequency** | Same file:466 publishes changed poses each fixed tick; protocol:1153 replicates them | Fast arcs/contact changes interpolate less faithfully | Payload and some send/receive work follow updates per second. Physics cost stays. **Build** |
| **Change/error threshold** | `publish_poses`:472 uses exact `set_if_neq` | Small motion is held, then visibly corrected if threshold is too coarse | Suppresses small changes; saving follows suppressed updates. **Build**, with accumulated error bound |
| **Delta/batch encoding** | Wall ledger already uses diffs, protocol:1159; debris poses are ordinary replicated components | Lossless deltas need lose nothing; stronger lossy deltas trade precision | Bytes depend on changed fields, ID encoding and baseline availability. **Partly exists; Build for debris codec** |
| **Per-client relevance and priority** | Carrier spawns at debris_stress:257,315 target `All` | Distant/off-screen motion updates later; visibility changes can reveal catch-up | Send/receive work follows recipients and update allocation. **Build** |
| **Continuing tracking versus release-only motion** | Physical pieces retain authoritative pose streams | Untracked pieces diverge after contacts unless kept purely decorative | Removes ongoing pose traffic but requires client approximation/computation. **Build; departure from current physical presentation contract** |
| **History/checkpoint granularity** | [walls.rs:29](/Users/Yan/github/vikng-dev/overmatch/src/net/walls.rs:29): append-only ledger; diff history **64 appends** (**bare inherited default**), older/late clients receive a snapshot | Exact checkpoints need lose nothing; discarded history can limit recovery | Join bytes and replay work follow retained events versus checkpoint state. **Build** |
| **Interpolation delay and reconstruction** | `net/interp_delay.rs:16`; debris uses position lerp/rotation slerp at debris_stress:138 and the shared cursor | More buffering delays everything; less buffering freezes/snaps; sparse samples round off abrupt motion | Buffering alone saves no simulation work. It enables lower transport cadence; reconstruction costs client CPU. **Existing shared law; coordinated change** |

The packed codecs at `net/wire_quant.rs:35,126` do **not** currently compress `DebrisPose`. They are registered separately for `Position` and `Rotation` at protocol:1251.

A reduced debris cadence must still present bodies, structure changes and vehicles at the same shared time. Giving rubble an independent “smooth but late” clock would conceal temporal disagreement.

**Client presentation and other costs**

| Lever | Where today; current setting | What coarser loses visibly | DERIVED cost relationship; availability |
|---|---|---|---|
| **Mesh detail and internal faces** | BV:180,198 emits member hull faces, including faces internal to a group | Simplification changes broken silhouette; removing genuinely internal faces loses nothing | CPU/upload/vertex work follows emitted geometry. **Build** |
| **Render grouping/instancing** | BV:198 creates group meshes; TV:322 merges standing meshes per material, broken trees use segmented hierarchy | Grouping loses nothing if equivalent; coarse impostors lose depth/detail | Draw/entity cost follows instances, materials and independently transformed parts. **Partly exists; Build for broken-state batching** |
| **View distance/projected-size threshold** | TV:393: abrupt **600 m** cutoff, comment labelled `DERIVED`; no equivalent destruction-specific building control found | Distant rubble/trees disappear or simplify | Savings follow culled draw work and screen coverage. **Tree code constant; Build for projected-size policy** |
| **Interior and fracture-edge detail** | BV:147 draws flat hull faces; generator:43 uses **12 sides** (**bare placeholder resolution**); irregular cuts above | Plainer, sharper or more faceted broken edges | Geometry displacement/tessellation adds vertices; normal/material detail mainly adds shading. **Generator/shader work** |
| **Crack display** | BV:279 half-width **0.006 m** (`Resolution`); standoff **0.0015 m** (**bare**) | Thin/fewer cracks become unreadable at distance | Work follows visible crack geometry and shaded pixels. Width alone is not a reliable cost control. **Constants; Build for display LOD** |
| **Surface-detail retention** | B:122 cap **16 per chunk** (`Resolution`); `building_holes.wgsl:114–165` contains detail/union/plane loops | Older/smaller surface details disappear or simplify | Pixel work depends on detail count; nested union walking has a quadratic worst-case term in that count. **Existing cap; Build for view-dependent projection** |
| **Shadow detail/range** | BV’s pieces inherit ordinary world rendering; `render_policy.rs:195` defaults to cast-and-receive | Less grounding and fine shadow motion | Shadow work follows casters, geometry, shadow views and covered pixels. **Policy APIs exist; destruction policy needs building** |
| **Mesh rebuild cadence** | BV:488 redraws changed shown state; BV:525 rebuilds the building mesh | Deferred rebuilds make visible breakage arrive later | Rebuild/upload cost follows changed buildings and rebuilt geometry. **Build**; incremental rebuild can save work without delay |
| **Presentation maintenance cadence** | BV:664 indexes carrier ancestry every frame; TV:548 follows segmented state | Less frequent updates step or delay membership/foliage motion | CPU follows retained groups/chunk references × presentation frequency. Exact caching could avoid the concession. **Build** |
| **Dust, splinters and audio activity** | No dedicated collapse-effect budget was found in the inspected building/tree paths | Less airborne detail and fewer contact cues | Effect cost follows emitters, lifetime, particles, overdraw and voices. Can replace some fine physical detail, but adding effects alone saves no server work. **Build** |
| **Newly exposed scenery** | Destruction removes occluding surfaces; current broken meshes expose what lies behind | Coarser interior/distant scenery reduces richness | Rendering cost can rise because more world becomes visible, even when rubble count falls. **Scene/rendering work**, not a fracture knob |
| **Destructible density and concurrency** | Map placements and construction topology; no destruction admission controller | Fewer nearby destructible structures or weaker chain reactions | Sets simultaneous active groups, contacts and creation bursts. **Content/workload choice**, changing the experience itself |
| **Spatial acceleration, parallelism and SIMD** | V/collider_tree/optimization.rs:66; server_host.rs:56; vendor Cargo features | Intended appearance unchanged | Benefit depends on moving fraction, spatial coherence, serial work and scheduling overhead. **Implementation controls**, not quality concessions |

Sharp-looking edges do not require equally detailed contact shapes. Conversely, reducing a visual crack’s width may barely change cost. The useful “sharpness” knobs are geometric complexity, surface-detail work and their screen-space importance.

**What published systems actually separate**

| Primary-source finding | What it adds to our menu |
|---|---|
| **Chaos:** clusters combine members into a rigid body; breaking depth, simulated depth and replication depth are separate controls. It also supports cached destruction. [Overview](https://dev.epicgames.com/documentation/unreal-engine/destruction-overview?lang=en-US), [component API](https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollectionComponent) | We have connected loose groups, but lack an independently controlled physical hierarchy and network detail hierarchy |
| **Blast:** structural graph reduction and per-frame stress-iteration budget are separate. Its documented reduction is experimental. [Stress solver](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html) | Graph accuracy and reaction delay deserve separate rows; neither requires changing the visible fracture mesh |
| **APEX:** budgets can protect essential hierarchy levels and prioritize by age/screen area; active-island limits and creation/fracture quotas are distinct. [Destruction documentation](https://nvidiagameworks.github.io/APEX/1.4/docs/APEX_Destruction/Destruction_Module.html), [module source](https://github.com/NVIDIAGameWorks/PhysX-3.4/blob/master/APEX_1.4/module/destructible/include/ModuleDestructibleImpl.h) | We lack benefit-based admission, protected detail levels and separate peak/population budgets |
| **Rainbow Six Siege:** documented choices include simpler collision outlines, replacement box-collision debris that does not mutually collide, recycling, ordered structural events, and asynchronous/time-sliced work. Small dynamic debris is not replicated. [Ubisoft GDC slides](https://media.gdcvault.com/gdc2016/Presentations/LHeureux_Julien_Art_Of_Destruction.pdf) | Collision fidelity, cosmetic debris, reaction delay and transport are independent axes. This supersedes hosting-research’s “Siege replication not found” |
| **Teardown:** reliable ordered structural commands coexist with motion-state synchronization, per-client visible-object priority and client simulation. Lower correction frequency can snap; history replay constrains joining. [Developer account](https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html) | We already use structural records, but lack motion relevance budgets/checkpointing. Its client simulation is not a drop-in fit for our authority contract |
| **The Finals:** physically simulated debris changes the playspace. [Developer GDC abstract](https://www.gdcvault.com/play/1034307/Engineering-Mayhem-Technical-Deep-Dive) | Keeping meaningful physical rubble is a valid design direction; cosmetic-only debris is not an industry requirement |
| **Red Faction: Guerrilla:** a developer describes periodic structural scans and distinguishes Geo-Mod’s structural work from Havok’s rigid-body motion. [Developer interview](https://www.gamewatcher.com/interviews/red-faction-guerrilla-interview/11408) | Structural assessment cadence is a shipped tradeoff. It does not establish an acceptable delay for Overmatch |
| **Frostbite/Battlefield:** EA describes persistent, deliberately shaped rubble, destruction-network prioritization and material-driven effects. [Developer article](https://www.ea.com/en-ca/amp/news/how-battlefield-6-redefined-destruction) | Ruin appearance and activity cues can be budgeted separately from freely simulated pieces |
| **Havok:** public material describes faster break processing, parallel collection updates and reduced cache memory. It does not expose enough implementation detail to reconstruct a Havok Destruction quality curve. [Havok post](https://www.havok.com/blog/how-havok-evolved-unreal-5-5/) | Evidence for implementation optimization; no justified Overmatch target or inferred hidden knob |

**No positive example found** in the consulted primary sources for:

- Per-viewer authoritative physical existence: different players having different actual rubble.
- Changing material strength merely to compensate for coarse visual fracture.
- Arbitrary forced-sleep regions replacing legitimate contact-island sleep and support-loss waking.

That is a bounded search result, not proof that no engine or shipped game has tried them. Forced freezing, static conversion and cached falls remain technical possibilities; their visible and mechanical costs belong in the inventory.

**The few I would prioritize**

This ranking is an **estimate of cost saved per perceptual loss**, conditional on the current evidence.

| Rank | Lever | Reason |
|---|---|---|
| Prerequisite | Exact graph reuse, contact-query reuse and correct waking | They can move substantial work without purchasing a visible concession. Keep them outside the quality curve |
| First | **Automatic rest: rotational/linear thresholds and quiet time** | The evidence shows a long active tail. Subtle motion may be cheap to surrender, while impacts still wake rubble. Correct lifecycle comes first |
| Next | **Contact proxies and colliders per group** | Players judge the outline mostly through rendering. Many contact details may be invisible, but gap-bridging and rolling errors need close inspection |
| Next | **Independent-body resolution and continued splitting** | Moves bodies, contacts, law work and transport together while allowing detailed visible surfaces. Larger slabs are the main visible cost |
| Next | **Solver substeps** | A cheap, clean experiment. Likely smaller total benefit because narrow phase and construction work remain |
| Conditional on wire cost | **Pose precision, frequency and relevance** | Potentially large transport savings with modest distant-motion loss; will not solve server physics overload |
| Conditional on client cost | **Projected-size mesh/surface/shadow detail** | Can preserve nearby destruction while reducing distant presentation work. Does not reduce authoritative simulation |

I would sweep **sleep first**, after the exact-work/wake baseline is valid. If peak ticks remain the problem while the tail becomes cheap, move directly to proxies and independent bodies. Admission delays come later: they sacrifice the immediate response the owner is likely to notice.

**Sweep contract**

Every test and benchmark must run under an actual **two-core allocation**. The local mechanism is missing.

Build a launcher that:

- Constrains the server’s CPU eligibility and quota to **two cores**.
- Lets `available_parallelism()` report that allocation.
- Refuses a run whose startup affinity/quota/pool report disagrees.
- Records CPU throttling and keeps capture/load clients outside the server allocation.

Simply setting Bevy’s compute pool to two threads is insufficient: main, async and IO work could still execute elsewhere. Current sizing follows available cores, with separate async/IO pools at `server_host.rs:56–75`; startup already reports the host allocation at `:193–207`.

Use the same production composition, optimized build, map, input replay and observation windows for each arm. Include isolated native breakup, dense piles, a vehicle remaining in contact, simultaneous collapses and a long accumulated-rubble scene. Independent fragment piles diagnose physics; they cannot price native construction work.

All candidate settings below are **estimated experimental settings**, not targets. Baselines are source transcriptions. Change one axis at a time.

| Sweep | Settings | Additional measurements | Side-by-side clips |
|---|---|---|---|
| **Sleep velocity thresholds** | Baseline; double angular only; double linear only; double both | Active body-seconds, time to island sleep, island sizes, wake causes/timer resets, residual near-pair work | Low camera beside the settling pile; small pieces rocking; later vehicle push; support removal |
| **Quiet time** | Baseline **0.5 s**; **0.25 s**; **0.1 s** | Same metrics; distinguish shorter wait from a changed resulting pile | Uncut settling tail, followed by a fresh interaction |
| **Contact proxy detail** | Current hulls; reduced-feature convex hulls; box/capsule proxies | Proxies, candidate pairs, manifolds, points, narrow-phase time, penetration and contact gaps | Thin panel landing; trunk rolling; irregular pile; slow vehicle traversal |
| **Colliders per group** | Current member hulls; exact-compatible merges only; modest excess-volume allowance; larger allowance | Same metrics plus proxy excess volume, false-positive pairs and hull-build spikes | Concave groups straddling gaps, toothed edges, vehicle touching the pile |
| **Physical breakup resolution** | Current grouping; double linear physical-cell scale; quadruple scale; coarse groups with decorative fine detail | Active bodies/colliders, further splits, group-size distribution, law work, active body-seconds and wire cost | Initial breach, full fall, secondary impact, final ruin and later push |
| **Substeps** | Baseline **6**; **4**; **3**; **2** | Separate broad phase, narrow phase and repeated solve time; penetration, jitter and settling | Same collapse; thin/long pieces landing; loaded stack; vehicle pushing rubble |
| **Pose cadence** | Every tick; every other tick; every fourth tick | Bytes per client, send/receive CPU, packet age, buffering, freezes and interpolation error | Fast rotating pieces and contact reversals; near view and optic view; shared vehicle/rubble timing |
| **Pose precision** | Full fields; existing packed-codec precision adapted to debris; position step enlarged by **4×**; by **16×** | Payload and CPU; spatial error at piece tips, not only at centres | Long rotating trunk/plank near static edges; close and distant cameras |
| **Client detail threshold** | Current display; simplify below **1**, **2**, or **4 projected pixels** | Client CPU/GPU frame time, draw calls, triangles, shadow work, surface-shader time and uploads | Nearby rubble beside distant collapse; optic transitions; views through opened walls |

For the physical-cell sweep, scaling means a generic resolution rule, not increasing the building’s actual brick size or changing its strength. The present bake does not cleanly expose that separation; it must be built first.

For every arm, measure server tick tails and overruns, physics stages, construction assessments/allocations, creation spikes, memory, retained entities and achieved tick rate. Record the whole run, including settling; a cheaper final frame can conceal a worse collapse.

Existing stats cover tick/physics/send timing, CPU, throttling and awake/asleep counts. **Per-client bytes and several stage/contact counters need instrumentation**: `net/server_stats.rs:36` explicitly says bytes are not carried. Send time is outside the fixed tick, so report both.

Clips should share input and camera timing, retain sound, and show both the initial event and the quiet tail. Include an observing client, not only the driver. Present unlabeled comparisons first; reveal settings afterward.

If measured spikes remain dominant, a separate **reaction-budget sweep** should compare immediate completion against completion spread over **2, 4 or 8 ticks**—estimated settings. Measure trigger-to-failure, failure-to-release and release-to-visible-change separately. It is not a substitute for reducing sustained cost.

**What remains unknown**

| Unknown | What settles it |
|---|---|
| **The knee itself** | Owner comparison of the isolated sweeps, plotted against measured two-core peak and accumulated cost |
| **Residual cost after T27/T4** | Production replay after equivalent graph reuse and validated automatic sleeping; current pre-fix host coefficients cannot answer it |
| **Whether sleep truncation is noticeable** | Close settling clips plus later impact/support-loss interaction; the Mac threshold experiments alone do not establish acceptability |
| **Sleeping rubble’s true floor** | Separate retained-body, collider-tree and active non-touching-pair measurements. Existing prose gives conflicting coefficients; no clean city marginal cost was identified |
| **Whole-collapse independent-body demand** | Count native bodies, member colliders, secondary splits and active body-seconds through complete building/tree collapses |
| **How coarse contact can become** | Proxy sweeps with quantified geometric error and the gap/rolling/traversal clips |
| **Whether coarser cuts alter strength** | Independent section/cut convergence checks. Today’s graph/cutter coupling prevents assuming appearance-only change |
| **Real transport scaling** | Several actual receiving clients, measured bytes/send CPU, controlled loss/jitter and late joining |
| **Broken-state client bottleneck** | CPU/GPU captures separating mesh preparation, ancestry maintenance, shading, shadows and newly exposed scenery |
| **Value of delayed admission** | Creation/split timing first. If ongoing contact work dominates, spreading creation merely postpones the overload |
| **The final unified system’s cost** | Repeat the measurements after its physical and representation changes. The plan describes future code, not current behaviour |

Where prose and code disagree, the inventory follows code: pieces currently remain dynamic all match; they are not governed by an old debris cap. Tree slices and per-member hulls are still present. Solid-section sampling, controlled group proxies and the common tree runtime are planned. Debris poses are still uncompressed floating-point fields. The historical one-core city estimates remain conditional derivations; they do not establish feasibility—or the visual knee—on the required two-core server.