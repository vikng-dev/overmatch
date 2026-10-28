# AP vs APHE behind the plate: shipped tank games (lane A, 2026-10-23)

Recommendation-only; brief `brief-games.md`. No law proposed here.

Labels: **MEASURED** = a number the game or its developer published; **DERIVED** = computed
here from MEASURED numbers. `[n]` = source list at the end. **(index)** = text read from a
search-engine extract because the page refused a fetch (401/403/captcha/reset): weaker
evidence. **community** = player posts, not developer statements.

Not repeated here: burst-volume precedents (`research/burst-model.md:8-19`) and the
feedback use cases per game (`product/competition-tanks.md:8-51`).

## 1. Comparison

| Game | Round classes; per-round data | AP after perforation | APHE burst | Crew | Modules | Shooter sees |
|---|---|---|---|---|---|---|
| **War Thunder** | AP/APC/APBC/APCBC, APHE(BC/C/CBC), APCR, APDS, HEAT, HESH [3]. Per round: fuse delay (m), fuse sensitivity (mm), explosive mass (g TNT eq). BR-350A: 1.2 m, 14 mm, 150 g, 6.3 kg; PzGr 39: 1.2 m, 14 mm, 28.9 g, 6.8 kg (MEASURED) [19][20]. No per-round cone angle or fragment count is published | The shell continues and "loses penetration value and kinetic damage proportionally to armor thickness". Each kinetic shell throws a "shard cone"; at 76 mm it is 2.5 m long at 15° (MEASURED; half- or full-angle not stated) [1]. "Solid AP does not stop until it has exhausted all of its force" [3]. Fragments come in groups: "the more powerful the fragments, the narrower the cone … and the smaller their number"; 3–30 mm RHA (MEASURED) [8] | The fuse arms only on armour ≥ sensitivity; on thinner armour the shell flies on [2][3]. It bursts at the fuse-delay distance past the penetration point (0.5–1.5 m in 2013) [1][2]. The burst is a "simplified pattern without sectors" of fragments, plus an HE sphere "smaller than" the fragment zone that does "relatively little damage" [12]. Overpressure needs ≥ 170 g TNT [5]. Since 2024-10-02 the warhead survives as a "mini-AP round" if explosive < 1.33 % of shell mass [15]. A shell that stops before the delay: not found | Each crewman is "a separate unit", damaged linearly to "black" (incapacitated); fewer than two left = vehicle lost [4]. "Shard damage is enough to knock out a crew member on a single hit" [1]. Wounded crew heal automatically since Seek & Destroy (2024) [4][11]. Loader: reload ×1.25 if wounded, ×2 if knocked out (MEASURED) [9] | Colour states. Linear: engine, transmission, barrel. Non-linear: radiator, fuel tank, drives ("intermediate damage has little effect") [4]. A destroyed fuel tank stops shielding (2.3.0.42) [4]. Modules were added to fill voids in 2024 [11] | Hit cam per hit: shell, plus red traces for fragments reaching modules [1]. Protection Analysis gained a "testing shells new parameters" option for the APHE test [14]. More: `competition-tanks.md:12,35,47` |
| **World of Tanks** | AP, APCR, HE, premium HE, HEAT, HESH; no APHE [24]. Fuse and fragment data: none published (not found) | A penetrating shell "move[s] chaotically and may strike modules and crew alike"; "transverse penetration of the vehicle is not possible" [21]. Console: projectiles "still travel inside the tank piercing modules" [25]. It flies 10 calibres past spaced armour or external modules (index) [30] | No APHE. HE: a penetrating HE also damages "multiple internal modules and/or crew" [24]. Blitz: "tracers (shell fragments)" go from the burst point to each module, and each module rolls to "dodge" [23]. 1.13 (2021-06-23) cut the crit chance against modules and crew; fragmentation on the armour does no damage, but spall inside always does some [27][57] | "All crew members are regarded as internal modules"; all injured = destroyed [22]. Crits to crew can occur "without damaging the vehicle" [21] | HP per module: 50 % = yellow, 100 % = red/critical; auto-repair to 50 %; the ammo rack detonates at 100 % [22]. Blitz damage chance on hit: engine 45, gun 33, ammo rack 27, turret 45, fuel 45, tracks 100 % (MEASURED, Blitz). If the module "dodges", the shell "falls apart" [23] | "Critical Hit" ribbon for damaging a component or crew member [29]; per-target ribbons and a log [28]; shot-result texts such as "Screen not penetrated" and a red/yellow/green reticle (1.13) [27]. No cam |
| **Steel Beasts Pro PE** | Modern and legacy rounds; the data page gives penetration only, and in-sim values run lower (index) [35]. APHE model: not found | Not found | Not found | "Direct" crew damage (the round passes through the crewman) vs "indirect" (spall). Liners lower only the indirect probability; most crew losses come from spall after a penetration (index) [31]. Any crew damage = "incapacitated" (forum, index) [34] | Damage model v2.460+ gives a "greater chance of damage" (index) [32]. The outcome follows hit locality and stowed ammunition (Ssnake, 2022, index) [33] | AAR; hits drawn as "colored rods" (`competition-tanks.md:51`) |
| **Gunner HEAT PC!** | Categories Penetrator / ShapedCharge / Explosive. Fields: `RhaPenetration`, `SpallMultiplier`, `MinSpallRha`, `MaxSpallRha`, `ForcedSpallAngle`, `DetonateSpallCount`, `TntEquivalentKg`, `ImpactFuseTime`, `RhaToFuse`, `ShatterOnRicochet` (field names MEASURED from mods that set them; the values there are the mod authors') [38]. No APHE round in the data surveyed | Official: a hit "produces spalling, fragmentation, and blast overpressure … on individual crew members and vehicle systems" (index) [36]. "Every piece of spall flies on its own" (third party) [39] | No APHE. The fragment count of a burst is round data (`DetonateSpallCount`, e.g. 150 in one mod) [38] | Limbs and major organs (build 20200710, index) [37]; "hit zones down to his eyes" (third party) [39]. A vehicle is knocked out only when combat-ineffective (index) [36] | Turret drives, elevation, engine, transmission, tracks. Ammunition has separate detection for propellant and warhead (index) [37]. Cook-off comes 10–25 s after heating (third party) [39] | Base game: "You fire, the target stops, maybe it starts smoking, and that's it" (third-party mod README, 2026) [39]. AAR only (`competition-tanks.md:17,50`) |
| **Sprocket** | Solid AP and APHE; HE was "in the files but removed" (community, index) [41]. Data: not found | V0.1136 (posted 2022-02-19): fragments "bounce around the inside, damaging anything they hit"; they "may also go out the other side …, meaning more penetration isn't always better"; "lower calibers hit fewer internals, higher calibers hit more" [40] | Not found | 100 health. Cannon base damage +50, so a crewman hit by the smallest cannon "wont be able to walk it off anymore" [40] | "Big absorbers like fuel and transmissions will now absorb fragments better"; "more partially working cases" [40] | Nothing live (`competition-tanks.md:17`) |
| **Post Scriptum / Squad 44** | 2021 armoured update (live by 2021-11-22 [43]): AP, APC, APCBC, APCR, HVAP, APDS, AP-HE, APC-HE, APCBC-HE, HEAT, in four base types: AP / HE / AP with HE filler / HEAT (index) [42] | Not found | The filler is "triggered by hitting objects or armour at a certain thickness" and bursts shortly after, hitting crew and internals; it "will detonate on the inside like a hand grenade" (index) [42]. Later notes: 40 mm APHE "Explosive Damage Outer Radius from 800 -> 500" (index, undated, unit not stated) [42] | Not found | Ammo stowage: "high chance" of fire or instant explosion (index) [42] | "New impact sounds for tank shells inside and outside of tanks" (index) [42] |
| **Squad** | No AP/APHE split documented (not found) | Not found | n/a | Not found | Engine, tracks/wheels, turret, ammo compartment become inoperable (community wiki, index) [45]. v2.9 (2021-06-28): a rack at 0 HP cooks off after a random 15–30 s, and "overdamage" was removed to cut one-hit kills (index) [44] | Not found |
| **Arma 3 + ACE3** | ACE reads the projectile's `warheadName` ∈ {he, ap, heat, tandemheat}; no APHE class. AP damage is scaled by speed and by (1 − `explosive`) [47] | Vanilla: penetrate the fire geometry, then damage the hitpoint (BI wiki, index) [48]. ACE: below a hitpoint's `minDamage`, the penetration chance = damage/minDamage. A hull hit that passes the effectiveness roll gives 25 % turret jam, 25 % mobility, 25 % both [47] | n/a | ACE docs: "a calculation takes place and determines which people inside … should get hurt" [46]. ACE code: crew take medical damage only on a rolled detonation or on added damage ≥ 15 [47]. The docs and the code disagree | Engine, hull (= crew compartment), turret, gun, track, wheels, fuel, SLAT, ERA [46] | Not found |
| **Hell Let Loose** | AP and HE stocks per tank. HP pool, e.g. Firefly AP damage 850 → 920 (MEASURED, 2026 experimental) [50] | Hitboxes rebuilt for "a very binary outcome". Target shots to kill: mirror ≈ 2, up-tier 2–3, down-tier 1–2 (MEASURED design target, 2021) [49]. 2026: armour resistance as a % by hull, Heavy front 32 % [50] | n/a | Not found | A penetrated engine bay immobilises; the engine disables at 0 % HP [49] | Penetration, non-penetration and ricochet effects and sounds made "far more distinct" [49] |
| **IL-2 Tank Crew** | AP and APHE; German guns had no solid AP (dev blog 195, index) [55] | A projectile that the plate "almost stopped" does "much less" internal damage (2015) [51] | Fragments get speed, mass and size, "universal for all munition types" (2020) [53] | "Simple GUI damage notifications for systems and crewmen" (2019) [52] | Engines, gearboxes and ammo racks gained hard casings (2020) [53] | GUI notifications [52]; updated hit audio [53] |
| **Armored Warfare** | AP/APFSDS, HEAT, HE, HESH [54] | Only main-armour penetrations damage. Kinetic and HEAT penetrators damage modules; crew effect not stated [54] | No APHE. HE: "massive damage to modules within a certain spherical radius", "injuring the crew within the shell's blast radius" [54] | HE and HESH injure crew; numbers: not found [54] | Module damage; numbers: not found [54] | HP and component icons (`burst-model.md:13`) |

DERIVED from the table:
- WT warhead-survival rule [15]: PzGr 39 28.9/6800 = 0.43 % < 1.33 %, so it keeps a mini-AP remnant;
  BR-350A 150/6300 = 2.38 % > 1.33 %, so it does not. The ratio uses TNT-equivalent mass, which is
  all the wiki lists, so it is approximate.
- WT overpressure (≥ 170 g [5]): neither BR-350A (150 g) nor PzGr 39 (28.9 g) qualifies. Their
  behind-armour effect is fragments plus the small HE sphere.
- WT AP shard cone at 76 mm (2.5 m, 15° [1]): base radius 0.33 m if 15° is the full angle, 0.67 m if
  it is the half-angle.
- WT fuse delay of 1.2 m at a residual 600 m/s is about 2 ms. GHPC names its fuse field as a time
  (`ImpactFuseTime`); its unit is undocumented.

## 2. Balance history: knobs turned and why

| Date | Game | Knob | Stated reason | Outcome |
|---|---|---|---|---|
| 2020-05-20 | WT 1.97 | APDS fragment count tied to "shell mass and parameters": < 100 mm reduced, ≥ 100 mm increased [6] | "More precise correlation" [6] | Shipped |
| 2021-02-02 | WT | 20 mm AP cone fixed: it had a large-calibre cone, and now hits "in a narrow cone along the trajectory" [7] | Damage to match projectile energy [7] | Shipped; the cone is per-calibre data, and a wrong default is a bug |
| 2021-06-23 | WoT 1.13 | HE: lower crit chance against modules and crew; no damage from fragmentation on the armour; a screen costs 3× its thickness [27] | "Avoid a huge spread in the final value of damage"; big damage to weak vehicles "very often … by accident" [26] | Shipped |
| 2021-06-28 | Squad v2.9 | Ammo rack: cook-off at 0 HP after 15–30 s; overdamage removed (index) [44] | Cut one-hit kills [44] | Shipped |
| 2021-07-27 | HLL U10 | Hand-built hitboxes; shots-to-kill targets [49] | A "very binary outcome" that players "can learn and rely on" [49] | Shipped |
| 2022-02-19 | Sprocket 0.1136 | Post-penetration rewrite: bouncing fragments, overpenetration, absorbers, crew one-hit [40] | (not stated) | Shipped |
| 2023-12-20 | WT | A refactor bug had cut secondary-fragment counts "for some weapons" [8] | Bug | Fixed |
| 2024-04-22 | WT poll | Crew stun: rejected. Internal fires: dropped on a ~2 % split. New modules and crew healing: accepted [10] | Hits that pass through voids, where the shooter "did everything right" but was "unlucky" [9] | Healing 85.67 %, modules 85.84 % (MEASURED); shipped 2024 [11] |
| 2024-08-13 → 10-02 | WT APHE | Fragments raised "to the level of" AP; warhead kept when explosive < 1.33 %; burst sectors (head + base + case cone) put to a vote [12][14][15] | The current sphere is "simplified". Sectors make cupola hits less lethal and hull hits more [12] | Test vote: 48.64 % for, which Gaijin read as "significant" interest [13]. Remnant shipped [15]. Sectors not shipped as of 2026-08 (community) [16] |
| 2024-10-02 | WT | Fragments from near-limit penetrations strengthened [15] | They "will now penetrate more inside a tank" [15] | Shipped |
| 2026-05-21 | HLL U20 (experimental) | Armour resistance as a % reduction by hull [50] | "Realistic penetration and damage resistance" [50] | Experimental |

What the community measured: vote shares (Gaijin-published) and anecdote. "9/10 times you'll one
shot the whole crew" of a Panzer IV with APHE; solid AP "vanish[es]" after one crewman
(community, 2025-11-07) [17]; calls to nerf APHE or buff AP (2024-06-03) [18]. Not found in any
title: a systematic measurement, such as kill rate per round class.

## 3. Lessons (ordered by bearing on the owner's model)

1. **AP = slug plus a narrow cone is shipped practice, and the cone is per-round data.** WT's solid
   shot continues, losing energy with armour, and throws a shard cone [1][3]. A 20 mm round that
   carried a large-calibre cone was a balance bug until 2021-02-02 [7].
2. **The shipped cone is a strength-vs-angle spread, not one angle.** WT's fragment groups get
   narrower and fewer as they get stronger, at 3–30 mm RHA [8]. A near-limit penetration throws
   stronger fragments in WT [15] and does "much less" in IL-2 [51]. Exit energy, not the round
   class, shapes the cone.
3. **APHE's exit cone is treated as the same object as AP's.** In 2024 Gaijin called weaker APHE
   fragments a "technical" artefact and raised them to the AP level [12][14].
4. **Shipped APHE keeps a slug when the filler is small.** Since 2024-10-02 WT keeps the warhead as
   a "mini-AP round" when explosive < 1.33 % of mass, so it reaches crew "behind internal
   partitions" [12][15]. The filler ratio is the data that selects (DERIVED: PzGr 39 keeps it,
   BR-350A does not).
