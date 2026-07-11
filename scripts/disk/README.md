# disk — target-dir footprint control

Two halves of one fix for target-dir bloat (measured 2026-07-11: ~90% of a 169 GB
fleet footprint was debuginfo, 37 GB of it our own crate's `.o` files):

1. **Profiles** — the real fix lives in `Cargo.toml` (`[profile.*]`): `line-tables-only`
   on our crate, `debug = 0` on deps. Nothing to install; it applies to every build.
2. **Weekly sweep** — this directory. A safety net that caps each checkout's target dir
   at 25 GB so stale fingerprints (old dep versions, old rustc) can't creep back.

## Files

- `sweep.sh` — runs `cargo sweep --maxsize 25GB` over the main checkout and both worktree
  bays (paths hardcoded; missing ones are skipped). Safe to run by hand any time.
- `com.overmatch.target-sweep.plist` — launchd user agent that runs `sweep.sh` weekly,
  Sunday 04:00. Logs to `/tmp/overmatch-target-sweep.log`.

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
