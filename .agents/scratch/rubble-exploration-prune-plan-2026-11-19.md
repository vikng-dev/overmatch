# Exploration code inventory and prune plan

Scope: the 7 active branches, all forked from the same commit 217e0662 (2026-11-18). `main` (ce01a9c4) is 10 commits ahead of that point, all docs. MEASURED means taken from git numstat or `du`. DERIVED means arithmetic on those.

How paths are sorted:
- **docs:** `.agents/`, `upstream/`, `*.md`
- **assets:** `assets/`
- **tools:** `scripts/`, `src/bin/`, `src/rubble_lab*`, `src/dev_capture*`, `*sandbox*`, the ruins `.house.ron` specs
- **tests:** `*tests.rs`, `headless_test/`
- **prod:** everything else. This includes `tools/src` (the asset bake) and `#[cfg(test)]` modules inside production files.

## 1. Size (MEASURED, against the fork point 217e0662)

| branch | ahead | prod | tests | tools | docs | assets | total | only here |
|---|---:|---|---|---|---|---|---|---|
| bed-drive | 14 | +2,971/−195 | +342/−84 | +183 | +141 | +169/−1 | +3,806/−280 | nothing: every other branch contains it |
| graded | 34 | +6,978/−407 | +966/−84 | +5,173 | +1,375 | +222/−1 | +14,714/−492 | ploughing law (3 commits) |
| staged | 31 | +4,078/−356 | +998/−154 | +3,934 | +2,098 | +8,562/−961 | +19,670/−1,471 | staged breakdown (5), ruins/permanent core/freeze fix (4) |
| lab | 46 | +8,880/−1,259 | +1,186/−285 | +6,073/−57 | +2,479/−16 | +266/−7 | +18,884/−1,624 | look pass 2 (4) |
| support | 44 | +8,170/−1,253 | +1,179/−285 | +6,178/−57 | +2,902/−16 | +252/−3 | +18,681/−1,614 | fill-support law (2) |
| fracture | 37 | +7,114/−1,266 | +736/−299 | +4,208/−57 | +1,607/−16 | +3,504/−4,305 | +17,169/−5,943 | natural fracture (8, the last at 23:09) |
| pieces | 35 | +6,942/−1,247 | +667/−297 | +4,208/−57 | +1,607/−16 | +3,504/−4,305 | +16,928/−5,922 | nothing committed (see below) |

**Pieces branch:** its tip dd1c80d7 is already inside the fracture branch. Its own work is uncommitted in bay-3: +1,329/−3,766 against the tip, measured by plain `diff`.

**How the branches nest:**
- bed-drive (14 commits) is the base of every branch.
- Lab passes 1–2 (6 commits plus a merge) come next.
- graded, lab and support add the graded arm, the coarse arm, three bed arms and the exact-support change. graded alone adds ploughing.
- lab, support, fracture and pieces add cruise, the old belt-fix copy, and wall arms A and B. lab and support also add the tracks-only fill contact.
- staged merged only the pinned lab dc8677e2.
- The union is 65 regular commits plus 4 merges: **+42,681/−10,068** (MEASURED sum of per-commit numstat).
- Six leaf tips hold every commit: graded, staged, lab, support, fracture, and pieces once it commits.

**Other branches:**
- 22 other `lane/rubble*` refs (the 2026-11-07/08 lanes, `rubble-bed-fair`, `rubble-bed-probe`) are 0 ahead of main.
- `rubble-climb` (1 commit, 870 test lines) is already on main as 88f90274 (`git cherry`).

**Older belt-fix copy:** 234513f2 and f5d18742 are on lab, support, fracture and pieces. This is round 0, whose patch-id differs from 1c9ca756 on `lane/belt-finite-inertia`. QUEUE says round 0 alone must not land.

## 2. Components

Line counts are MEASURED sums of each component's commits.