5. **The burst's shape is the most contested knob.** WT ships a sectorless sphere [12]. The
   forward model (head + base + case cone) lowers cupola lethality and raises hull lethality
   [12]. It was tested and not shipped after a 48.6/51.4 split [13][16]. Once players are used to
   a lethality level, cutting it is politically hard.
6. **The explosion is minor; the fragments kill.** WT's APHE HE sphere does "relatively little
   damage" [12], and overpressure needs ≥ 170 g TNT [5], which typical WWII APHE lacks (DERIVED).
   Arming is data: below the fuse sensitivity, APHE behaves as a solid shot and flies on [2].
7. **Crew are fragile, but shipped games keep a wounded state.** WT: one shard can knock out a
   crewman [1], yet crew degrade linearly to "black" and heal (85.67 %) [4][11]. Sprocket made the
   smallest cannon's hit un-survivable [40]. GHPC models limbs and organs [37]. Steel Beasts rolls
   direct and spall crew damage separately [31].
8. **The worst feel is the penetration that hits nothing.** Gaijin's diagnosis was a shooter who
   "did everything right" but was "unlucky" [9]. Players rejected a stun patch; the fix was more
   internal modules (85.84 %) [10][11]. They added targets rather than widening cones.
9. **Sturdy modules survive partly and shield what is behind them.** WoT has a 50 % "damaged"
   state before "critical" [22]. WT's non-linear modules ignore intermediate damage, and a
   destroyed fuel tank stops shielding [4]. Sprocket's big absorbers soak fragments [40]. In Blitz,
   a module that "dodges" ends the shell [23].
