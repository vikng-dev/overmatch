Subject: the video game's dedicated-server environment physics; game state only.

Run only in a leased bay. The RON numbers are diagnostic input; the report owns the interpretation.
Copy `reproduce.py` and `nearest.ron` from this evidence directory into `target/diag-tree-crash/`
before switching source versions, since the evidence files do not exist at the release tag.

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
export OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1
export RUST_BACKTRACE=1
git switch --detach v0.7.0-alpha.10
scripts/serial-cargo build --locked --bin overmatch --bin overmatch-server --features dev_ui,bitprobe
python3 target/diag-tree-crash/reproduce.py alpha target/diag-tree-crash/nearest.ron
# Repeat the build and command on main, using label main.
```

The runner holds the machine lock around the real dedicated server and real hidden client.
Set `DIAG_WINDOW=headless` for the GPU-less variant. Exit status one means the server exited;
the JSON records both processes' observed statuses before cleanup. Logs and traces go into
`target/diag-tree-crash/`; the runner stops and joins every process it starts.

For the minimized red witness, apply `witness.patch` to the comparison source, then run:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe diag_tree_crash_detached_support -- --nocapture
```

MEASURED: this test fails at the same belt-shape invariant after production tree detachment.
`identity-probe.patch` adds only the tagged diagnostic used in the headless network run.
Revert temporary source edits after either probe; neither patch is a shipping fix.
