# Lens: error-handling

Principle: `AGENTS.md`, "Every failure has one deliberate outcome". Every failure site gets exactly
one of four outcomes, at the earliest gate that can see it:

1. **Unrepresentable**: a type rules it out (an enum per state, a checked newtype with one
   constructor, `Dir3`, `#[require]`, `Infallible`).
2. **Refused before play**: the asset compiler, `build.rs`, a `const` assert or the load gate stops,
   in every build, naming the file, the field and the fix.
3. **A bug**: a broken invariant panics in every build, its `expect` saying why it should have held.
4. **Outside input**: network peers, player input, player-owned files, the OS and operator levers get
   a typed error enum and visible handling (the server refuses and logs; the player sees the cause;
   the process refuses to start).

A **legitimate absence** is not a failure: a state the game really passes through (no local tank
before spawn, no round in flight). It is fine as a skip once the type or a line beside the skip names
the state.

## Finding classes

- **Silent exit**: a `let … else { return / continue }`, an `if let` with no else, or an
  `Option<Single>` / `Single` system that leaves a case unseen when that case is a failure rather
  than a named absence. Bevy 0.19 skips a `Single` system silently on none and on many.
- **Invented default**: a value standing in for a truth the data should state: `unwrap_or(<number>)`,
  `unwrap_or_default()`, `Default` impls or `#[serde(default)]` on asset data (`*.tank.ron`,
  `materials.ron`, rounds, substances, maps), a code constant used when a field is absent.
- **Dropped error**: `.ok()`, `let _ = <Result>`, `.unwrap_or(..)` on a `Result`, `.flatten()` over
  results, an error mapped to `None` or to a log line and then ignored.
- **Degrade in release**: `debug_assert!` guarding an invariant the release build relies on, a
  `cfg(debug_assertions)` arm that panics in debug and carries on in release, a `warn!` or `error!`
  where the case is a bug and the code continues.
- **Late check**: data validated at first use during play instead of at load; validate-then-`expect`
  downstream of an edge that should have parsed into a type; an invariant a type could carry but a
  runtime check carries instead.
- **Fail-closed arm**: a per-event "on error, do nothing and log" path in the simulation, the
  server's included. It is a bug outcome wearing a handled-error costume: classify each of its error
  kinds into outcomes 1–3.
- **Outside input reaching a panic or a default**: a network message, player file or env var that
  can panic the process, or silently become a default (a typo read as unset).
- **Erased error**: `anyhow`, `Box<dyn Error>` or `Result<_, String>` past a load-time parser whose
  message a panic prints.

## Not a finding

- A named legitimate absence (above).
- Outside input handled visibly through a typed error.
- A panic or `expect` whose message says why the invariant holds.
- Tests, and the panic-family patterns (`unwrap`, `expect`, `panic!`) themselves: a panic is the
  right outcome for a bug.

## Fix by outcome

- Missing truth: put the value in the data with its provenance, and make the field required.
- Can't happen: a type, else an `expect` naming the invariant.
- Data fault: a load-gate check naming file, field and fix.
- Outside input: a typed error enum and the handling the input's owner sees.
- Legitimate absence: name the state in the type or beside the skip.