10. **Studios tuned for predictability, and deep simulation goes unseen without a replay.** WoT
    acted on a "huge spread" [26], HLL on "binary" outcomes [49], Squad on one-hit racks [44]. In
    GHPC "you fire, the target stops, maybe it starts smoking" [39]. The WT hit cam draws only the
    fragments that reach modules [1].

## 4. Where the owner's model matches or departs from shipped practice

- **Matches, AP.** A solid slug continues until spent, and a narrow cone leaves the exit [1][3]. In
  shipped games the cone's width and strength are per-round data that scale with calibre and exit
  energy [7][8][15].
- **Matches, APHE exit cone.** WT now gives APHE the same exit fragments as AP [12][14].
- **Departs, "no slug".** WT's newest rule keeps a slug for low-filler APHE (< 1.33 %) [15]. Shipped
  APHE also carries an arming threshold (fuse sensitivity, mm) and a delay (m). A plate too thin to
  arm it makes APHE behave as AP [2]. The owner's model names neither.
- **Departs, "a second, wider cone".** The shipped burst is a sphere, with the HE part smaller than
  the fragment zone [12]. A forward cone is what Gaijin proposed and failed to ship [12][16]. The
  owner's model resembles that proposal, which brings its predicted effect: less lethal cupola
  hits, more lethal hull hits [12].