| # | component | where | lines | depends on | verdict |
|---|---|---|---|---|---|
| C1 | Shared height-map bed: `BedTile` wire type (rev 108), deposit/flow/bearing, readers in terrain_grid, track contact, spawns, rounds and decor seating; deletes main's welds (`merge_fragments`) | c77992c3…142a1519, all branches | +2,354/−336 | main | DELETE (welds: §4.1) |
| C2 | Graded arm (`Solids`, rev 109) | 707195dd, df956a26 (graded/lab/support) | +1,369/−247 | C1 | DELETE |
| C3 | Coarse arm (`CoarseTile`, relief, rev 110) | bad4126b, 100682d3, 8437efcf | +2,936/−302 | C1 | DELETE (its wire saving serves a rejected premise) |
| C4 | Three bed arms in the lab: a frozen plain-bed copy `rubble_lab/arms/plain` (1,043 lines), `Arm` enum, key G, jolt tracing | 13edf7bf, 4c8eee3d, a937702e | +1,677/−88 | C1–C3 | DELETE; carry the jolt-peak metric into the lab rebuild (step 1 of §5) |
| C5 | Exact support geometry: removes main's per-column terrain facet plane | 6b97f73d (graded/lab/support) | +163/−255 | C1 | UNDECIDED: does main's terrain, with no bed, show phantom support? If not, DELETE |
| C6 | Ploughing law | c124b58c, 1f6b8860, f6a59739 (graded) | +2,417/−639 | C1–C3 | DELETE (its builder rejected it) |
| C7 | Tracks-only fill contact (`touch.rs`, `ride.rs`) | 72685fbc, 4d75581c (lab/support) | +885/−146 | C1 | DELETE |
| C8 | Fill-support law, `fill_support.ron` | d2855118, ed3362b3 (support) | +1,485/−234 | C1, C7 | DELETE (premise rejected; heavy-feel bar failed) |
| C9 | Bed look 1: facets, masonry shader, chunk skin, fringe, settlement powder, dev_capture rubble | 518c9bef…9922d813 | +1,669/−161 | C1 | DELETE |
| C10 | Bed look 2: arrival, imprint, vehicle occlusion and cutout shader | 349d5cf6, de3b7c10, 3420615f, 438c115d (lab) | +1,591/−147 | C1, C9 | DELETE (the occlusion hides a 0.61 m hull overlap, per QUEUE) |
| C11 | Lab: `rubble_sandbox` bin, `src/rubble_lab/*`, `scripts/rubble-lab`, the today-build shims in `rubble-lab-support/`, `assets/dev/rubble_*.ron` | 379f5e54…dc8677e2, plus lab files inside later commits | +3,813/−193 own; +6,073 tools at the lab tip | C1 (bed refs: `metrics.rs` 19, `inspection.rs` 5, `capture.rs` 4) | TOOL; land a trimmed core (§5 step 1) |
| C12 | Cruise as desired speed: driver, transmission, HUD, `notch_count` 3→4, `drag_throttle_release` removed, rev 109 | a3b759eb, eed74910, 28bc7917, f9375d1f | +1,271/−250 (prod+tests +346/−225; lab `cruise.rs` 372; docs 515) | the belt fix | KEEP-CANDIDATE |
| C13 | Belt-fix copy (round 0) | 234513f2, f5d18742 | +1,745/−546 | — | DELETE here; `lane/belt-finite-inertia` owns it |
| C14 | Wall arm A: burst ring thrown from the filler centre, plus the lab start fix | 3d9a290b | +142/−23 (prod +24/−12) | — | KEEP-CANDIDATE |
| C15 | Wall arm B: impulsive joint verdict (`fracture/impulse.rs`) replaces the pulse column | 0798fd5a | +640/−347 (prod +429/−259) | the welds decision | KEEP-CANDIDATE |
| C16 | Cutter natural fracture: seeded cuts (tools), break-face colour, `UnitBond` (FORMAT 9), unit view (`units.rs` 1,170, `loose.rs` 141), brick picture, cracks, house_1 rebake | 77d5b63e…8f38ab09 (fracture; pieces has the first 6) | +6,283/−4,843 (prod ~+2,420/−430; rebake +3,345/−4,360) | `decor.rs` with bed seating | KEEP-CANDIDATE |
| C17 | Staged breakdown: grain-resolved seams, silhouette size floor 0.7 m, occupied-section spacing, house/fir rebake | 90b8e019, 6361b066, c6cb3124, f83c50a9, 4dc546fa (staged) | +3,756/−1,126 | the cutter (same file as C16) | UNDECIDED: decided by the size-line measure (§5 step 9) and by whether long timber snapping is noticed in play |
| C18 | Permanent core: `Element::permanent` (FORMAT 9), refusal check, breakage path, generator cross walls and roof span | 136fc264 (staged) | ~+470 hand-written of +7,395 | — | KEEP-CANDIDATE (owner principle 5; only cored buildings left ruins) |
| C19 | Ruins fixtures `ruins_core`/`ruins_hall` (6,287 baked lines), lab activity and park metrics | 136fc264, a2b6c3f7, 5bbd03a5 | +6,287 assets; ~490 lab | C11, C18 | TOOL (keep `ruins_core` only if a map or the lab uses it) |
| C20 | Freeze reach honours collision masks | b7e98d00, `freeze.rs` | +69/−9 | — | KEEP-CANDIDATE (main has query-only colliders with no contact filter at `breakage.rs:3144`) |
| C21 | Client-owned small pieces: a separate client Avian world in `decor.rs` (+955/−413), private crumble and split, `local_pieces.ron`, a 50-line bed stub; deletes the plain bed | uncommitted in bay-3 | +1,329/−3,766 | C16 units; built without welds | UNDECIDED (being tried): decided by its report's "heap that is not there", late join and client cost |
| C22 | Revision bumps: two meanings each for rev 109 and 110, two different FORMAT 9s (§4.4–4.5) | — | — | — | DELETE all; the rebuild assigns fresh numbers |
| C23 | Experiment reports (~3,970 lines, largest version of each file) | all branches | docs | — | stay on the archive tags (§5) |

