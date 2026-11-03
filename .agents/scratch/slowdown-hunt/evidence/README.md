# Reproduce the observations
Subject: the video game's environment destruction and damage model; CPU and game state only.

Work only in a leased bay. All commands inherit:
```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1
export OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1
scripts/serial-cargo build --locked --profile profiling --config 'profile.profiling.package.overmatch.codegen-units=16' --config 'profile.profiling.package.overmatch.debug=1' --bin overmatch-server --features dev_ui,bitprobe
scripts/serial-cargo build --locked --bin overmatch --features dev_ui,bitprobe
mkdir -p target/profiling-symbols
cp target/profiling/overmatch-server target/profiling-symbols/overmatch-server
/usr/bin/strip -S target/profiling-symbols/overmatch-server
python3 .agents/scratch/slowdown-hunt/evidence/measure.py quiet --profile profiling-symbols --duration 35 --samples 15
python3 .agents/scratch/slowdown-hunt/evidence/measure.py shell --profile profiling-symbols --duration 120 --shells 20 --samples 25,65,100
python3 .agents/scratch/slowdown-hunt/evidence/measure.py ram --profile profiling-symbols --duration 150 --plan .agents/scratch/slowdown-hunt/evidence/ram.ron --samples 45,82,105,135
python3 .agents/scratch/slowdown-hunt/evidence/analyze.py target/slowdown-hunt/quiet target/slowdown-hunt/shell target/slowdown-hunt/ram
```
The runner takes the machine lock itself; never nest it. It checks the local port, launches only direct local server/client connections, and reaps its children. `-mayDie` preloads profiler symbols; `strip -S` removes debug-map lookup, retaining symbol names. MEASURED baseline executable `__text` SHA-256 was identical before/after stripping. The runner records exact window offsets and trailing censoring; exclude failed clients and incomplete windows.

`instrumentation.patch` and `proof.patch` are **alternative throwaway patches**, each based on the commit subject `hosting: claim one end report and park panic watchdog (#194)`. Neither is a shipping implementation. Apply in the leased bay at that baseline. The first adds census/timing probes; the second also compares original/candidate weight verdicts and mutable state, preserving warm original section caches. It contains the non-shipping stationary-contact guard witness. All physics APIs come from the vendored Avian source.

Build the patched test artifact with `scripts/serial-cargo test --lib --features dev_ui,bitprobe --no-run`. Run the diagnostic filter `buildings::breakage::tests::diag_pieces_rest` with `--exact --nocapture --test-threads=1`, under `scripts/serial-cargo test` or one machine lock around the built test binary. Recipe: `DIAG_KIND=native`, `DIAG_ACTION=baseline|threshold|timer|guard`, `DIAG_SECONDS=80` (guard observation: 180). Independent pile: `DIAG_KIND=1000 DIAG_ACTION=guard DIAG_SECONDS=120`. MEASURED native recipe uses four diagnostic construction inputs through production breakage/release geometry; it is a flat-ground composition, not the terrain server. No forced sleep was used.

For production counters, build the patched `overmatch-server` in `dev` through the queue and run `measure.py --profile debug --variant 'dev opt1; counters only' --samples ''`. Use the same shell/ram recipes. `DIAG_GUARD=1` selects the wake experiment at startup; `DIAG_REMOVE_BUILDINGS=1` removes standing buildings at the diagnostic's twentieth period. The unpatched production binary has neither switch.

`rest_summary.py` reads the tagged native logs. The frozen-contact witness fails with `DIAG_GUARD=0` and passes with `1`; the existing impact/support-removal tests pass with the guard. For the weight comparison, use `proof.patch`, run `buildings::fracture` and the native diagnostic with `DIAG_ACTION=baseline DIAG_SECONDS=45`. `[DEBUG-gate]` totals are calls, original ns, candidate ns, eligible calls, original eligible ns, candidate eligible ns. Original-first order can favor candidate caches; this is a mechanism probe, not release pricing.

Remove all patches and temporary source instrumentation before final formatting, CI-feature clippy, targeted tests and whole-tree guards. Keep only the shipping no-live-joint early return. `summary.json`, `counters.json`, `rest-summary.jsonl` and `gate-proof-summary.json` retain compact results; large raw logs, profiler trees, binaries, failed attempts and build/check logs stay under `target/slowdown-hunt/`.
