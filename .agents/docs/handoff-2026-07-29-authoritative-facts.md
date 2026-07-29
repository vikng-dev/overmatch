# Handoff — authoritative-facts arc + simplifier experiment

**Written 2026-07-29, header current as of slice 3.10.** State as of commit `a0ac961` on
`feat/authoritative-facts`.
Read this if the session ended mid-arc. It is written for whoever picks it up, human or agent.

---

## 1. What this arc is

The server computes a hull impulse when a shell hits you (Δv = 0.1383 m/s for an 88mm). The client
never felt it. `ROLLBACK_VELOCITY = 1.0` m/s — the state-difference gate that decides whether an
authoritative value is worth adopting — is **7.2× larger than the impulse itself**, so that gate
could never once have passed a real hit.

The fix is not a smaller threshold. It is a separation that every shipped engine makes and this
codebase had not: **adopting authority is unconditional and server-authored; hiding the seam is
thresholded and view-only.** Being shot is a knowledge question, not a cost question. See
`.agents/docs/adr/0032-unpredictable-authoritative-facts-adopt-unconditionally.md` — that ADR is the
precedent document, and the next fact of this kind (a ram, a blast, a crew knockout, a track break)
should be implementable by reading it.

The primitive is `src/net/adoption.rs`. `HullShock` is merely its first consumer.

---

## 2. Where things stand

### `feat/authoritative-facts` — 16 commits, HEAD `a0ac961`, **690 lib tests passing**

```
a0ac961 revalidate the WHOLE rigid body at the request, not just the shove — wire unchanged at REV 24
baade0b revalidate delivery at the REQUEST, not at the offer — wire unchanged at REV 24
6b902eb docs: handoff — round 5 verdict, the determinacy argument is false
906ff79 docs: handoff note for the authoritative-facts arc
4f523b1 delivery is ESTABLISHED before a rollback is asked for — wire unchanged at REV 24
2a2e282 the episode carries its own span, and `bypassed` is established — wire REV 24
8c33217 correlate the shove with its spark by IDENTITY — wire REV 23
88a7e29 docs(adr): 0032
6cf5553 the shove ordering rule is best-effort, and now says so
a0c831e the shove waits for its spark, bounded and instrumented
aa042ba generic unconditional adoption path
d8ff806 make the lead-0 delivery failure executable
4c13ffe docs(upstream): lightyear report — LOCAL DRAFT, NEVER FILED
8a2fe21 HullShock as built — correct shape, wrong delivery path
```

Wire is at **`PROTOCOL_REV = 24`**. Hashes, independently recomputed by review:
surface `0xf321_3c48_61b3_bfea`, types `0x268a_d4fb_e297_639b`, manifest `0x13cd_6a8e_562f_0315`.

### `chore/simplify` — 8 commits, a separate experiment (§6)

---

## 3. The review history — read this before trusting anything

Codex has reviewed this arc **six times** and returned DO NOT SHIP **five times**. Every round found
that the previous fix read as complete and was not — including the two rounds that were themselves
fixing a partial fix. **Twice a newly added test asserted the defect as success, and twice a newly
added test claimed coverage it did not have.**

| Round | On | Verdict | What it found |
|---|---|---|---|
| 1 | `a0c831e` | DO NOT SHIP | Ordering claimed a guarantee it cannot keep; wait clock measured server-fact age including transit |
| 2 | `6cf5553` | DO NOT SHIP | Episode correlation swapped one invalid rule for another; `retirement` blind to rollback kind |
| 3 | `8c33217` | DO NOT SHIP | Disjointness held only while `last_bump_tick` was `Some` — a fresh hull's first episode claimed 15 ticks it never held, and a prior life's spark could release a respawned hull |
| 4 | `2a2e282` | DO NOT SHIP | Correlation **confirmed settled**; `Adopted` still retired facts whose restore carried nothing |
| 5 | `4f523b1` | DO NOT SHIP | The determinacy argument is **false** — confirmed history is not append-only, and the predicate is proven at staging but never revalidated at the request. Fixed by slice 3.9 (below); round 5 passed C, E, F, G, H |
| 6 | `baade0b` | DO NOT SHIP (1 High, 2 Low) | The revalidation covered **half the predicate**: only the velocities, while the offer proved all four rigid-body histories, so a late `Position` removal reached a claimed rollback that deleted the component and closed as `Adopted`. Plus two overstated coverage claims in the new tests and a false comment. Fixed by slice 3.10 (below); round 6 passed A, B, C, D, E, H |