## 3. Totals

- **Union committed:** +42,681/−10,068 (MEASURED).
- **DELETE outright** (C1–C4, C6–C10, C13): **+18,128** lines, of which +11,044 production (MEASURED).
- **TOOL:** C11 +3,813 in its own commits (~6,000 at the tip) and C19 fixtures +6,287.
- **UNDECIDED:** C17 +3,756, C5 +163, C21 work in progress +1,329.
- **KEEP-CANDIDATE, as committed:** ~8,875 gross. Hand-written production plus tests is ~3,970 (DERIVED: cruise 346, A 26, B 440, fracture ~2,620, core ~470, freeze 69).

**Clean adoption estimate (DERIVED): about 2,000–5,000 hand-written lines, central ~3,800.**
- Wall A: ~45, the design's own figure.
- Wall B with the design's change 3 (welds that read separation speed): ~500 added, ~260 removed. Change 3 alone is ~80 lines.
- Natural fracture: ~750 at the design's size up to ~2,600 as built. The central figure takes ~1,500, because the bed-seat workarounds (including 8f38ab09) fall away, and `units.rs` alone is about 4 times what the design estimated.
- Permanent core: ~300–470.
- Freeze fix: ~70.
- Cruise: ~350 added, ~230 removed.
- Pieces, if adopted: ~1,000. Its bay-3 `decor.rs` work is +955/−413; its bed deletions do not apply to main.
- Not counted above: the regenerated bake output and the trimmed lab (~2,300 lines, dev-only).

That is about 9% of the committed union (15% with the lab).

## 4. Entanglement

