# Lens: code-branching

Principle: `AGENTS.md`, "Commit to one path". Once a decision is made the code holds one way to do
it; every switch that remains names its current user: a setting a player or operator changes, a
hosting contract, or a tracked script that runs it.

## Finding classes

- **Kept alternative**: a second way to do a decided thing, still compiled: a cargo feature, `cfg`
  arm, build profile, env var, const toggle, setting, enum variant or function whose only caller is a
  test or a dev lever, a `legacy`/`old`/`v2`/`_experimental` twin, a reachable fallback path for a
  case that should not happen.
- **Switch without a user**: an env var, flag, feature, cfg arm, profile or setting that no player,
  operator, hosting contract or tracked script sets. Check `scripts/`, `.github/`, the release smoke
  and the settings UI before calling a switch unused.
- **Finished A/B**: both arms of an experiment whose lane has reported.
- **Test arm of a deleted path**: a test that pins the behaviour of an alternative the code no longer
  takes, or keeps a dead arm alive.
- **Commented-out code** and dead code kept "in case".
- **Fallback for a should-not-happen case**: hand to the error-handling lens's outcomes (ADR-0011)
  and report it here only if it is also a kept alternative.

## Not a finding

- A switch whose current user the reviewer can name, and where that user is.
- `cfg(test)`, and a platform `cfg` where the platforms genuinely differ (`target_os`).
- A rejected alternative described in prose (a doc names it and why it lost).
- An open A/B experiment inside the lane that runs it.

## Fix

Delete the losing alternative in one change, with its flag, env var, feature, cfg arm, profile and
test arms, and fix every doc line that names it as live. A switch with a real user keeps a line
naming that user beside its definition.