**The operational lesson, and it is the important part of this document: a green test suite has
never once caught one of these.** The tests were written by the same reasoning that produced the
bug. What caught them, every time, was Codex reading our code against lightyear's actual source in
`~/.cargo/registry/.../lightyear_prediction-0.28.0/`. Do not report "N tests passing" as evidence on
this work. Keep the adversarial review in the loop until a round comes back clean.

**The second lesson, which rounds 5 and 6 taught together: a fix aimed at one instance of a defect
tends to land on exactly that instance.** Round 5 found readiness proven at staging and acted on
later; the fix revalidated at the request — for the two components the finding named. Round 6 found
the other two still stale. When a review names a defect, ask what CLASS it belongs to and enumerate
the members before writing the fix. Round 6's Low findings are the same shape at test level: the
fixture exercised the mechanism the finding named and quietly did not exercise the one it claimed to.

### What is now settled (round 4 verified, do not re-litigate)

- The single same-tick `arm` path; `close_episode` runs after `ProjectileMarch` in the same tick.
- Episode spans are disjoint **by construction** — publication clears `opened_at`, so the next
  span must start strictly later. No appeal to `SHOCK_EPISODE_TICKS` arithmetic.
- Respawn is structurally safe, including a late-arriving prior-life `ImpactConfirm` (it keeps its
  older server tick and so cannot drift into the new span).
- The lead-zero fixture matches the four components production actually predicts: `Position`,
  `Rotation`, `LinearVelocity`, `AngularVelocity`.
- All wire accounting.

### What round 5 found, and what slice 3.9 did about it (`baade0b`)

`4f523b1` strengthened readiness instead of detecting failure after the fact, on this argument:
at offer time, whether `prepare_rollback` will carry the shove is **already fully determined** and
cannot change while `produced_at` is fixed, because `get_state_at_or_before(produced_at)` only
resolves samples ≤ `produced_at` and confirmed history only grows forward. So retrying is not merely
unbounded — it is futile.

**Round 5 confirmed that argument is FALSE**, on two independent grounds, and slice 3.9 landed
against it:

1. **Confirmed history is not append-only.** `ConfirmedHistory::insert_raw` does sorted *middle*
   insertion with same-tick replacement; `SameAsPrecedent` explicitly changes its effective value
   when a late *preceding* sample arrives (lightyear ships a test for it at
   `lightyear_core-0.28.0/src/confirmed_history.rs:648`); and replicon mutation transport is
   unordered, with history-enabled entities accepting older mutations.
