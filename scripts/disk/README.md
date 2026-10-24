# disk — target-dir footprint control

Two halves of one fix for target-dir bloat (measured 2026-07-11: ~90% of a 169 GB
fleet footprint was debuginfo, 37 GB of it our own crate's `.o` files):

1. **Profiles** — the real fix lives in `Cargo.toml` (`[profile.*]`): `line-tables-only`
   on our crate, `debug = 0` on deps. Nothing to install; it applies to every build.
2. **The sweep** — this directory. A safety net that reaps leaked `.rcgu.o`, incremental
   directories untouched for a day, and then caps each checkout's target dir at 40 GB so
   stale fingerprints (old dep versions, old rustc) can't creep back. The ceiling sits above
   the incremental steady state on purpose: `cargo sweep` counts `incremental/` but cannot
   evict it, and a ceiling below it evicts every warm dependency instead (see `sweep.sh`).

## Files

- `sweep.sh` — the reapers and `cargo sweep --maxsize 40GB` over the main checkout and every
  `git worktree list` entry. Safe to run by hand any time (`FORCE=1` skips the session check).
- `com.overmatch.target-sweep.plist` — launchd user agent that runs `sweep.sh` every 6 h while a
  Claude Code session is running. Logs to `/tmp/overmatch-target-sweep.log`.

- `warm-target.sh` — run from a fresh worktree's root (`scripts/serial-cargo` runs it on a
  worktree's first build): clones the main checkout's `target/` (APFS, seconds) and gives every
  tracked file outside `src/` that is identical to main's copy main's mtime, so cargo reuses every
  dependency main already compiled — including Bevy above the `vendor/` path crates, which a plain
  `cp -c -R` rebuilds because checkout stamps them newer than the clone. The crate itself
  rebuilds, so its binaries carry this worktree's path.

Requires `cargo-sweep` (`cargo install cargo-sweep`; 0.8.0 at time of writing).

## Install the weekly agent

Not installed automatically — one manual step, run once:

```sh
cp scripts/disk/com.overmatch.target-sweep.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.overmatch.target-sweep.plist
```

Verify it registered, and (optionally) fire it once immediately to confirm it works:

```sh
launchctl list | grep com.overmatch.target-sweep
launchctl start com.overmatch.target-sweep
cat /tmp/overmatch-target-sweep.log
```

To uninstall:

```sh
launchctl unload ~/Library/LaunchAgents/com.overmatch.target-sweep.plist
rm ~/Library/LaunchAgents/com.overmatch.target-sweep.plist
```

The agent runs `sweep.sh` from the **main** checkout
(`.../personal/overmatch/scripts/disk/sweep.sh`), so keep that tree present; the bays are
optional.
