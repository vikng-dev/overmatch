# Settings brevity pilot

Scope: `src/settings.rs` and every file in `src/settings/`, based on main `test: restore killcam device clock and notch assertions (#146)`. Counts below stop at `settings: shorten page explanations and reuse click fixtures`, before the separate backlog fixes.

## MEASURED line classes

One counter for both snapshots: blank if whitespace-only; comment if its first non-space characters are `//`, `/*` or `*`; otherwise code. This is the requested lexical classification, not a Rust parser.

| file | code, before → after | comment, before → after | blank, before → after |
|---|---:|---:|---:|
| src/settings.rs | 2466 → 2466 | 1682 → 1311 | 228 → 228 |
| src/settings/displays.rs | 372 → 372 | 133 → 95 | 35 → 35 |
| src/settings/limiter.rs | 144 → 144 | 107 → 68 | 20 → 20 |
| src/settings/preset.rs | 344 → 344 | 65 → 53 | 26 → 26 |
| src/settings/probe.rs | 176 → 176 | 124 → 59 | 17 → 17 |
| src/settings/store.rs | 650 → 650 | 270 → 191 | 51 → 51 |
| src/settings/ui.rs | 1967 → 1948 | 764 → 465 | 148 → 146 |
| **Total** | 6119 → 6100 | 3145 → 2242 | 525 → 523 |

DERIVED from those counts: 903 comment lines (28.7%), 19 code lines and 2 blank lines removed; 924 total lines (9.4%). Production executable lines are unchanged in the brevity commits. The code cut reuses the existing pointer/click fixtures in the arrow interaction test and retains both assertions.

## Finding counts

- Restated code: 10.
- Padding: 78.
- Repeated why: 60.
- Edit history: 10.
- Retold narrative: 1.
- Knowledge stated twice: 0.
- Shallow abstraction: 0.
- Needless intermediate: 0.
- Hand-written standard operation: 0.
- Test ceremony: 1.
- Dead code: 0.

Each comment-block rewrite is one finding, assigned to its primary class. Zero-result code classes were checked: the slider types differ in units/Off semantics; checked constants have guards; helpers preserve ownership or name game state; intermediates carry units or reuse; no unreachable setting or legacy file reader was demonstrated.

## Five representative cuts

### Metres rather than tier names

`src/settings.rs`; Padding; DERIVED net 2 lines saved.

Before:
```rust
    /// The row value the settings page renders. ASCII only — it reaches `Text`. Honest metres rather
    /// than LOW/MEDIUM/HIGH: the number is the thing a player can actually compare against a sight
    /// picture, and it is what makes "my shadows stop halfway to that ridge" a self-answering
    /// complaint.
```

After:
```rust
    /// ASCII only — it reaches `Text`. Metres, rather than LOW/MEDIUM/HIGH, let players compare the
    /// cutoff with the sight picture.
```

### Off preserves a valid config

`src/settings.rs`; Padding; DERIVED net 3 lines saved.

Before:
```rust
        // `Off` deliberately leaves the cascade config at its LAST value rather than rebuilding it
        // to something degenerate: nothing samples it while casting is disabled, and re-enabling
        // must not depend on a zero-metre envelope having been repaired first. A cascade-count
        // change made while off therefore stays PENDING and lands on this same rebuild the moment a
        // distance exists again — the row is inert while off, not lost.
```

After:
```rust
        // Off preserves the last valid config: a zero-metre rebuild would be degenerate. Count changes
        // remain pending until the next casting frame; re-enabling needs no repair.
```

### One owner for probe failure reasoning

`src/settings/probe.rs`; Repeated why; DERIVED net 10 lines saved.