- **Partly departs, crew.** "Killed unless a slim graze" matches WT's one-shard knockout [1] and
  Sprocket's one-hit crewman [40]. Every shipped model surveyed also keeps a wounded tier: WT's
  colour states and healing [4][11], Steel Beasts' probabilities [31], GHPC's limbs and organs [37].
  The repo already wounds crew by line chord and field dose (`.agents/docs/adr/0040-crew-are-witnesses-not-matter.md:16-22`),
  which is the axis a graze sits on.
- **Matches, sturdier components.** They survive "a little", with partial states: WoT's 50 % [22],
  WT's non-linear modules [4], Sprocket's partially working internals [40]. No game surveyed lets a
  module shrug off a full cone. Several let a module shield what lies behind it [4][23][40].

## 5. Not found

WT: what an APHE does when it stops before its fuse delay; published sphere radii. WoT PC: module
HP and crit chances (Blitz figures only). Steel Beasts, Sprocket, Squad: any per-round
post-penetration data, and any APHE model (Steel Beasts, Sprocket). GHPC: an official damage
document; any APHE round. Post Scriptum: the devblog itself (Steam fetch failed). Arma 3 vanilla:
post-penetration crew rules (BI wiki fetch failed). Combat Mission: forum gated (403). Only
index evidence was found: a 2015 developer post saying every polygon has an armour rating and a
hit can penetrate, partially penetrate, spall or bounce [56]. Any title: a systematic community
measurement of lethality by round class.

