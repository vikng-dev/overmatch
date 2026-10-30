# Destructible buildings and walls in shipped games: web survey, second pass

Subject: the game's destruction model, as game state and engine architecture. When destruction
touches a unit or crew, this file says only "HP pool", "knocked out" or "damage to units' HP".
Read 2026-10-30. Tags: [S] read in the cited source; [S~] via a search summary or a secondary
that quotes a primary; [I] my inference; [?] not found. Numbers are REPORTED or DERIVED.

## Summary (10 lines)

1. **Teardown (primary blog, now read):** a hybrid. The world's structure changes only through a reliable, ordered stream of deterministic commands (destruction logic rewritten in fixed point). Transforms use unreliable per-client priority state sync (~1 Mbit/s per client, REPORTED). Late join replays the command log, capped by a buffer. Bulk voxel deltas "choked the connection" [S].
2. **UE5 Chaos (official API docs):** geometry-collection replication sends break state, and sends transforms only down to a set cluster level. Past `ReplicationAbandonAfterLevel` the server stops updating fragments and clients simulate them alone. Fragments past `ReplicationMaxPositionAndVelocityCorrectionLevel` get only the initial break velocity [S]. This is the engine-level version of "debris is never replicated".
3. **Frostbite (Kihl, SIGGRAPH 2010, primary):** a destroyed wall section is shown by removing a piece, adding authored detail meshes, then a distance-field mask built from artist spheres (~2 m/texel, REPORTED). The look of a breach is derived from which parts are destroyed. Frostbite 1 needed UV-mapped masks for every destructible part, and that workflow cost drove the rewrite [S].
4. **Red Faction Guerrilla/Armageddon:** buildings contain invisible pre-made shards that are unhidden on destruction. Shard size is set per material, a separate high-res intact model is kept, and "world anchoring" decides what stays after destruction [S]. Artists "had to become structural engineers" to stop buildings collapsing at spawn, and a vehicle on a weakened bridge brings it down [S].
5. **Red Faction GDC 2009 talk** ("Blowing Up the Outside World", Arnold/Hanna) exists and is listed as free, but neither slides nor video were reachable. The stress-solver internals and multiplayer sync are still [?].
6. **Company of Heroes 3:** the official diary confirms progressive damage, collapse, debris that damages nearby units, collapses that leave new cover, and pre-battle bombing [S]. The Relic lead-artist post on modelling and fracturing could not be reached (TLS error, not archived). Via summaries: layered building shells, damage states by where a building is hit, and pre-alpha holes made with transparency maps [S~]. Building HP model, line of sight after collapse and sync are [?].
7. **THE FINALS Season 12 (Oct 2026):** adds gradual destruction (buildings "hinge, sag, bend, and deform"), blast waves, a physics-engine upgrade "to handle the extra load", and "debris piles that dynamically grow". Read via MP1st quoting the patch notes [S~]. The piles suggest that some debris now carries gameplay [I].
8. **World of Tanks (official):** only some one-storey buildings fall; felled trees become concealment like bushes; and enemies see knocked-over objects even when they cannot spot the player [S]. That last point means destruction state reaches every client regardless of spotting.
9. **Hell Let Loose (2020 FAQ via Wayback):** bridges, walls and fences were the destruction priority. Complex building and foliage destruction was tested and dropped because "the load on the network would compromise the total player count" [S]. No current primary source shows anything shipped [?]. **Squad:** UE5 (9.0) added no building destruction that I could find [?].
10. **A shared pattern across the primaries** [I]: authoritative state is a small discrete record (destroyed parts, commands or break events), delivered reliably and replayable for late join. The visible result (masks, shards, fragments past a cluster level) is derived per client and not replicated.

---

## 1. Company of Heroes 3 (and the Essence lineage)

Sources are thin. The best primary, a Relic community post by the lead environment artist, could
not be reached.

