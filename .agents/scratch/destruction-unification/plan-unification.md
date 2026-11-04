Subject: the video game's environment destruction model (walls, roofs and trees). Game state only. Crew are HP pools at stations; use the approved terms if they come up: knocked out, out of action, HP pool emptied, reduced, potency, dose.

## Your job

The owner has decided that walls, roofs and trees become one destruction system, and that this runs ahead of the next game build. Bring it to **queue readiness**, which the owner defines as "research and planning is done, no open questions for human remain, and it's ready for implementation".

You read and plan. No edits, no builds, no web search. Your final message is the plan, in full; nothing else you write gets read.

## Read

Designs, all under `/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/design/`. Read them whole, rulings sections included; a later ruling overrides an earlier section:
- `generic-fracture-merged.md`: the system, its path (steps 0-12) and the delete list. This is the spine.
- `break-survival-merged.md`: survival is read on the solid, and its six lanes.
- `material-proxy-merged.md`: the material row, lanes P1-P6.
- `structures-fields-merged.md`: spall and burst fields. Provisional and after the first playtest. The plan leaves room for it and builds none of it.

Issues, comments included (`gh issue view N --comments`; owner directions live in comments): #106 and its sub-issues, #264, #268, #232, #256, #150, #188, #189, #119.

Evidence: `.agents/scratch/diag-tree-crash/report.md` and `evidence/`; `.agents/scratch/diag-pieces-rest/report.md`; `.agents/scratch/diag-collapse-cost/report.md`.

Code at HEAD: `src/buildings/fracture.rs`, `src/buildings/breakage.rs`, `src/trees.rs`, `src/trees/motion.rs`, `src/contact_transfer.rs`, `src/net/destruction.rs`, `src/track/rubble.rs`, `vendor/avian3d-0.7.0-child-pose`, plus whatever those lead to. Rules: `AGENTS.md`, `.agents/lenses/README.md`, `.agents/lenses/backlog.md`, `.agents/docs/adr/0014-sim-view-split.md`, `.agents/docs/adr/0034-tanks-are-data-no-per-tank-overrides.md`.

Never open `shape-law.md`, `tiger-ammo-history.md` or `burst-delay-history.md`.

## Rules for reading

- Prose here is a claim. The designs were written before today's landings, so check each code claim against source and cite `file:line` at HEAD. Label every number MEASURED or DERIVED.
- Code facts are not decisions. Where code contradicts a ruling, the ruling stands: name the code fact, and either plan the change or raise it as a question. Never restate a ruling to match the code.
- A moved result is neither a merit nor a cost. No task pins an emergent number; a moved number goes in its lane's report for the owner's eye.
- Verify any engine API you plan on against the pinned source (Bevy 0.19, avian3d 0.7, the vendored copy above).

## Settled: plan on these, don't re-ask them

- Baked cuts, with the stiffness solve opening only if a wrong hold shows in play. No editor: the bake is a command, in the separate asset tooling package, never shipped.
- Survival reads on the solid. A hit opens part of a slice through the thing, and it stands while what is left holds its load. The cut only decides which faces open and what falls.
- Trees lose their hit-point number. Their exceptions are deleted, not wrapped: `segment_hp`, `hp_per_m2`, `fell_on_pool_emptied`, `load_trunk`'s missing strike, `TrunkLoads`, `structure_of`, the 15 equal slices, `TreeState::Down` with `Fall` and `Swap`, `moving_near`'s trunk exclusion, one wood strength at every angle.
- Roofs change. No moisture variable.
- Default picks until the owner judges the first bake by eye: about 5 shots to fell a fir, repeating a hole adds nothing, one wood row until a second ships.
- It is the whole of `generic-fracture-merged.md`, not trees alone. The stopgap for the belt reader's panic is dropped; that panic goes when trees move over.

## Folded in

Each of these needs a home in the plan, or a stated reason it has none.

