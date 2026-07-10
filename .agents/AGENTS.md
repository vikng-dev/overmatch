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
- **UI strings rendered through Bevy `Text` must stay within the bundled Barlow Condensed coverage.** The client ships Barlow Condensed (`assets/fonts/`, loaded by `ui_font::UiFonts`), which has no glyph fallback either — anything outside its coverage still draws tofu. Coverage is printable ASCII (U+0020–U+007E) plus the verified typographic set `… — – ° × ± ≤` (U+2026, U+2014, U+2013, U+00B0, U+00D7, U+00B1, U+2264), each confirmed present in both shipped weights' cmaps. Those typographic characters are now safe in rendered strings; anything more exotic needs a fresh cmap check against the shipped `.ttf`s before it can be used (and added to `TYPOGRAPHIC_SET` in the test). This applies ONLY to strings that reach `Text`; comments, logs (`info!`/`warn!`/`error!`/…), panics and asserts are unrestricted — the house style uses em dashes heavily there, do not touch them. `tests/ui_ascii.rs` guards the rendered set (it strips comments and diagnostic-macro arguments, then asserts the remaining string literals in the `Text`-spawning files stay within that coverage). Note the dev sandboxes (`sandbox.rs`, `track_sandbox`) keep Bevy's default ASCII-only font, so keep their own labels ASCII.
- **Treat this codebase's own prose as a claim, not as ground truth.** The doc comments, design docs and ADRs here are unusually detailed, which makes them unusually persuasive when they are stale or wrong. In one 2026-07-09 session: a design doc asserted a finding its own table cell had superseded the same day; ADR-0015 repeated it as *measured*; two ADRs cited [[0004-avian-physics]] for a lockstep rejection it never makes; a doc comment declared a code path unreachable that a server fallback reaches; and a much-quoted `~125 m` figure turned out to be derived, never measured, and 2.5× too large. Every one was caught by checking source or by measuring. **Label every number MEASURED or DERIVED. An invariant a comment relies on must be named in that comment, or it is not stated.** Where comment and code disagree, the code wins and the comment is the bug.