- **(a) State model.**
  - Relic's diary describes gradual damage ending in collapse: buildings show pock marks from flak and holes from larger weapons, and they collapse after enough damage [S~ diary summary; SEGA press page S for the collapse].
  - Gamepressure, writing from the diary, says destruction "will affect the places where the bullets/shells hit". A structure collapses only after severe structural damage, and it may fold inward or slide into the street and cover it with debris [S, secondary].
  - The Relic post "Destruction Overview Part 1 - Modeling & Fracturing" (Patrick Lopetrone, Lead Environment Artist) is described in summaries as follows [S~]:
    - props from buildings to foliage have "different damage states, reflecting where they've been hit, by what and how much damage they've endured";
    - buildings are built from multiple layers (plaster, bricks);
    - physics is applied to the pieces.
  - The underlying HP or section model is [?].
- **(b) Collision, cover, line of sight and traversal.**
  - Collapsed buildings "occasionally" leave "a new space where infantry can take up cover within the remaining walls" [S~ Relic post via summary]. The SEGA diary page says toppled buildings create new cover positions [S].
  - Falling debris deals damage to the HP of nearby units [S SEGA page].
  - Patch history treats garrisonability as authored per building. In 2.0.5 a small one-storey building was converted into a "blocker", meaning not garrisonable [S~].
  - Patch 2.2.0 (via summary): losing a constructed garrison no longer knocks out its occupants, and an occupied garrison grants +50% damage reduction (REPORTED) [S~]. A "Breach" ability needs ≥50% damage to the garrison (REPORTED) before attackers can occupy it [S~].
  - Line of sight after collapse is [?].
  - Crushing, from the CoH2 lineage and community sources only: vehicles carry crush tiers ("medium crush" versus "heavy crush"). Heavy crush breaks hedges, and tank traps are disputed [S~ forum, unverified]. CoH3 crush rules are [?].
- **(c) Network sync and late joiners:** [?]. No source found. I did not assume the RTS-lockstep pattern.
- **(d) Debris line.**
  - Per the Relic post, CoH1 and CoH2 had no real physics debris [S~].
  - The CoH3 pre-alpha punched holes with transparency maps instead of fracturing, and simulated debris with camera-based effects [S~]. The shipped game uses physics pieces [S~].
  - Whether those pieces are gameplay (collision, line of sight) or cosmetic is [?]. Debris damaging units implies at least a gameplay damage event at collapse [I].
- **(e) Authoring:** layered building shells plus fracturing (summary of the Relic post) [S~]. The tools are [?].
- **(f) Regrets and ceilings:** [?]. Engine: Essence uses Havok as physics middleware [S~ Wikipedia via search].

## 2. Hell Let Loose

- The FAQ, reached through a 2020 Wayback snapshot of `hellletloose.com/faq` (the live page returns 404) [S]:
  - "At present, bridges, walls and fences are at the top of the destruction priority list, as this will have significant effect on gameplay, strategy and tactics. Players will be able to shape the flow of traffic across the map by making areas very difficult for armor and vehicles to traverse safely."
  - "We tested numerous forms of more complex destruction for both foliage and buildings and decided that the load on the network would compromise the total player count of the game. It is more important to us to field 100 players on a large map, than 5 players on a destructible battlefield."
  - On the build side, engineers place barbed wire, tank traps and emplacements [S].