2. **The control flow never revalidates.** The predicate gates the *offer*; an `ExternalEvent` fact
   then stays staged for the visual budget, often into later frames; a later offer pass that fails
   readiness merely `continue`s and leaves the staged fact alone; and `request_staged_adoption`
   consumes it and claims the rollback without re-running the predicate. The same-frame premise is
   also literally false — lightyear's `Check` path calls `ConfirmedHistory::add_unchanged` for all
   four relevant types between the gate and `Prepare`.

   *One correction to that last sentence, found while fixing it and worth keeping:* `add_unchanged`
   is reached only from `check_rollback`'s policy branch, and `check_rollback` consumes a pending
   forced-rollback request BEFORE that branch and then skips it entirely. So on the frame this module
   claims, it cannot run. It runs on the frames the fact spends WAITING — which is the whole window
   the finding is about, so the finding stands; the "between the gate and `Prepare`" framing does not.

   *A correction round 6 made to my own round-5 reasoning, and the more useful of the two:* I argued
   that `insert_raw`'s same-tick REPLACEMENT could not change an already-stored value, because two
   different authoritative values cannot map to one lightyear tick. **That is not true, and it is not
   the proof.** Replicon checkpoints and lightyear ticks are not in bijection — several checkpoints
   can land on one tick, and each would replace the entry the previous one wrote. Production is safe
   for an entirely different reason: the hull values in question are authored by the fixed-step
   simulation, which produces one final value per lightyear tick, so the replacement is idempotent in
   fact rather than by construction. Keep the real reason. An invariant that holds because of who
   WRITES the data is a different, weaker thing from one that holds because the type cannot express
   the violation, and only the second survives a change of writer.

#### The fix

**The predicate is now asked at the request transaction**, in `request_staged_adoption`, over the
histories as they are at that moment, immediately before the slot claim. The offer's identical gate
stays, demoted in writing to an ECONOMY: one staging slot, don't fill it with an undeliverable fact.

**A failed revalidation is a WAIT, not a drop.** Nothing is claimed, nothing is tallied, the fact
stays staged. The bound is the replay-window check that already runs first in the same function:
once the fact's age passes `RollbackPolicy::max_rollback_ticks` it is closed with a WARN naming it,
and the local tick advances at least once per tick — so the stall is bounded and the give-up is loud.

**A second, unbriefed defect fell out of the same reading.** `restore_carries_the_shove` was scanning
present values only, while `prepare_rollback` reads `get_state_at_or_before`, which resolves
authoritative REMOVALS. A late removal middle-inserted between the event and the restore target made
lightyear delete `LinearVelocity` from the hull while our predicate, seeing only the older sample the
removal shadowed, called it a delivery. The predicate now asks lightyear's own lookup FIRST and uses
the present-value scan only for the tick it resolved at; `authority_reaches` fails closed the same
way. A `SameAsPrecedent` entry still counts at its own tick, deliberately — the marker asserts the
authority still held that value there, so a restore resolving it installs the authority's real state.

**`Undelivered` is now unreachable against pinned lightyear 0.28**, on three steps, all in the
`retirement` doc: (1) the branch needs both `adoption.requested` and this module's own installed
claim, and the slot's claim is consumed every frame, so it is same-frame with the revalidation;
(2) between them, `net::watchdog` is read-only, `check_rollback` consumes the forced request and
skips the whole policy branch — the only writer of `ConfirmedHistory` reachable in that gap — and
replicon receive already ran; (3) the two lookups now agree by construction. Steps 2 and 3 are
DEPENDENCY properties. A lightyear bump can retire either without touching a line here, which is
exactly why the branch, its ERROR log and its counter all stay.

**The test that was missing for five rounds now exists.**
`net::lead_zero_rollback::a_late_replicated_change_is_revalidated_before_the_request` stages the fact
on the arrival frame, then moves the confirmed history through lightyear's own insertion API — a
newer sample (compressed to an unchanged marker) plus a removal stamped before it, middle-inserted —
and asserts the module waits, tallies nothing, and leaves the fact staged. Its sibling
`a_revalidation_that_never_passes_is_dropped_at_the_replay_window` carries the same run past the
window and asserts the fact is closed. Both were verified RED against the pre-3.9 code, where the
module claimed the slot on the staging-time answer and `prepare_rollback` deleted the hull's
`LinearVelocity` outright.

**Round 5 passed C, E, F, G and H** — the close tick is the right comparison tick, the real-restore
fixtures genuinely run lightyear's `Prepare`, the fixture helpers can no longer manufacture
impossible shapes, the state hash is right with both re-pins correct, and the wire is byte-identical
at REV 24 with `settled_at` correctly derived rather than transmitted. Do not re-litigate those.

