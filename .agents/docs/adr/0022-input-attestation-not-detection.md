# Input attestation (superseded by input on arrival)

Subject: the game's fire control and damage model, game state only.

**Status: superseded.** Connection-scoped edge ids, cumulative application/refusal answers and the server's live/stale arrival state replace tick attestation. No command is addressed to a tick. See `.agents/scratch/input-arrival-slice-1.md` for implementation and proof, and `.agents/scratch/net-timing-design-uplink-r2-2026-11-17.md` for the accepted design.

The retained invariant is that the authority never spends a round for an action nobody requested. Tick attestation guarded a native input buffer that could repeat or synthesize commands; edge identity removes that buffer from the tank-input path and makes every discrete request apply at most once. The server's arrival-gap state releases held fire and refuses non-fresh starts on recovery. Withdrawals always apply.

The rejected mechanism and its measurements remain in git history and `upstream/lightyear-absent-anchor-input-freeze.md`. Its input-delay pin, attestation and local-fire bookkeeping have no remaining runtime user.
