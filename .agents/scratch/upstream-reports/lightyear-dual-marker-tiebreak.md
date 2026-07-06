# lightyear 0.28: Predicted+Interpolated dual markers route updates through the no-check path

**Target:** lightyear 0.28 · **Severity for us:** HIGH (fixed d4f92d3) · **Status:** unfiled

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

## Our workaround + removal condition

`demote_predicted_interpolated` (src/net/rig.rs:289-297, commit d4f92d3): a client-side system
that strips `Interpolated` from any entity also carrying `Predicted`, polled every frame (the
marker can be re-added by later replication actions). Remove when upstream defines the
precedence.
