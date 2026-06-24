## Agent skills

### Issue tracker

Issues and PRDs live as local markdown files under `.agents/scratch/<feature>/`. See `.agents/docs/issue-tracker.md`.

### Triage labels

Five canonical triage roles, using the default label strings. See `.agents/docs/triage-labels.md`.

### Domain docs

Single-context: `.agents/CONTEXT.md` + `.agents/docs/adr/`. See `.agents/docs/domain.md`.

## Working discipline

- **Treat Bevy API knowledge as deprecated.** Bevy moves fast; this project pins **Bevy 0.18.1** (`Cargo.toml`). Verify every Bevy API against current docs or versioned source *before* writing it — `docs.rs/bevy/0.18.1/…`, or `raw.githubusercontent.com/bevyengine/bevy/v0.18.0/…` for example/source. This has repeatedly caught real renames (`Trigger`→`On`, buffered events→observers, `Camera` moving to `bevy::camera`). Do not write Bevy code from memory.
