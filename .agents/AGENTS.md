## Agent skills

### Issue tracker

Issues and PRDs live as local markdown files under `.agents/scratch/<feature>/`. See `.agents/docs/issue-tracker.md`.

### Triage labels

Five canonical triage roles, using the default label strings. See `.agents/docs/triage-labels.md`.

### Domain docs

Single-context: `.agents/GLOSSARY.md` + `.agents/docs/adr/`. See `.agents/docs/domain.md`.

### Playtest forks

Design decisions chosen *provisionally*, to be settled by how the slice feels in play —
each with its default, preserved alternatives, and revert cost. See
`.agents/scratch/playtest-forks/README.md`. (Distinct from ADRs, which are settled.)

## Working discipline

- **Sim state is built at spawn, from data — never from an asset, never late.** No change may initialize a rollback-registered component from a loaded asset, or insert/attach sim state onto an already-replicated entity after spawn. If it rolls back, it must be constructible synchronously at spawn from data (the glb is a *view*, not the sim constructor). See `.agents/docs/adr/0014-sim-view-split.md`.
- **Treat Bevy/Avian API knowledge as deprecated.** Both move fast; this project pins **Bevy 0.19** and **avian3d 0.7** (`Cargo.toml`). Verify every engine API against versioned docs or source *before* writing it — `docs.rs/bevy/0.19.0/…` / `docs.rs/avian3d/0.7.0/…`, or the git tags `v0.19.0` (bevyengine/bevy) and `v0.7.0` (Jondolf/avian) for example/source. This has repeatedly caught real renames (`Trigger`→`On`, buffered events→observers, `Camera` moving to `bevy::camera`, `SceneRoot`→`WorldAssetRoot`). Do not write engine code from memory.