1. **Welds.** 0a9ad290 (inside C1) deletes main's `merge_fragments`. Wall B, fracture and the pieces work were all built and measured with no welds: the wall report says so, and bay-3 does not restore them (grep). On main, wall B needs either change 3 or deleting the welds.
   - I decided: change 3.
   - Reason (DERIVED from main's crumble routing): welds are what keep thrown matter below the size floor as server lumps that lift roadwheels, and the owner likes that oscillation.
   - Roadwheel travel was never measured on any branch.
2. **`decor.rs` is edited three ways on sibling branches.**
   - Bed seating from 1995553c reads `rubble_bed::BedSurface`.
   - Fracture's unit sheds (5b0a907b, and 8f38ab09, which only makes unit drops independent of bed history).
   - Bay-3's client-world rewrite.
   - Bay-1's uncommitted work touches it too. Loose units must be rewritten on main's `decor.rs`, not lifted.
3. **`run_laws` in `breakage.rs`.** Wall B's signature change sits on the weld-less tree.
4. **Two asset FORMAT 9s.** Permanent core and `UnitBond` each bump to 9 independently. There are also two independent house_1 rebakes (f83c50a9, 458ab4e2) through the same `tools/src/construction/fracture.rs`. Rebuild with one bump and one rebake from the final tools.
5. **Protocol revision numbers.**
   - 108: `BedTile`. 109: graded `Solids`, and separately the cruise `Command` meaning. 110: `CoarseTile`, and separately bay-3's removal of `BedTile`.
   - Wall A/B, natural fracture and ruins changed no wire type. The "rev 109" QUEUE gives wall and fracture comes from cruise.
   - Bay-3's wire hashes equal main's (`0xb35d6c7c…`, `0xb12be3bc…`).
   - On main, only cruise needs a bump (107→108).
6. **Lab and bed.** The lab reads the bed in many places (§2 C11). Bay-3 keeps a 50-line `rubble_bed.rs` stub ("empty read-only inspection vocabulary") only so the lab compiles. It is a shim with no game user.
7. **Belt.** Cruise's notch-2 failure and every jolt or feel number on lab, support, fracture and pieces were taken with the round-0 belt copy, or before it. All of them need re-measuring on main with the final belt fix.
8. **Size floor.** staged redefines `server_rubble_min_size_m` from cube root of volume at 0.5 to silhouette at 0.7, and the QUEUE header says 0.7. The owner said 0.5 tonight. The pieces chain uses main's cube-root 0.5.
9. **Invalid timings.** Low power mode (18:00–20:40) makes the ms figures from look pass 2, the start of the support run and the early fracture pass-2 captures unusable (per QUEUE).

**Switches the exploration introduced.** None has a player or operator user, so all go except those noted:
- **Env vars, lab:** `RUBBLE_LAB_{SCENARIO,OUT,CLIPS,INSPECT,SMOKE,AIM_SHARE,ROUND,ARM,ENTRY_SPEED}`.
- **Env vars, staged:** `RUBBLE_LAB_{CONSTRUCTION,APPROACH}`.
- **Env vars, test-only:** `GRADED_TAG` (`bed_probe.rs:265`) and `STAGED_ARM` (`staged.rs:242`). Their only users are hand-typed commands in reports.
- **Arms and keys:** lab `Arm {Plain, Graded, Coarse}` with `run_if` gates; key G cycles arms; key B fires at the wall.
- **Script flags:** `--arm --today --touch --entry-speed --round --aim-share --clips --contact-sheets --inspect --binary --construction --approach`, plus the `today-build` and `smoke` subcommands.
- **Build-level:** `[[bin]] rubble_sandbox` (requires `dev_ui`), `#[cfg(dev_ui)] pub mod rubble_lab`, `#[cfg(dev_ui)] Frozen::island()`, `dev_capture::offline_rubble_plugin`, `scripts/capture/rubble-look.ron`.
- **Data:** the `rubble_bed` block in `physical_laws.ron`, `fill_support.ron`, 52 lines of bed tastes in `presentation.ron`, `rubble_lab.ron`, `rubble_plain.ron`, `drive.ron` notch_count, the `engine.ron` field removal, `local_pieces.ron`.
- **Survivors:** the trimmed lab's own inputs, with `scripts/rubble-lab` as their user, and `notch_count` with cruise.

## 5. Prune plan

Each step is its own lane from `main`, passes the six lenses, and writes a fresh report. Nothing is lifted from a branch.

0. **Precondition:** the belt fix lands from `lane/belt-finite-inertia`. Never lift 234513f2.
1. **Trimmed lab** (the one TOOL that lands). Keep: sandbox, the passes/ram/burst scenarios, capture, labelled two-panel clips, the wall observer, jolt peaks, piece counts. Drop: arms, bed readers and the today-build shims; comparisons become main against main. The core files measure ~2,300 lines at the fracture tip, before trimming.
2. **Freeze fix (C20).** Write the regression on main first; drop the fix if the test does not fail.
3. **Wall arm A (C14).** Re-measure the armour recorded scenes (report: 14 of 16 unchanged, ring doses in s13 and s16 move).
4. **Wall arm B with change 3 (C15).** Re-measure on main, with welds: opening m², chunks lost and flying, speeds, crack reach (5.19 m, open), burst tick, p99, live server bodies.
5. **Natural fracture, as the design's three stages:**
   - (a) cutter and rebake, still FORMAT 8;
   - (b) break-face colour;
   - (c) unit view, FORMAT 9, on main's `decor.rs`.
   - Re-measure counts, bytes, triangles, frame ms (low power mode) and the hole-edge clips.
6. **Permanent core (C18).** Shares FORMAT 9 if it lands beside 5(c), otherwise takes 10. Re-measure the park tick on a cored building (24–43 ms reported, against a 4.8–9.7 ms baseline).
7. **Cruise (C12), rev 108.** Re-measure creep over a pile, notch 1 and 2 restarts (3 of 3), fast-pass reversals and crossing time against main.
8. **Client-owned pieces (C21)**, only if the owner adopts them, and after steps 4 and 5(c). Re-measure everything: its evidence had no welds and carried the bed.
9. **C17 items**, decided by step 8's size line.

**Docs:** the 2026-11-18/19 design docs on main stay as the prose record of rejected alternatives. One disposition doc (this plan) names each DELETE, why it lost, and its archive tag.

**Branches (the orchestrator decides; I only recommend):**
- Tag the six leaf tips: `archive/rubble-2026-11-19/{graded,staged,lab,support,fracture,pieces}`. Then delete those branches and bed-drive (7 refs).
- The 22 already-merged refs and `rubble-climb` can be deleted now.

**Artifacts:**
- `.agents/scratch/rubble-explore` is 2.8 GB (MEASURED), untracked and not ignored in the main checkout, so a broad `git add` would stage it. Move it outside the repo and keep the curated clips and reports.
- Bay `target/` subfolders hold about 108 GB (MEASURED; belt-round3 and generic capture excluded): rubble-pass2 32.9, rubble-ruins 30.7, rubble-wall 28.2, graded 4.5, look-2 3.1, touch 2.9, coarse 2.5 and smaller ones. Delete the subfolders, never `target/` itself, once the owner has the clips.

## 6. Stop tonight

- **QUEUE "Next" items that only add to the heap:** 1 (merge graded and staged into lab and rerun the sets), 3 (coarse-bed branch), 4 (more look passes), 6 (baked ruins with a coarse bed) and 10 (full cruise on the exploration branch). Stop them.
- **Fracture pass 2 (bay-1):** finish its report, then no pass 3 on this branch. Its newest commit, 8f38ab09, already exists only to work around the bed.
- **Pieces (bay-3):** finish to its report. Do not grow the lab arm, the bed stub or the late-join scatter beyond the brief, and no pieces pass 2 on this branch.
- **Captures:** stop refreshing `play/` from exploration branches, and stop writing captures into bay `target/` beyond what each report needs.
- **Law A:** when a bay frees, start it from `main` as planned, not from an exploration branch.
- **Belt round 3:** unaffected; let it run.

## 7. What I could not verify

- **Uncommitted bay work** was measured with plain `diff` of files changed after each branch's last commit. I ran no `git status` in the bays, per AGENTS.md, so earlier uncommitted edits could be missed.
- **Report numbers** (opening m², jolts, ms, wire) are the reports' claims. I built and ran nothing and viewed no clips. I checked only the commits and files the reports name, which is how I found the rev-109 misattribution.
- **Two open questions on main:** whether the freeze-mask defect actually reproduces (only its precondition is confirmed), and whether main's terrain shows phantom support (C5).
- **Builder logs:** all three running builders' logs show auth-refresh errors from their second line, but the logs keep growing with build activity. I did not establish whether the errors matter.
- **Counting rules:** test modules inside production files and `tools/src` are counted as production. Components are mapped by commit, and some commits mix components (136fc264, 0798fd5a, 234513f2).
- **Adoption and lab-trim sizes** are DERIVED, not rebuilt.

**Decisions I made without asking:**
- Wall arm B is rebuilt with the design's change 3 (welds kept and gated by separation) rather than deleting welds.
- A trimmed lab lands first as the one tool on main.
- Cruise is a keep-candidate, rebuilt after the belt fix.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/decor.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/schema.rs
- /Users/Yan/github/vikng-dev/overmatch/tools/src/construction/fracture.rs