Before:
```rust
/// Distil a wgpu capability list into the two facts the ladder gates on. Pure, so the mapping is
/// testable against fake capability lists.
///
/// **An EMPTY list is a failure, not an answer.** `Surface::get_capabilities` does not return a
/// `Result`: wgpu 29 flattens a failed `surface_get_capabilities` into `SurfaceCapabilities::
/// default()`, whose `present_modes` is an empty `Vec`, and its own docs specify the same empty
/// list for a surface the adapter cannot present to. A surface that CAN be presented to always
/// reports at least `Fifo` (wgpu guarantees it), so "no present modes at all" can only mean the
/// query failed or the surface is incompatible — it is not evidence about `Immediate` or `Mailbox`,
/// and letting it reach [`PresentCaps::Reported`] would hand `settings::normalize_vsync` a
/// fabricated negative to spend a player's stored FAST/OFF on. Same class as the surface-creation
/// failure below, one layer deeper, and answered the same way.
```

After:
```rust
/// Distil the two ladder gates. Empty means query failure/incompatibility, never evidence
/// for normalization; see the module doc.
```

### Main-thread limiter placement

`src/settings/limiter.rs`; Repeated why; DERIVED net 4 lines saved.

Before:
```rust
/// The `Last` system. Reads the settings every frame (one `Option` construction); does nothing at
/// all unless the cap is armed.
///
/// The `NonSendMarker` is load-bearing — see the module doc: it keeps the wait off the shared
/// `ComputeTaskPool`, which pipelined rendering needs while this thread is parked.
```

After:
```rust
/// `Last` system; [`NonSendMarker`] keeps the wait off the shared compute pool (see module doc).
```

### Drag ownership and saving

`src/settings/ui.rs`; Padding; DERIVED net 2 lines saved.

Before:
```rust
/// The slider drag in progress, if any. While `Some`, the mouse belongs to that slider: the value
/// scrubs with the cursor every frame (even outside the track's rect — standard slider behaviour),
/// and the SAVE happens once, on release, instead of once per dragged frame.
```

After:
```rust
/// An active drag owns the mouse, scrubs even outside the track, and saves once on release.
```

## Kept deliberately

- The shadow pricing, split arithmetic and audit tables stay in source: the shadow design note explicitly points there for them, and they carry unique conditions and bounds.
- Physical/CSS/window-logical units, frozen drag geometry, asynchronous fullscreen sequencing and the no-retry rule remain. Their counterexamples explain non-obvious code.
- Persistence keeps sparse defaults, version-before-schema ordering, anonymous RON, unique temporary files and durability tradeoffs. The retained `F_FULLFSYNC` rejection also anchors the source-scan exception.
- Old-file readers and aliases stay because player files still use them; they are compatibility paths with users, not unused alternatives.

## Backlog ratchet

Closed, each in its own behavior commit after the measured brevity sequence:

- `load-04`, `settings: refuse nonfinite pixel budgets at load`: serde refusal reaches the existing typed corrupt-file report and preserves the input. Finite off-ladder edits remain exact; nonfinite code construction is a bug.
- `load-14`, `settings: assert settings page construction cardinalities`: missing/duplicate cards and hints panic with construction guarantees while the page is hidden or visible. A table covers both failure shapes and both visibility states.
- `load-05`, `settings: show typed save failures on the page`: retain filesystem stage/path/cause; show safe-font failure copy until a successful retry. Resolve the process config path once. Directory-sync failure reports that bytes were written but power-loss durability is unconfirmed.

Added `load-16`–`load-23`: stale shadow, LOD, writer and test-premise claims; a preset test coupled to current tastes; authored values in code; remaining optional-singleton failures; impossible-state defaults. These were verified against source and recorded rather than folded into brevity or silently fixed. No existing row was dismissed as a non-defect.

## DERIVED tree-wide estimate

Baseline `src/` is MEASURED with the same counter: 305 Rust files, 199,935 code lines, 49,206 comment lines and 13,193 blank lines. The selected pilot holds 6.4% of all comment lines.

Planning estimate: roughly 5,000–10,000 comment lines across `src/`, assuming a 10–20% yield against 49,206 comment lines. This discounts the pilot’s 28.7% because it was selected for dense explanations. Repeated rationale and edit history should scale; unique laws, provenance and counterexamples do not. Blindly projecting the pilot would give about 14,100 lines and is not a defensible forecast.

## Validation

Formatting, CI clippy with `dev_ui,bitprobe`, 89 targeted settings/doc-citation/font/provenance tests and 44 whole-tree guards passed on the completed brevity tree. Unchanged files were committed individually against that verified tree. The nonfinite-budget step also passed formatting/clippy and 89 targeted tests. The cardinality step passed formatting/clippy and 127 combined targeted/whole-tree guards. The final save-error step passed formatting/clippy and 129 combined targeted/whole-tree guards. Tests ran through `scripts/serial-cargo`, unattended/headless, under its machine lock. No screen/captures, CI changes, external issue comments or pushes.