- The search found no current primary source (dev briefs #198, #202 and #210 were searched) saying which destruction actually shipped [?]. HLL: Vietnam adds foliage that reacts to players. That is interaction, not destruction [S~].
- The lesson I draw [I]: the scope was chosen by gameplay effect on traversal (bridges, walls, fences decide where vehicles can go). Building and foliage destruction was cut on network cost, against a 100-player target.

## 3. Squad

- The UE4 to UE5 move shipped as Update 9.0 (September 2025, REPORTED by a secondary) [S~].
- No official patch note found adds building or wall destruction. Player threads still ask for it [S~]. Status: [?]. As far as I can find, the world remains static.

## 4. Teardown (primary blog read)

The source is Gustafsson, "The unlikely story of Teardown Multiplayer", 2026-03-13. WebFetch could
not reach it; plain curl could. The parts not already covered:

- **(a) State model: hybrid determinism** [S].
  - "destruction done deterministically, while most other things use state synchronization."
  - The destruction logic was rewritten in fixed-point integer math. Each breakage event "is split into a stream of deterministic commands that are replicated on all clients", for example "cut hole in this shape at voxel coord x,y,z", "change ownership of that shape", "reconnect joint to this shape".
  - Anything that changes scene content (spawning, recolouring) uses the same command path.
- **(c) Network** [S].
  - Commands travel on a reliable, ordered stream. "The bandwidth requirements are small because commands are the same regardless of object size."
  - Transforms, velocities and player positions use unreliable state sync with eventual consistency. A per-client server priority queue favours objects visible to that client, within "around one Mbit per client" (REPORTED).
  - Clients simulate locally and are corrected when state arrives. Under load the corrections come less often and "can cause visible snapping".
  - There are no dedicated servers: the host is the server.
- **Late join** [S]. Three options were weighed:
  1. A full scene serialisation, "30-50 MB is common, often more" (REPORTED).
  2. A delta of the objects that changed.
  3. Replaying the recorded deterministic command stream.

  They chose option 3. It is "less data than we anticipated", but applying it costs computation, so "we still limit the buffer size and disable join-in-progress when it fills up."
- **(d) Debris line** [I]. Every voxel change is authoritative and deterministic. Only rigid-body motion is approximate state.
- **(f) Regrets** [S].
  - The 2021 naive prototype "send altered voxel data as objects were destroyed. It used enormous amounts of bandwidth and completely choked the connection when large objects were destroyed."
  - Gustafsson "dismissed the idea of full determinism for the entire engine (a view I have since reevaluated)". He now considers floating-point determinism "mostly safe if you know how to navigate around the pitfalls".
  - "The multiplayer implementation in Teardown isn't particularly elegant". His new engine is trying "simplified approaches".
  - The older voxagon posts on voxel physics were not read (time box) [?].

## 5. Red Faction (2001 Geo-Mod) and Guerrilla (Geo-Mod 2.0)

- **GDC 2009 primary:** "Blowing Up the Outside World: Destruction Done the Next Gen Way", by Eric Arnold and Jeff Hanna (Volition), Visual Arts track. GDC Vault lists it as "free content", but the page showed no slides or video, only a user-limit notice [S]. Its content is [?].
- **GDC 2010:** "Multiplayer Level Design in Red Faction Guerrilla", by Luke Schneider. Per the abstract, it covers [S]:
  - how destruction and terrain change level flow;
  - pitfalls of building structures for multiplayer maps;
  - "technical constraints and exploits" tied to destruction;
  - pipelines adapted for "large emergent systems".

  Media were behind the Vault login [?].
- **(a) and (b) State and stress model**, from a 2009 CBS Q&A with Volition's Eric Arnold [S]:
  - "Artists had to become structural engineers to prevent their creations from crumbling as soon as they are dropped in game." I take this to mean that structural integrity is evaluated on authored content at spawn, so every building must pass the solver [I].
  - "It absolutely does react to exactly where you hit the building, and beyond that how you hit it", and "none of it is scripted".
  - "Driving out on to a bridge after the supports have been weakened will cause the bridge to collapse". So vehicle weight counts as structural load [S], and the load path is live state [I].
  - Costs: "the increased simulation load caused by updating the physics in real time" and "having to draw the insides of walls". The PS3 build used the SPUs.
  - A review says each building "is governed by a stress system crafted from the ground up by Volition" [S~ secondary]. The solver itself is [?].
- **(d) and (e) Pre-fracture and debris**, from art director Chad Greene on Armageddon, which says it "inherited that code and built upon it" [S]:
  - "invisible shards that are in the buildings that don't get activated until destruction has happened ... we basically unhide all of the shards";
  - a separate "really high-res version" of the intact building is kept "but not have to be at the cost of the shards";
  - shard size is set per material ("concrete will have a different size from fiberglass");
  - an early test was so dense "you could write your name into the wall";
  - "World anchoring basically has to do with what stays around and what doesn't after destruction".
- **(c) Multiplayer:** "When you get more people and simultaneous destruction, you do have to consider it" [S Greene]. No sync mechanism is stated [?].
- **(f) Regrets and ceilings:** QA must break "everything ... from every angle, with every weapon", and the bugs are "hard to reproduce" [S Greene]. Guerrilla had about five years of engine development behind it [S~ article summary]. Debris "tended to be on the chunkier side" because of hardware limits [S~ PC Gamer via summary].
- **Red Faction 2001 (runtime CSG):** no primary found [?]. The community client Dash Faction exposes a `geomod_limit` setting [S~]. A forum anecdote ties geomod to multiplayer desync [S~, weak].

## 6. Frostbite / Battlefield, primary technical sources

- **Kihl, "Destruction Masking in Frostbite 2 using Volume Distance Fields"**, SIGGRAPH 2010 Advanced Real-Time Rendering course (PDF read) [S]:
  - **The destroy sequence:** "When destroying a part of this house, we begin by removing a piece of the geometry. We then add detail meshes around the destroyed section. And the final step is to add the destruction mask." Particles and mesh debris are layered on top.
  - **Frostbite 1** (Bad Company, 1943, BC2) used UV-mapped masks: "The main issue we wanted to address was the time consuming workflow which involved creating UV maps for each destructable part."
  - **Frostbite 2:** spheres are auto-generated, "so that each destructable part has one". The artist can move, scale or add spheres. Each masked geometry gets its own signed volume distance field: "approximatly 2 meters per pixel", "typically 8x8x8" (both REPORTED), packed into atlases.
  - **Two draw paths:**
    - deferred-decal volumes;
    - PS3 SPU triangle culling, which splits triangles into inside and outside index buffers. The culling is redone "only when the distance field changes".
  - The culling path won on PS3 GPU time but was a platform-specific code path. The choice was left open.
  - For Overmatch [I]: the authoritative input is the set of destroyed parts. Mask, detail meshes and debris are derived per client at render time.
- **DICE talks on destruction networking or pre-fractured states** (BC2 "Destruction 2.0", BF3, BF4 "Levolution"): none found in the time box [?]. Search hits were marketing ("massive networked destruction") or a DICE audio talk [S~].

## 7. THE FINALS since GDC 2024

- **Season 12 "PRIME TIME"** (patch 1.000.159, 2026-10-20/21, REPORTED). Read via MP1st quoting the patch notes; the official reachthefinals.com page was not found (`/patchnotes/1200` returned 404) [S~]. The notes say:
  - "the biggest update to the destruction system since launch";
  - "Buildings can now hinge, sag, bend, and deform." Gradual destruction is enabled game-wide, and "every map has had its destruction completely rebalanced";
  - suspended structures now "hang from physics-enabled attachments allowing them to swing freely";
  - blast waves from breach charges, drills, thermal bore and C4: "walls will dent, trees can snap";
  - "We've also upgraded the physics engine to handle the extra load from the new destruction tech."
  - "debris piles that dynamically grow and form as buildings crumble";
  - new audio and particle cues make "the fragile, weakened state of partially destroyed buildings easier to read".
- **Season 9 official notes** (via summary): "objects remain attached when structures come down" [S~].
- **Inference** [I]: growing debris piles and deforming (not just breaking) structures mean the replicated state now includes continuous deformation and some debris that shapes play. That would extend the GDC 2024 transform-replication budget. No source states the network or server cost [?].
- **Late joiners, debris lifetime, server cost per match:** [?]. Embark repeats that destruction is server-side [S~].

## 8. Unreal Engine 5 Chaos Destruction (official docs)

- **Model.** From the Destruction Overview page [S]:
  - assets are built "using pre-fractured geometry";
  - clustering "will simply join the mass and inertia of each connected element into one larger single rigid body";
  - a connection graph of nearest neighbours carries "initial strain values"; connections break when an impulse from a collision or a field "exceeds the connections limit";
  - fields can "decrease the internal strain values" to weaken a structure;
  - a cache system replays heavy destruction cheaply.
- **Component API**, Python API 5.5 (`unreal.GeometryCollectionComponent`) [S]:
  - `enable_replication`: "Per-instance override to enable/disable replication".
  - `replication_abandon_after_level`: "the cluster level after which replication will not happen". It is enabled by `enable_abandon_after_level` ("stop providing network updates to clients"). It replaces the deprecated `replication_abandon_cluster_level` ("the cluster level to stop sending corrections for geometry collection chunks").
  - `replication_max_position_and_velocity_correction_level`: "the maximum level where clusters will have their position and velocity send over". A search summary of the C++ docs adds: "When breaking, client will only receive the initial break velocity" [S~].
  - `abandoned_collision_profile_name`: "Whether abandoned particles on the client should continue to have collision". Client-only fragments can be configured as non-colliding.
  - Damage: `damage_threshold`; `use_size_specific_damage_threshold` ("Damage threshold for clusters at different levels"); `damage_propagation_data`; `enable_damage_from_collision` ("collisions ... will apply strain"); `max_cluster_level`; `collision_profile_per_level`.
  - Cleanup: `allow_removal_on_break` and `allow_removal_on_sleep`.
  - Events: `notify_breaks`, `notify_crumblings` and `notify_removals` (per component, or through a global relay).
  - Throttles (community notes): `p.Chaos.Clustering.PerAdvanceBreaksAllowed` and `...PerAdvanceBreaksRescheduleLimit` [S~].
- **Release notes**, via search summary [S~]:
  - 5.3: "server authoritative Geometry Collection particles were made unbreakable on the client". In server-authoritative mode the client cannot decide a break [I]. Also a fix for replication relying on `bFirstUpdate`.
  - 5.5: a fix for "internal cluster information was not properly sent over to the client".
  - 5.8: a fix for "crashes happening when using replication with Geometry Collections".
- **Field reports** (forum, community, unverified) [S~]: in 5.3 the initial break velocity is replicated and the client predicts where fragments end up, so debris positions diverge between client and server. Workarounds are to turn off abandon-after-level or use `p.bGeometryCollectionRepUseClusterVelocityMatch 0`.
- **GDC 2025, Epic, "Dynamic Destruction in UE5 with the Chaos Destruction System"** (VanAllen, Caillaud). The abstract covers strain manipulation, break and shock propagation, anchoring from Fracture Mode, cost-effective destruction without heavy field use, and Niagara Data Channels for gameplay hooks. It says nothing about networking [S].
- **Havok Destruction and breakable-constraint docs:** not reached in the time box [?].
- **Takeaway** [I]: Epic's layering is the same two-tier split. Cluster break state (and transforms down to a chosen level) is server state; deeper fragments are abandoned to client-only simulation and can drop collision.

## 9. World of Tanks, official, on buildings

- From the official "20 secrets" article (undated) [S]:
  - "Some single-story buildings are destructible, but most two-story ones won't fall down" and "find a taller building that you won't accidentally bulldoze";
  - "Standing trees don't really conceal, but knocked-down ones do", so a felled tree "can be treated like a bush";
  - "The enemy sees trees and other destructible things knocked over even when they can't spot you." Destruction state goes to all clients regardless of spotting. That is a deliberate information channel, or at least an accepted one [I];
  - "Destroyed tanks soak up artillery splash damage", and shots can go through windows at the right angle.
- The 2014 physics dev diary (worldoftanks.asia) only announces "fully-destructible buildings and tank components" [S]. Secondaries say ramming and HE break buildings by material, and objects become pushable [S~].
- What ruins block (shells, sight, tanks), and the Havok collapse behaviour on reworked maps: no official source found beyond the above [?].

## 10. Extras

- **Valheim structural integrity** (secondary sources only) [S~]:
  - support decays with the number of connections from a ground-anchored piece;
  - longer pieces (beams, poles) span more per connection;
  - stone, or attachment to stone, resets wood to full support;
  - a colour readout (blue, then yellow, orange, red) shows remaining support, and red pieces cannot carry more.

  The mechanism it adds [I]: a cheap graph-distance support rule instead of a load solve. It is a readable, deterministic alternative to the solvers in THE FINALS and Red Faction Guerrilla.
- **Others:** none read. Battlebit and Delta Force were not pursued.

---

## Source index (all read 2026-10-30 unless marked)

- Gustafsson, D., "The unlikely story of Teardown Multiplayer", 2026-03-13: https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html (WebFetch blocked; read via curl).
- Kihl, R. (DICE), "Destruction Masking in Frostbite 2 using Volume Distance Fields", SIGGRAPH 2010 ARR course, PDF: https://www.advances.realtimerendering.com/s2010/Kihl-Destruction%20in%20Frostbite(SIGGRAPH%202010%20Advanced%20RealTime%20Rendering%20Course).pdf
- Epic, Chaos Destruction Overview: https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview
- Epic, Python API `GeometryCollectionComponent` (5.5): https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollectionComponent?application_version=5.5 (first 100k chars read)
- Epic C++ API `UGeometryCollectionComponent`: https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/GeometryCollectionEngine/UGeometryCollectionComponent (empty render). The 4.27 API page returned 403.
- Epic UE 5.3/5.5/5.8 release notes, via search summary only: https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-5-5-release-notes , https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-8-release-notes
- UE forum, "5.3 chaos destruction replication": https://forums.unrealengine.com/t/5-3-chaos-destruction-replication/1326436 (via search summary)
- hzFishy notes, Geometry Collection: https://notes.hzfishy.fr/Unreal-Engine/Physics/Types/Geometry-Collection
- GDC Vault, Epic, "Dynamic Destruction in UE5 with the Chaos Destruction System" (GDC 2025): https://www.gdcvault.com/play/1035357/Dynamic-Destruction-in-UE5-with
- GDC Vault, Arnold & Hanna, "Blowing Up the Outside World" (GDC 2009): https://gdcvault.com/play/1670/Blowing-Up-the-Outside-World (metadata only)
- GDC Vault, Schneider, "Multiplayer Level Design in Red Faction Guerrilla" (GDC 2010): https://gdcvault.com/play/1012330/Multiplayer-Level-Design-in-Red (abstract only)
- CBS News, "Red Faction: Guerrilla Q&A" (Eric Arnold, Volition): https://www.cbsnews.com/news/red-faction-guerrilla-q-a/
- Game Developer, "The Destructible World: Building Red Faction: Armageddon" (Chad Greene): https://www.gamedeveloper.com/design/the-destructible-world-building-i-red-faction-armageddon-i-
- SEGA press, "Company of Heroes 3 – Destruction Dev Diary": https://sega.prezly.com/company-of-heroes-3-destruction-dev-diary
- Relic community, "Destruction Overview Part 1 - Modeling & Fracturing": https://community.companyofheroes.com/coh-franchise-home/company-of-heroes-3/forums/1-general-discussion/threads/2212-company-of-heroes-3-destruction-overview-part-1-modeling-fracturing (**inaccessible**: TLS certificate mismatch, empty via curl, not in Wayback). Content only via search summaries.
- Gamepressure, CoH3 destruction: https://www.gamepressure.com/newsroom/company-of-heroes-3-new-level-of-destruction-in-rts-games/zf45db
- PC Gamer CoH3 articles: https://pcgamer.com/company-of-heroes-3-makes-destruction-an-art-form and https://www.pcgamer.com/company-of-heroes-3s-environmental-gameplay-lets-you-literally-reshape-the-battlefield (**body truncated**; only search snippets)
- GameSpot CoH3 articles: https://www.gamespot.com/articles/grand-theft-auto-6-release-date-trailed-in-new-official-video/1100-6499603/ and https://gamespot.com/articles/company-of-heroes-3-video-details-new-ways-to-blow-up-everything-on-the-map/1100-6504674/ (**ECONNRESET**, unread)
- CoH2.org forum, "Panther, why does it have heavy crush?": https://Www.Coh2.org/topic/83307/title/post/703188 (via search summary)
- Hell Let Loose FAQ, Wayback 2020 snapshot: https://web.archive.org/web/2020id_/https://www.hellletloose.com/faq (read via curl). The live page https://www.hellletloose.com/faq returns 404.
- Squad UE5 9.0, via search summary only (steamdb / gamepressure): https://www.gamepressure.com/newsroom/squad-90-trailer-drops-with-unreal-engine-5-upgrade-but-the-new-p/zb7f8f
- MP1st, THE FINALS Season 12 patch notes: https://mp1st.com/?p=317079 . Official https://reachthefinals.com/patchnotes/1200 returned 404. Season 9 official: https://reachthefinals.com/patchnotes/900 (via search summary only)
- World of Tanks, "20 secrets": https://worldoftanks.com/en/news/general-news/20-secrets
- World of Tanks, "Developer diaries: physics" (c. 2014): https://worldoftanks.asia/en/news/general-news/developer-diaries-physics/
- Dash Faction (Red Faction 1 community client): https://github.com/rafalh/dashfaction (via search summary)
- Valheim structural integrity, secondaries: https://sportskeeda.com/esports/understanding-structural-integrity-valheim , https://mobalytics.gg/gamebase/guides/valheim-general-building-guide (via search summary)
