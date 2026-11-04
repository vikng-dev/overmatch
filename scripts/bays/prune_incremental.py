#!/usr/bin/env python3
"""Bound an idle bay's incremental cache; release owns the pool and build locks.

Rust 1.98.0's persist/fs.rs (crate_path, find_source_directory_in_iter,
lock_file_path) defines the layout and selects the newest finalized session:
https://github.com/rust-lang/rust/blob/1.98.0/compiler/rustc_incremental/src/persist/fs.rs
Recheck the layout when rust-toolchain.toml changes. Different crate hashes can
serve different Cargo units; only budget pressure evicts those variants.
"""

import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import re
import shutil
import stat
import sys


SESSION = re.compile(r"s-([0-9a-z]+)-([0-9a-z]+)-([0-9a-z]+)")
LOCK = re.compile(r"s-[0-9a-z]+-[0-9a-z]+\.lock")
CRATE = re.compile(r"[A-Za-z0-9_]+-[0-9a-z]+")
GIB = 1024**3
# Operator choice: the three-bay pool retains useful state without allowing the
# MEASURED 33 GB apparent incremental footprint per bay to grow without a bound.
# The pool operator sets OVERMATCH_BAY_INCREMENTAL_GIB on release to change it.
DEFAULT_BUDGET_GIB = 12


class CacheLayoutError(ValueError):
    """Refuse an unfamiliar layout before deleting any cache entries."""


def accounted_bytes(path: Path) -> int:
    """Conservative retention charge, not physical usage (APFS shares blocks).

    Count hard links once per tree. Charge at least the logical length and the
    allocated blocks, including directories; only df measures physical usage.
    """
    seen = set()
    total = 0
    pending = [path]
    while pending:
        entry = pending.pop()
        info = entry.lstat()
        if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
            raise CacheLayoutError(f"{entry}: expected a regular file or directory; inspect it before release")
        inode = (info.st_dev, info.st_ino)
        if inode not in seen:
            seen.add(inode)
            total += max(info.st_size, info.st_blocks * 512)
        if stat.S_ISDIR(info.st_mode):
            pending.extend(entry.iterdir())
    return total


@dataclass
class Variant:
    path: Path
    newest: Path
    lock: Path
    timestamp: int
    charge: int


@dataclass
class Retained:
    variants: int
    charged_bytes: int


def prune(target: Path, budget_bytes: int) -> Retained:
    if budget_bytes <= 0:
        raise ValueError("incremental budget must be positive")
    try:
        target_info = target.lstat()
    except FileNotFoundError:
        # A bay that has not built yet has no cache to retain.
        return Retained(0, 0)
    if stat.S_ISLNK(target_info.st_mode):
        raise CacheLayoutError(f"{target}: target must not be a symlink")
    if not stat.S_ISDIR(target_info.st_mode):
        raise CacheLayoutError(f"{target}: expected a target directory")

    roots = []
    for first in sorted(target.iterdir()):
        mode = first.lstat().st_mode
        if stat.S_ISLNK(mode):
            raise CacheLayoutError(f"{first}: incremental paths must not contain symlinks")
        if not stat.S_ISDIR(mode):
            continue
        for second in sorted(first.iterdir()):
            mode = second.lstat().st_mode
            if stat.S_ISLNK(mode):
                raise CacheLayoutError(f"{second}: incremental paths must not contain symlinks")
            if second.name == "incremental":
                roots.append(second)
            elif stat.S_ISDIR(mode):
                root = second / "incremental"
                try:
                    root.lstat()
                except FileNotFoundError:
                    # Most profile children are artifacts, not target triples.
                    continue
                roots.append(root)
    variants = []
    empty = []
    for root in roots:
        # Validate even entries that will be discarded before any mutation.
        accounted_bytes(root)
        for crate in sorted(root.iterdir()):
            if not crate.is_dir() or not CRATE.fullmatch(crate.name):
                raise CacheLayoutError(f"{crate}: unrecognized crate directory; inspect it before release")
            finalized = []
            for entry in crate.iterdir():
                session = SESSION.fullmatch(entry.name)
                if entry.is_dir() and session:
                    timestamp = int(session[1], 36)
                    lock = crate / (entry.name.rsplit("-", 1)[0] + ".lock")
                    # rustc discards sessions without their lock file; working
                    # sessions were never published and cannot seed a build.
                    if session[3] != "working" and lock.is_file():
                        finalized.append((timestamp, entry, lock))
                elif not (entry.is_file() and LOCK.fullmatch(entry.name)):
                    raise CacheLayoutError(f"{entry}: unrecognized session entry; recheck the pinned rustc layout")
            if not finalized:
                empty.append(crate)
                continue
            timestamp, newest, lock = max(finalized)
            info = crate.stat()
            charge = max(info.st_size, info.st_blocks * 512)
            charge += accounted_bytes(newest) + accounted_bytes(lock)
            variants.append(Variant(crate, newest, lock, timestamp, charge))

    total = sum(variant.charge for variant in variants)
    evicted = set()
    for variant in sorted(variants, key=lambda v: (v.timestamp, v.path)):
        if total <= budget_bytes:
            break
        evicted.add(variant.path)
        total -= variant.charge

    for crate in empty:
        shutil.rmtree(crate)
    for variant in variants:
        if variant.path in evicted:
            shutil.rmtree(variant.path)
        else:
            for entry in variant.path.iterdir():
                if entry not in (variant.newest, variant.lock):
                    if entry.is_dir():
                        shutil.rmtree(entry)
                    else:
                        entry.unlink()
    return Retained(len(variants) - len(evicted), total)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", type=Path)
    args = parser.parse_args()
    try:
        setting = os.environ.get("OVERMATCH_BAY_INCREMENTAL_GIB", str(DEFAULT_BUDGET_GIB))
        if not re.fullmatch(r"[1-9][0-9]*", setting):
            raise ValueError("OVERMATCH_BAY_INCREMENTAL_GIB must be a positive integer in GiB")
        budget = int(setting) * GIB
        kept = prune(args.target, budget)
    except (OSError, ValueError) as error:
        print(f"prune-incremental: {error}; bay remains leased", file=sys.stderr)
        return 1
    print(f"incremental: retained {kept.variants} variants, "
          f"{kept.charged_bytes / GIB:.2f}/{budget / GIB:g} GiB accounted (physical space: df)",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
