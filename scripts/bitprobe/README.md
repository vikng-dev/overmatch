# Cross-target bitprobe

The bitprobe runs one fixed tracked-vehicle fixture for the DECLARED 3,072 ticks at 64 Hz and writes
raw startup values plus raw per-tick payloads at seven simulation seams. It is a differential
instrument: it identifies the first bit that differs between two builds, then shows when each
downstream seam diverges and how its f32 error grows.

## Run the pair

From the repository root on macOS aarch64, with the fixture's Git LFS asset present:

```sh
BEVY_ASSET_ROOT="$PWD" cargo run --release --locked --features bitprobe --bin bitprobe -- \
  bitprobe-macos-aarch64.obp.gz
```

Push the exact branch under test, dispatch the Linux x86_64 leg, and use the run ID printed in the
workflow URL:

```sh
gh workflow run bitprobe.yml --ref <pushed-branch>
gh run watch <run-id> --exit-status
gh run download <run-id> --name bitprobe-linux-x86_64 \
  --dir /tmp/overmatch-bitprobe-linux
```

Compare the two raw dumps:

```sh
python3 scripts/bitprobe/diff.py \
  bitprobe-macos-aarch64.obp.gz \
  /tmp/overmatch-bitprobe-linux/bitprobe-linux-x86_64.obp.gz
```

## Read the result

- Exit `0`, `startup dump: IDENTICAL`, `tick payloads: IDENTICAL`, and every seam marked
  `identical` is a clean pair.
- Exit `1` means a raw value differs. Read `first tick divergence` first: it names the earliest
  seam, field, raw bits, decoded values, and physical delta. `per-seam first divergence` shows the
  downstream order; `divergence growth` reports each seam's per-tick f32 L-infinity norm.
- Exit `2` means the dumps are malformed, incomplete, or describe incompatible scenarios/schemas;
  it is not a determinism result.

Rerun this macOS-aarch64/Linux-x86_64 pair before merging any dependency, compiler/profile, or
target-feature upgrade that touches math or physics. Preserve both dumps for any failing pair.
