# lightyear 0.28: Predicted+Interpolated dual markers route updates through the no-check path

**Target:** lightyear 0.28 · **Severity for us:** HIGH (worked around d4f92d3; root-caused and the
workaround DELETED at f2c9510 — see the corrected workaround section) · **Status:** unfiled

## Suggested title

Entity carrying both Predicted and Interpolated markers silently bypasses rollback checks
(equal-priority write-fn tie-break)

## Mechanism

A replicated entity can arrive carrying BOTH `Predicted` and `Interpolated` (in our topology:
`PredictionTarget::Single(client)` + `InterpolationTarget::AllExceptSingle` — the predicted
client's own tank still arrived with both). The replicon write-fn registration for
Position/Rotation has an equal-priority tie-break between the prediction and interpolation
paths; when interpolation wins, confirmed updates are applied through interpolation's
no-mismatch-check path — the rollback check never runs for those components, with no warning.

## Measured consequence

Silent multi-second Position desyncs (6–16 cm) on the player's own predicted tank with zero
rollbacks — discovered only via per-entity confirmed-authority trace fields
(`ConfirmedHistory::newest_present` vs predicted pose).

## Suggested upstream fix

Either make the marker combination an explicit error/warn (an entity should not be both), or
give the prediction write path strict priority when both markers are present.

## Our workaround + removal condition — **CORRECTED 2026-07-12: the workaround is already gone**

Originally `demote_predicted_interpolated` (then src/net/rig.rs, commit d4f92d3): a client-side system
that stripped `Interpolated` from any entity also carrying `Predicted`, polled every frame.

**It was deleted at f2c9510, and the reason matters for the filing.** The dual marker was not lightyear
mis-targeting: the server never inserted `ReplicationSender` on its per-client `LinkOf` entities, so
lightyear's per-client visibility hooks (`ReplicationTarget::on_insert` / `ControlledBy::on_insert`)
silently no-op'd — and an **unset visibility bit defaults to VISIBLE**, so `Predicted` / `Interpolated` /
`Controlled` broadcast to every client and the owner's own tank arrived carrying both markers. With
`ReplicationSender` attached the owner arrives `Predicted`-only: **0 strips over ~840 frames** in a
two-client loopback, which is why the demotion system could be removed outright.

So the *seed* was our configuration error. What stays upstream-reportable is the **failure mode**: when
an entity does end up with both markers — reachable, as we proved, from an ordinary omission with no
error and no warning — the equal-priority write-fn tie-break silently routes confirmed updates through
interpolation's no-check path and rollback dies quietly. Remove nothing on our side when upstream defines
the precedence; there is nothing left to remove.

## What fixing this unlocks for us

**Clean up: nothing — already banked** (`demote_predicted_interpolated` is deleted; see the correction
above). **Optimize: nothing.**

**Explore: nothing — the payoff is purely diagnostic, and it is worth stating that way in the filing.**
This defect cost us multi-second, 6–16 cm silent Position desyncs on the player's own predicted tank with
zero rollbacks, found only by writing a per-entity confirmed-authority trace. A one-line `warn!` on the
marker combination (or strict prediction precedence) converts that hunt into a log line. That is the whole
value of the fix to us, and it is entirely for the *next* person who forgets a `ReplicationSender`.