## Sources (all retrieved 2026-10-23)

1. Gaijin, "Developers' Diaries: Ground Forces Damage Model [Part 2]", 2013-12-25. https://warthunder.com/en/news/387-Developers-Diaries-Ground-Forces-Damage-Model-Part-2-en
2. Gaijin, "New shell icons" (fuse delay and sensitivity definitions), 2014-09-16. https://warthunder.com/en/news/670/current
3. WT Wiki, "Tank ammunition", dated 2018-06-01. https://wiki.warthunder.com/weapon/2544-tank-ammunition
4. WT Wiki, "Ground vehicle modules", dated 2018-05-25 (text includes 2024 Seek & Destroy changes). https://wiki.warthunder.com/mechanics/4775-ground-vehicle-modules
5. WT Wiki, "The Mechanics of High-Explosive Effect and Overpressure", 2025-07-31. https://wiki.warthunder.com/mechanics/4236-the-mechanics-of-high-explosive-effect-and-overpressure
6. WT Update 1.97 notes (console 3.09 mirror), 2020-05-20. https://mp1st.com/news/war-thunder-update-3-09-may-20-brings-version-1-97-2-10-on-consoles
7. Gaijin (Ouiche), "Server update 02.02.2021", 2021-02-02 (devtrackers mirror). https://devtrackers.gg/war-thunder/p/410da3ca-server-update-02-02-2021-planned-br-changes
8. Gaijin, "Detailed information surrounding Spall Liners in War Thunder", 2023-12-20. https://warthunder.com/en/news/8670-development-detailed-information-surrounding-spall-liners-in-war-thunder-en
9. Gaijin (Stona_WT), "Following the Roadmap: Possible Changes to Ground Vehicle Damage Models", 2024-04-16. https://forum.warthunder.com/t/following-the-roadmap-possible-changes-to-ground-vehicle-damage-models/98214
10. Gaijin, "Results of polling on the new points in the Road Map", 2024-04-22. https://warthunder.com/en/news/8860-roadmap-results-of-polling-on-the-new-points-in-the-road-map-en
11. Gaijin, "Following the Roadmap: Additions in the Second Major Update of 2024", 2024-06-25. https://warthunder.com/en/news/8960-roadmap-following-the-roadmap-additions-in-the-second-major-update-of-2024-en
12. Gaijin (Stona_WT), "Following the Roadmap: Voting to Test our Proposed APHE Shell Changes", 2024-08-13. https://forum.warthunder.com/t/following-the-roadmap-voting-to-test-our-proposed-aphe-shell-changes/141855
13. Gaijin (magazine2), pinned explanation about voting and testing APHE changes, 2024 (devtrackers mirror; exact date not shown). https://devtrackers.gg/war-thunder/p/c08037dd-pinned-explanation-about-voting-and-testing-changes-to-aphe-shells
14. Gaijin (Stona_WT), post #3107 (dev-server APHE test), 2024-09-03. https://forum.warthunder.com/t/following-the-roadmap-voting-to-test-our-proposed-aphe-shell-changes/141855/3107
15. Gaijin (Stona_WT), "Update 2.39.0.53", 2024-10-02. https://forum.warthunder.com/t/update-2-39-0-53/165189
16. Community, "Can we just all agree that Gaijin straight up gave up on … APHE?", 2026-08-18/19. https://forum.warthunder.com/t/can-we-just-all-agree-that-gaijin-straight-up-gave-up-on-actually-addressing-the-state-of-aphe/347643
17. Community, "If APHE won't get its realistic nerf.. then it's time to unrealistically buff AP", 2025-11-07. https://forum.warthunder.com/t/if-aphe-wont-get-its-realistic-nerf-then-its-time-to-unrealistically-buff-ap/282085
18. Community, "I'm worried about the planned rework on APHE shells", 2024-06-03. https://forum.warthunder.com/t/im-worried-about-the-planned-rework-on-aphe-shells/114202
19. WT old wiki, F-34 (76 mm) ammunition, BR-350A row (index; 401 on fetch), undated. https://old-wiki.warthunder.com/F-96_(76_mm)/Ammunition
20. WT wiki, KwK40 L43 (75 mm) ammunition, PzGr 39 row (index), undated. https://wiki.warthunder.com/KwK40_L43_(75_mm)/Ammunition
21. Wargaming support, "Armor Penetration" (WoT), undated. https://wargaming.net/support/en/products/wot/article/10220/
22. Wargaming support, "What Happens When a Shell Hits a Vehicle?" (WoT), undated. https://wargaming.net/support/en/products/wot/article/15762/
23. Wargaming support, "How do shells deal damage?" (WoT Blitz), undated. https://wargaming.net/support/en/products/wotb/article/15412/
24. WoT Modern Armor support, "What is Ammo and how it works", undated. https://modernarmor.worldoftanks.com/support/en/products/wotx/article/301/
25. WoT Modern Armor support, "How Penetration works", undated. https://modernarmor.worldoftanks.com/support/en/products/wotx/article/302/
26. Wargaming, "[Sandbox] High-Explosive Shells Revision: Join the Testing!", undated. https://worldoftanks.eu/en/news/general-news/sandbox-october/
27. Wargaming, "Release Notes 1.13", undated page. https://worldoftanks.eu/en/content/docs/release_notes/release-notes-1-13/
28. Wargaming, "Update 9.17.1: New and Improved Battle Indicators", undated. https://worldoftanks.com/en/news/general-news/9171-damage/
29. WoT Modern Armor, "The Game HUD" guide, updated 2026-02 (index). https://modernarmor.worldoftanks.com/en/cms/guides/game-hud/
30. WoT Fandom, "Gunnery & Armor Penetration" (index; 402 on fetch), undated. https://worldoftanks.fandom.com/wiki/Gunnery_%26_Armor_Penetration
31. SBWiki, "Spall Liner" (index; captcha/403 on fetch), undated. https://www.steelbeasts.com/sbwiki/index.php/Spall_Liner
32. SBWiki, "Tactics" (index), undated. https://www.steelbeasts.com/sbwiki/index.php/Tactics
33. Steelbeasts.com forum, "Is T-72B3 Damaging model at close Ranges accurate…", 2022-08 (index; 403). https://www.steelbeasts.com/topic/16238-is-t-72b3-damaging-model-at-close-ranges-accurate-compare-to-sb-wiki/
34. Steelbeasts.com forum, "Damage assessment" (index; 403), undated. https://www.steelbeasts.com/topic/15516-damage-assessment/
35. SBWiki, "Ammunition Data" (index), undated. https://www.steelbeasts.com/sbwiki/index.php/Ammunition_Data
36. Steam store, Gunner, HEAT, PC! description (index; connection reset on fetch). https://store.steampowered.com/app/1705180/Gunner_HEAT_PC/
37. Radian Simulations, Patreon release notes build 20200710, 2020-07 (index; 403). https://www.patreon.com/posts/39199366
38. GHPC mods setting `AmmoType` fields: Cyances/Super-M60 `AmmoArmor.cs` (last commit 2024-05-09) https://github.com/Cyances/Super-M60 ; thebeninator/M1A1Abrams `Ammo_120mm.cs` (2026-10-16) https://github.com/thebeninator/M1A1Abrams ; thebeninator/Pact-Increased-Lethality (2026-10-23) https://github.com/thebeninator/Pact-Increased-Lethality
39. Dami1189, "GHPC Combat Overhaul" README (third-party mod), last commit 2026-10-22. https://github.com/Dami1189/ghpc-combat-overhaul
40. SpaceBattles thread, post #78 reproducing Sprocket V0.1136 patch notes, 2022-02-19. https://forums.spacebattles.com/threads/sprocket-tank-design.964063/page-4
41. Steam discussions, Sprocket "New Shell type?" (community + developer reply; index; reset on fetch), undated. https://steamcommunity.com/app/1674170/discussions/0/4665175132463211104/
42. Steam announcements, Post Scriptum (app 736220), 2021 armoured update and later notes (index; reset on fetch). https://store.steampowered.com/news/posts/?feed=steam_community_announcements&appids=736220&enddate=1637838667 and …&enddate=1630411082
43. IL-2 forum, "Post Scriptum's new armoured update compared to il2 Tank Crew", 2021-11-22 (dates the PS update). https://forum.il2sturmovik.com/topic/75456-post-scriptums-new-armoured-update-compared-to-il2-tank-crew/
44. TechRaptor, "Squad Update v2.9 Adds 5 New Map Layers, Fixes Ammo Racks", 2021-06-28 (index; 403). https://techraptor.net/gaming/news/squad-update-v29-adds-5-new-map-layers-fixes-ammo-racks
45. Squad Fandom, "Vehicles" (index), undated. https://squad.fandom.com/wiki/Vehicles
46. ACE3 docs, `docs/wiki/feature/vehicledamage.md` (v3.14.0), last commit 2024-08-20. https://github.com/acemod/ACE3/blob/master/docs/wiki/feature/vehicledamage.md
47. ACE3 code, `addons/vehicle_damage/functions/fnc_processHit.sqf`, `fnc_handleDetonation.sqf`, `fnc_medicalDamage.sqf`; addon last commit 2026-08-03. https://github.com/acemod/ACE3/tree/master/addons/vehicle_damage/functions
48. BI Community Wiki, "Arma 3: Damage Description" / "Damage Enhancement" / "Tanks Config Guidelines" (index; 403), undated. https://community.bistudio.com/wiki/Arma_3_Damage_Description
49. Black Matter, "Developer Briefing #135 – Armour Overhaul" (Update 10, 2021-07-27; devtrackers mirror). https://devtrackers.gg/hell-let-loose/p/545e0a3f-developer-briefing-135-armour-overhaul
50. HLL "Dev Brief #218 – Update 20 Experimental Branch Test", 2026-05-21 (patched.gg mirror). https://patched.gg/games/hell-let-loose/dev-brief-218-update-20-experimental-branch-test
51. 1C Game Studios, "Dev blog #114", 2015-11-20. https://il2sturmovik.com/news/194/dev-blog-114/
52. 1C Game Studios, "Dev blog #222", 2019-04-30. https://il2sturmovik.com/news/472/dev-blog-222/
53. 1C Game Studios, "Dev blog #246", 2020-03-27. https://il2sturmovik.com/news/536/dev-blog-246
54. Obsidian/MY.GAMES, "Developer Diary – Shell Types" (Armored Warfare), 2015-10-29. https://armoredwarfare.com/en/node/2023
55. 1C Game Studios, "Dev blog #195" (index), undated here. https://il2sturmovik.com/news/402/dev-blog-195/
56. Battlefront forum, "How does the damage system work for vehicles" (2015 developer post; index; 403). https://community.battlefront.com/topic/118855-how-does-the-damage-system-work-for-vehicles/
57. Wargaming, "Update 1.13 release" (servers down 23 June; the post refers to Ranked 2021–2022, so 2021-06-23 is DERIVED; index). https://worldoftanks.com/news/updates/update-1-13-release/
