# Bounded bay incremental retention (#275)

Subject: build tooling for the game's agent worktrees; no gameplay changes.

`release` retains the newest finalized session and its lock per crate/hash, removes
superseded, orphaned and unfinished sessions, then evicts variants by oldest session
timestamp until the retained incremental state fits the pool operator's budget.
The default is **12 GiB per bay**, adjustable with `OVERMATCH_BAY_INCREMENTAL_GIB`
on `scripts/bays/release`. This is an operator choice to preserve first-edit reuse
without allowing the pool's caches to grow without a bound. It caps retained
incremental state, not dependency artifacts or temporary sessions during builds.
There is no age expiry: a useful cache remains useful after an idle weekend.

Both the pool lock and the machine build lock cover pruning. Validation completes
before deletion; an unfamiliar layout, symlink, invalid budget or OS error refuses
release with a visible cause and leaves the lease intact. An OS error during
deletion can leave a partially pruned cache, but cannot free the bay. No target
directory is removed by pruning. Lease and Cargo.lock warming are unchanged.
An individual variant exceeding the budget is evicted in full, even if newest;
the budget takes priority over reuse and release reports the retained count.

## Verified compiler contract

The installed compiler matches `rust-toolchain.toml`: **MEASURED Rust 1.98.0**.
Its [incremental filesystem implementation](https://github.com/rust-lang/rust/blob/1.98.0/compiler/rustc_incremental/src/persist/fs.rs)
defines `crate_path`, `generate_session_dir_path`, `find_source_directory_in_iter`
and `lock_file_path`. Its [base-number implementation](https://github.com/rust-lang/rust/blob/1.98.0/compiler/rustc_data_structures/src/base_n.rs)
defines the case-insensitive encoding as base 36.

- Each variant is `<crate>-<stable-crate-id>` inside an incremental directory.
- Sessions are `s-<base36-microseconds>-<random>-working` until publication replaces
  `working` with the crate hash. The lock is `s-<timestamp>-<random>.lock`.
- The next compilation selects the newest finalized session by the encoded
  timestamp, not filesystem mtime, and hard-links or copies its files.
- rustc's collector discards sessions lacking a lock. Its collector excludes its
  own session, which can leave that session plus an older published one.
- Distinct crate/hash directories may serve different Cargo units. Treating them
  as interchangeable would throw away useful Clippy, test or executable state.

The implementation covers `target/<profile>/incremental` and
`target/<triple>/<profile>/incremental`, including a target tree left by the
previous release script with no incremental directories at all.

## Evidence and expectations

**MEASURED filesystem free space:** `df -k .` reported 111,670,204 KiB available
at lane start, about **106.5 GiB**. `df` is the physical-space authority. The cache
budget conservatively charges the larger of file length and allocated blocks,
including directories and counting hard links once per session. APFS clones can
share those blocks, so this accounting is not a physical-usage measurement.
Release reports the filesystem-wide `df` delta, which can include unrelated writes.

The existing pool notes report **MEASURED apparent sizing** of **33 GB incremental
out of 57 GB target artifacts**. These are sizing inputs, not physical usage.
**DERIVED steady-state expectation:** if the other artifacts remain about
`57 - 33 = 24 GB`, adding the retained cap gives about **37 GB apparent per bay**
(`24 GB + 12 GiB`), plus temporary build sessions. Dependency growth is outside
this cap; actual physical use must be checked with `df` by the orchestrator.

[The dev-loop research](research.md) reproduces **MEASURED compiler phases** of
**69.9 s without state versus 14.4–14.8 s incrementally**. The cited
`build-throughput-2026-10-24/` directory is absent from this checkout, so these
are inherited measurements, not independently rerun results.
**DERIVED first-small-edit expectation:** about **15 s for those compiler phases**,
about **55 s saved**, when the next lane reuses retained units with compatible
toolchain, profile, features and dependencies. This is not total queue/build time.
The orchestrator measures actual first-edit latency and peak disk after landing.

## Validation

`python3 scripts/bays/test_prune_incremental.py` exercises fixture-only timestamp
selection, session/lock pruning, distinct profiles and target triples, oldest-first
budget eviction, oversized units, cold caches, hard links, operator errors and
refusal before deletion for symlinks or unknown layouts.

`bash scripts/bays/test_bays.sh` drives the production scripts against a throwaway
repository, redirecting only the machine lock path into the fixture. It verifies
release/next-lease retention, positive and invalid operator budgets, dependency
preservation, Cargo.lock warming and the existing pool safety rules.
No live bay has been released or pruned by this lane.