### What round 6 found, and what slice 3.10 did about it (`a0ac961`)

Round 6 passed almost everything: the gap between revalidation and `slot.claim` is inert, the
predicate's lookups match `prepare_rollback` for present values / removals / markers / empty
histories, the narrow `Undelivered` unreachability proof holds, the age check runs before the failed
revalidation and drops at 101 for a 100-tick window, the fixtures use lightyear's real sorted API
with a genuinely middle-inserted removal, and the wire is clean at REV 24.

**The one High: the revalidation covered half the predicate.** The offer checks all four rigid-body
histories; `request_staged_adoption` re-checked only `LinearVelocity` and `AngularVelocity`. A late
authoritative REMOVAL of `Position` therefore survived — the offer pass correctly stopped offering
the hull and that changed nothing (re-offering never touches a staged fact), the velocity-only
revalidation passed, the slot was claimed, `prepare_rollback` answered the removal by taking
`Position` OFF the hull, and `confirm_forced_rollback` closed the fact as `Adopted` because both
velocities were genuinely carried. **Every counter this module keeps is defined on the velocities, so
nothing could fire.** Exactly the round-5 defect, still live for the other half.

Both sites now call one function, `restore_is_deliverable`, and its signature is what stops the
request's query shrinking again. **The two halves keep DIFFERENT predicates, deliberately:** the
velocities CARRY the fact (a `HullShock` episode is a velocity impulse and nothing else), so they get
the strong `restore_carries_the_shove`; the pose carries none of the event — the impulse changes
velocity, and the pose only moves as later ticks integrate it — so it keeps the weaker
`authority_reaches`, which asks only that the restore not delete the component or fall through to the
client's own prediction. One predicate for all four is wrong in both directions, and the second
direction is the non-obvious one: `restore_carries_the_shove` on the pose is strictly stronger, so
every verdict it changes turns a restorable pose into a WAIT — and replication transmits only
components that CHANGED, while the `SameAsPrecedent` markers that would date a stationary hull's pose
forward come from `check_rollback`'s policy branch, which our own forced request skips. A hull that
was standing still when it was shot would stall until the replay window dropped its shove.
`confirm_forced_rollback` deliberately stays velocity-only; it asks whether the dv is already live,
and refusing there would only re-request state that cannot change.

`a_late_pose_removal_is_revalidated_before_the_request` runs the real `Prepare` and was verified RED
on the deleted `Position` — a clean assertion, not a panic. `the_pose_is_asked_a_weaker_question_than_the_shove_and_that_is_deliberate`
pins the asymmetry, which the integration fixture cannot see: that fixture would pass just as happily
against a module that had tightened the pose to the velocity predicate.

**Two Lows, both about tests overstating coverage.** The `SameAsPrecedent` marker in the revalidation
fixtures sat at 106 while the restore target was 104, so the lookup never reached it and only the
removal at 102 was load-bearing — moved to 103, where `get_state_at_or_before(104)` lands ON the
marker and resolves back through it, with asserts in the fixture that keep both orderings true. And
the replay-window fixture observed the fact closed at age 101 but would have passed on a give-up
anywhere from 17 to 100; it now runs twice, one tick apart, and asserts the ages — staged at exactly
100, closed at exactly 101.

**One literally-false claim, corrected in both the `retirement` doc and ADR-0032.**
`check_rollback`'s policy branch is not "the only caller of `ConfirmedHistory::add_unchanged`":
`push_unchanged` calls it too, and lightyear's interpolation invokes that path
(`lightyear_interpolation-0.28.0/src/plugin.rs`, `Update`, `With<Interpolated>`). The proof survives
for a reason unrelated to call counts — that path runs in `Update` on `Interpolated` entities, while
the gap is inside one `PreUpdate` on a `Predicted` hull — and that is now the wording.

---

## 4. Running at handoff

