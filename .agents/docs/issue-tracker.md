# Issue tracker: GitHub

Issues, epics and specs for this repo live as GitHub issues on `vikng-dev/overmatch` (public). Use the `gh` CLI for every operation. The repo is public: no credentials, no agent ids, and the damage-model vocabulary of `AGENTS.md` in every issue and comment.

## Shape

- **Epic**: a parent issue labelled `epic`. Its tasks are native sub-issues. An epic body is an index: destination, decisions so far as one line each linking the task that holds the detail, and what is not yet specified. Never restate a decision in the epic.
- **Task**: a sub-issue of one epic, sized to one agent session, with acceptance criteria. Carries one category label (`bug` / `enhancement`) and one state label (see `triage-labels.md`).
- **Decision**: a `/wayfinder` ticket (`wayfinder:*` labels) when the question is sharp but the answer is not; the owner resolves it, the resolution is a comment, and the epic's index gains one line.
- **Blocking**: native issue dependencies, so the frontier is visible in GitHub's own UI.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body-file <file>` (write the body to a scratch file first; no heredocs in `run_in_background` shells).
- **Read an issue**: `gh issue view <number> --json number,title,body,labels,comments`.
- **List issues**: `gh issue list --state open --json number,title,labels` with `--label` filters.
- **Sub-issue**: `gh issue create --parent <epic> ...`, or `gh issue edit <epic> --add-sub-issue <child>` (`gh` 2.94+). Older `gh`: `gh api --method POST repos/vikng-dev/overmatch/issues/<epic>/sub_issues -F sub_issue_id=<child-db-id>` (database id: `gh api repos/vikng-dev/overmatch/issues/<n> --jq .id`).
- **Blocking**: `gh issue create --blocked-by <n>,<n>` or `gh issue edit <child> --add-blocked-by <blocker>` (`gh` 2.94+). Older `gh`: `gh api --method POST repos/vikng-dev/overmatch/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`.
- **Count open blockers**: `gh api repos/vikng-dev/overmatch/issues/<n> --jq .issue_dependencies_summary.blocked_by` (0 means startable).
- **Comment**: `gh issue comment <number> --body-file <file>`.
- **Labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`.
- **Close**: comment the resolution, then `gh issue close <number>`.

File issues only into `vikng-dev/overmatch`. Never file, open or comment in an external repository (`AGENTS.md`, hard rules).

## Pull requests as a triage surface

**PRs as a request surface: no.** Work lands by direct push to `main` after the agent gates; PRs are not a request surface.

## When a skill says "publish to the issue tracker"

Create a GitHub issue, as a sub-issue of the epic it belongs to.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets.

- **Map**: one issue labelled `wayfinder:map` (an epic may double as a map: add the label). Body: Destination / Notes / Decisions so far / Not yet specified / Out of scope.
- **Child ticket**: a sub-issue of the map, labelled `wayfinder:<type>` (`research` / `prototype` / `grilling` / `task`). Once claimed, assigned to the driving dev.
- **Blocking**: as under Conventions.
- **Frontier query**: the map's open sub-issues with no open blocker and no assignee; first in map order wins.
- **Claim**: `gh issue edit <n> --add-assignee @me`, the session's first write.
- **Resolve**: comment the answer, close the issue, append one line (gist + link) to the map's Decisions so far.
