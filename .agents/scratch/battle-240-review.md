# Review of battle-241 and battle-242

Subject: Overmatch's match flow and the game's damage model, game state only.
Reviewed commits: `532c6822` (#242), `92d76076` and `a3efef24` (#241), against
AGENTS.md's error-handling, code-branching and base-truths lenses and their issue specs.
The owner forbids subagents, so this is a single implementer review.

## Spawn placement

- P1, `assets/maps/kalinovo/level.json`, `spawn_points`: the candidate set could
  hold fewer stationary players than the chosen capacity. Queue-and-retry could
  leave admitted players waiting forever while everyone stayed put. Fixed by
  expanding the data and exercising a full stationary lobby from every first
  point, without weakening the minimum. The cycle also bounds dynamic congestion
  at its fresh-combat boundary.
- No P2 found. Picks and harness instruments have named users, and automatic
  selection uses checked data and live tanks rather than a vehicle-specific rule.

## Scoring and hosting

- No P1 or P2 found in knockout credit or scoreboard registration. The recorder
  observes `Added<TankKnockedOut>` and chooses the deciding deposition once; the
  score consumes that fact. It does not introduce a second knockout rule. The
  globally replicated board holds combatant IDs, not entity references requiring
  remapping. The cycle's real-UDP probe covers its replication and reset.
- P3, `server::schedule_bot_respawn`: a separate hard-coded wait gave bots a
  different mode rule. Fixed by deleting the scheduler and reading the same
  scored opening as players.
- P3, `HostedConfig::from_lookup`: missing capacity silently invented a limit.
  Fixed with a required setting and visible typed startup refusal naming it.
- P3, `backend/README.md`: the capacity was labelled ASSUMED despite the owner
  having chosen it. Corrected to identify the owner decision.

Area counts: spawn placement, one P1 fixed, no P2 or additional P3; scoring and
hosting, no P1/P2, the three named P3s fixed. No other P3 findings.