Nothing. Round 6's findings are all fixed in `a0ac961`; no job is in flight, the branch is clean, no
agent holds a lock, and the simplifier is idle.

---

## 5. What is next, in order

1. **Send slice 3.10 to review as round 7.** File it the way rounds 3.5–3.10 were filed: the review's
   own file:line citations, and an explicit instruction to say where the brief is wrong against the
   code — that instruction has caught a real error of mine in four of six rounds, and rounds 3.9 and
   3.10 each corrected one of the previous review's own claims (see the two notes under ground 2
   above). Round 7's job is to check whether 3.10's fix is, once again, only the instance and not the
   class: the specific question to put to it is whether any OTHER readiness or delivery fact in this
   module is established at one point and acted on at another.
2. **Slice 4 — `render_error`** (task #32, held all session). Fix the one-frame render freeze per
   rollback, and honour the `AdoptionCause` tag so an adopted authority impulse stays sharp instead
   of being smoothed like a misprediction. `AdoptionCause` currently has **no consumer** —
   `ForcedRollbackSlot::installed()` is read only inside `net::adoption`. Held deliberately: I did
   not want presentation logic built on an adoption path that kept changing.
3. **#36 — the native-comparator bypass.** `HullShock` still carries a
   `with_rollback_condition(..)`, a second delivery path that ping influences. Originally scoped
   around "don't disturb the registration"; that constraint was imaginary (see §7) so re-scope it
   rather than inheriting the shape.
4. **#34 — measure what is only derived.** The ~10-tick catch-up, ~156 ms and <120 m dead zone are
   DERIVED and were never measured; before ADR-0032 they existed nowhere in the repo at all. Also
   queued: `jitter_margin` 1.0→4.0, quantize, predict the impact.
5. **#29 — three dangling identifiers** found by the doc gate. Ground truth established:
   `apply_tank_spec` → `tank::spawn_complete_tank`; `net::client::focus_menu` →
   `overlay::focus_declare` (verified by the actual call to `collapse_focus`). **`PUFF_CAP` is not a
   rename** — the puffs no longer have their own cap, they share `BILLBOARD_CAP`, the very constant
   whose doc cites it, so that sentence needs a rewrite. Also decide whether the gate should exempt
   deliberate past-tense citations ("the old `focus_menu`" in `overlay.rs:185/332/343` is history,
   not rot).

---

## 6. The simplifier experiment (separate, `chore/simplify`)

Session-experimental rule: an async Codex simplifier working in
`.claude/worktrees/simplifier`, with the orchestrator responsible for merging. **Not promoted —
do not persist the rule without Yan's decision.**

Delivered: a ranked backlog at `.agents/docs/simplification-backlog.md` (9 entries, plus an honest
record of areas surveyed and found clean, so later runs don't re-search them), and **5 of 9 entries
done**. The best find deleted a verbatim copy of `rig_world_pose` from `bake.rs` — inside code doing
bit-exact `to_bits` comparisons, where a drifted copy would have made the verifier validate a
different composition and still pass.

Remaining: entry 8 (only applicable one left), 3 and 6 blocked by the active-net exclusions, 7 wants
focused tests on its input-closure conventions first.

**Recommendation on record:** do not promote "always one instance running." The value is real but
the backlog is finite — nine items, five cleared in an afternoon, and the tree is otherwise clean.
Promote a *periodic survey-then-batch* instead; the survey is the expensive and valuable part and
only needs redoing after significant new code lands.

---

## 7. Operational notes that cost real time to learn

**Wire changes are a non-issue and must not be avoided.** Client and server ship together and the
handshake is version-exact, so a mismatched peer is *refused*, not desynced. A wire change costs a
coordinated release, which every release already is. Never design around one, never stop to ask.
The only obligation is honesty: bump `PROTOCOL_REV`, re-pin deliberately, and say in the commit
what moved and why. (Related: `4f523b1` *declined* an authorized wire field because `settled_at`
derives from `HullShock::tick`, which REV 24 already carries — "a wire field would only have added
a way for the two to disagree." That is the right instinct, not timidity.)

**Determinism is not a refactor brake either.** Same reason. `PROTOCOL_FINGERPRINT` hashes the wire
surface, never the sim math.

**Codex job tracking — the plugin keys its registry on `git rev-parse --show-toplevel` of your
cwd.** A worktree therefore gets a *separate registry*, and querying from the wrong directory
returns "No job found" for a perfectly healthy job (upstream #524). This caused a real incident:
I read that as a dead job, committed a file out from under a live agent, and launched a duplicate
into its worktree. **Always pass `--cwd <root>` on every call — dispatch, status, result, cancel.**

- The registry has `MAX_JOBS = 50` and prunes by deleting the `.json` *and* `.log`. It sits at the
  ceiling. **Snapshot every result to disk on completion** or it will vanish.
- `status` reports the last value *written*; nothing reconciles it against the OS, so a dead worker
  reads `running` forever. Check the pid from `status --json` as well.
- Never call `result` bare — it resolves to "newest finished job", which is a different run the
  moment anything else finishes. Always pass the explicit id captured at dispatch.
- The `codex:codex-rescue` subagent returns in ~20s having only *launched* the job, so its
  completion notification is structurally meaningless. Dispatch directly and wait with a
  backgrounded shell loop instead (`await-codex.sh` in the session scratchpad).
- Codex's sandbox cannot bind loopback UDP (six tests fail `PermissionDenied`) and cannot write
  `.git` or `.agents`. Both are configurable — `network_access` and `writable_roots` — but see the
  open decisions below.

**`cd` persists between Bash calls in this harness.** Both of the above incidents trace to one
stale cwd. Prefer absolute paths and `git -C`.

---

## 8. Decisions waiting on Yan

1. **Grant Codex write access to `.git`?** Recommendation: **no.** The carveout currently enforces
   the `checkout`/`restore`/`stash`/`clean` prohibition *structurally* rather than by instruction,
   and `writable_roots` is coarse — you cannot grant "commit" without also granting `reset --hard`.
   The commit workflow already works with the orchestrator as the gate. (Note: an earlier version of
   this argument cited uncommitted work in the main tree; that work is now committed at `84473ae`,
   so the argument rests on the structural point alone.)
2. **`network_access = true`?** It is **not** loopback-scoped — the emitted Seatbelt policy is bare
   `(allow network-outbound)` / `(allow network-inbound)`. Recommendation: enable per-run for tasks
   that need the UDP tests, not globally in `~/.codex/config.toml` where it would apply to every
   Codex invocation on the machine.
3. **Victim identity on the wire.** `ImpactConfirm` and `RicochetKeyframe` now carry
   `victim: Option<CombatantId>`, needed for per-hull correlation. The justification has been
   rewritten honestly — it **is** new exact target information, deliberately published, because
   proximity inference was not exact enough (that is precisely why the field was added). Policy
   unreversed pending Yan.
4. **Simplifier promote/drop** — see §6.
5. **`PUFF_CAP`** — needs a doc rewrite, not a substitution; confirm the intended reading.

---

## 9. Hard rules in force

- **NEVER** `git checkout`, `git restore`, `git stash`, `git clean`.
- **NEVER** bare `cargo fmt` — per-file `rustfmt --edition 2024 <file>`. `cargo fmt --all --check`
  is fine as a check.
- One cargo command at a time; `pgrep -f "cargo|rustc"` first.
- Do **not** push, merge to main, tag, or release without explicit approval. Commits to the working
  branch are authorized.
- Do **not** file, open, or comment on anything in any external repository. The lightyear writeup at
  `.agents/docs/upstream/` is a **local draft only** — Yan sends it, if ever.
- Codex is the **verification/review lane only**; Claude agents implement. (The simplifier is Yan's
  explicit, session-scoped exception.)
