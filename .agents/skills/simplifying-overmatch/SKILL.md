---
name: simplifying-overmatch
description: Use before simplifying, cleaning up, tidying, de-duplicating or "improving code quality" in this repo, and before acting on any generic simplification advice here. Carries the guardrails that generic advice gets wrong.
---

Generic simplification advice is wrong in this repo in specific, damaging ways. This skill exists
to fire *before* that advice is acted on.

**Read [`.agents/docs/code-quality-standard.md`](../../docs/code-quality-standard.md) now** — it is
the brief: what to hunt (§A), what not to touch (§B), how to work (§C).

## The six that do damage

Inlined because every simplification run needs them and the cost of loading them late is a bad
commit. Everything else is in the doc.

1. **Never split a file for being large.** `transmission.rs` (~3 065 lines, 33 public items) is
   deliberately one file. The gate is on **function** length (300, `tests/fn_length.rs`), and it is
   function-not-file *for stated reasons*. Do not add rows to its `ALLOWED` list.
2. **Never open a commit to remove comments.** 23 % of `src/` is comments and 14 % is `///` item
   docs, which are the module *interface*. The applicable rule is about content, not volume: a
   comment states the current invariant, never the edit history.
3. **Never touch `tests/` or `vendor/`.** `tests/` is eleven gates and upstream tripwires — several
   are tautological on purpose and fail only on a dependency bump. `vendor/` carries
   `// OVERMATCH PATCH:` marks whose value is diffing cleanly against upstream.
4. **Never touch a `WIRE_SURFACE` type.** Renaming or reordering a replicated type is a protocol
   change needing a deliberate `PROTOCOL_REV` bump (ADR-0018).
5. **Never strip a MEASURED / DERIVED label**, and never move a constant away from its label.
6. **Determinism is NOT a brake.** Client and server ship together and the handshake is
   version-exact, so float and math refactors in the sim **are allowed**. Declining a legitimate
   simplification citing "determinism" is itself a failure — it has happened. Only two things bind:
   no SIMD math path (ADR-0028), and report a moved measured result rather than silently re-pinning.

## Before filing anything

Stock clippy runs at `-D warnings` on correctness/suspicious/style/complexity/perf, so everything
those categories catch is **already gone**. Hunt what a linter cannot see, or you add nothing.

Verify every engine API against the pinned version — Bevy 0.19, avian3d 0.7, lightyear 0.28 — never
from memory. `AGENTS.md` requires it and it has repeatedly caught real renames.

When it is a judgement call, **stop and report** rather than guess. The doc's §C.7 lists what is
automatically a judgement call.
