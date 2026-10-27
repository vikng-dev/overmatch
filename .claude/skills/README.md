# Project skills

Each entry here is a symlink into `.agents/skills/`, the lock-managed home (`skills-lock.json`, `npx skills` from https://github.com/mattpocock/skills, MIT, `.agents/skills/LICENSE-mattpocock-skills`), which Codex reads directly. Linked: the promoted `engineering/` and `productivity/` buckets, except `code-review`, which would shadow Claude Code's built-in `/code-review` that this project uses. Update with the `skills` CLI, then add a link here for any new skill Claude should see.

Per-repo configuration these skills read: `.agents/docs/issue-tracker.md` (GitHub Issues), `.agents/docs/triage-labels.md`, `.agents/docs/domain.md`. The issue tracker is GitHub: epics are parent issues labelled `epic` with native sub-issues, dependencies are native "blocked by" edges, and `/wayfinder` maps carry `wayfinder:*` labels.