1. **The tree crash.** `src/track/rubble.rs:252` panics on a freed tree piece's compound collider (`launch_range`, `src/trees/motion.rs:105`). The owner's real drive into a tree must survive at cutover: carry the report's witness and its `reproduce.py` drive into that task's proof. State the earliest task after which that drive survives, and whether a different order reaches it sooner.
2. **How hard a blow counts.** `Load::mean_force` (`fracture.rs:947`) divides a blow by the physics step, with no contact duration. See #256, including its handbook checks.
3. **The building slowdown.** Fallen pieces wake each other (Avian's new-contact path resets sleeping islands' timers), and awake contacts drive `apply_deposits` → `run_laws` → `fracture::weigh`. Two candidates: a guard on contact waking in the vendored narrow phase, then caching or reusing the construction graph. Trees would inherit this once they move over. The owner prefers configuring the library's own mechanism before hand-rolling a parallel rule. Automatic settling with the guard alone is unmeasured, so say how to measure it. `scripts/perf/collapse.sh` is the existing test.
4. **Ramming seen in a real match.** A body's contact already loads structures through generic code (`src/contact_transfer.rs`), and it has never been watched in a real match against a building. Say which task proves ramming a tree and ramming a building end to end, through the production server and client.
5. **The design's two added gaps:** one allocation rule for a line's work among chunks, and a field stop that names its chunk. Decide whether the second is needed before the first playtest, given that fields come after it.

## Produce

**1. A verdict.** Is it ready, and if not, what stands between it and ready? Then a few lines on how far the designs have drifted from the code.

**2. An accounting.** Every step 0-12, every break-survival lane, every lane P1-P6, every delete-list item and every folded-in item appears exactly once, as: a task; merged into a named task; already landed (give the commit); or dropped (give the reason). Also say what each open issue listed above becomes: closed by a named task, or left open and why. Check `.agents/lenses/backlog.md` for rows naming the files these tasks edit, since a lane must close those rows.

**3. The ordered task list.** One merged sequence, ready to file as GitHub sub-issues, each sized to one agent session of under 90 minutes (split anything larger). For each:

```
### T<n>. <plain-language title>
Does: two to four sentences.
Absorbs: design steps, lanes and issues.
Files: file:line at HEAD.
Deletes: what goes in this task, with file:line. The losing path goes in the same change; no flags.
Proof: the invariant each test names and who relies on it, the mutant it fails on, and any measurement. Laws and sourced facts only, no emergent number pinned.
Wire and data: PROTOCOL_REV bump, bake format, asset or LFS change, or none.
Builder: Codex, or Claude where it needs 3D-space reasoning or a visual judgement, with the reason.
Blocked by: T<n>. Conflicts with: T<n> (shared files), so the three lanes can be scheduled.
Player sees: what visibly changes.
Eye gate: what the owner looks at, if anything.
```

Then:
- which tasks can run at once across three lanes;
- where the owner's eye gates fall, and what each can change downstream;
- a DERIVED estimate of lane count and elapsed time, with its basis;
- what state `main` is in after each landing. Walls must keep working throughout. Say where the game is playable, and name the earliest task after which the next game build could be cut if one had to be.

**4. Risks and kill criteria.** Which of the designs' "what would prove it wrong" items still stand, which task tests each, and what happens to the later tasks if one trips. Put the cheapest test of the riskiest assumption first, and say why it's first.

**5. Questions for the owner.** Only calls no one else can make. Decide everything that code, laws, the rulings or a stated default can settle, and say so in one line. For each real question: plain words with no code names, what it blocks, your recommended answer, and what that answer costs. Classify these held items, with a reason each:
- metal stays rigid;
- the word "structure": the owner proposed it for any one-material piece fractured into smaller bits (a wall, a roof, a trunk), so check what `Structure` means in code today and what the renames in step 2 would do;
- whether a hole stays drawn only, given that survival now reads on the solid (see the limit in `generic-fracture-merged.md`);
- rounds on edges;
- rebuilding destruction state at spawn;
- the stored results file (backlog row ballistics-28);
- material row calls 1-5, some of which `break-survival-merged.md` overtook;
- anything else you find.

An owner's eye gate during the work is not an open question. List it under eye gates.

Keep it under 300 lines. Say each thing once. Don't re-tell the designs; point at them.

Do not stop to ask questions: decide, and state your decision in the final message.
