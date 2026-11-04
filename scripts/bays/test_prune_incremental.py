"""Fixture-only contracts relied on by the bay pool and its next lane."""

import os
import errno
import io
from contextlib import redirect_stderr
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from prune_incremental import CacheLayoutError, accounted_bytes, main, prune


class PruneTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.target = Path(self.scratch.name) / "target"

    def session(self, crate="debug/incremental/game-abc", stamp="10", suffix="abc", locked=True):
        # Fixture names use rustc's base-36 timestamp and random/hash fields.
        path = self.target / crate / f"s-{stamp}-000001-{suffix}"
        path.mkdir(parents=True)
        (path / "query-cache.bin").write_bytes(b"fixture" * 1000)
        lock = path.parent / f"s-{stamp}-000001.lock"
        if locked:
            lock.touch()
        return path, lock

    def test_next_lane_keeps_newest_published_session_and_its_lock(self):
        old, old_lock = self.session(stamp="z")
        newest, lock = self.session(stamp="10")
        working, working_lock = self.session(stamp="11", suffix="working")
        orphan, _ = self.session(stamp="12", locked=False)
        stray_lock = newest.parent / "s-13-000001.lock"
        stray_lock.touch()
        # mtime does not decide rustc's source session; the encoded timestamp does.
        os.utime(old, (2000, 2000))
        os.utime(newest, (1000, 1000))

        kept = prune(self.target, 10**9)

        self.assertEqual(kept.variants, 1)
        self.assertTrue(newest.is_dir())
        self.assertTrue(lock.is_file())
        for stale in (old, old_lock, working, working_lock, orphan, stray_lock):
            self.assertFalse(stale.exists(), stale)

    def test_distinct_cargo_units_survive_across_profiles_and_target_triples(self):
        sessions = [self.session(crate=crate)[0] for crate in (
            "debug/incremental/game-abc", "debug/incremental/game-def",
            "edit/incremental/game-abc", "aarch64-apple-darwin/edit/incremental/game-abc",
        )]
        dependency = self.target / "debug/deps/libdep.rlib"
        dependency.parent.mkdir()
        dependency.write_text("warm")
        prune(self.target, 10**9)
        self.assertTrue(all(path.is_dir() for path in sessions))
        self.assertEqual(dependency.read_text(), "warm")

    def test_budget_evicts_oldest_variant_even_in_a_different_profile(self):
        old, _ = self.session(crate="edit/incremental/game-abc", stamp="z")
        recent, lock = self.session(crate="debug/incremental/game-def", stamp="10")
        # Derive the cap from a fixture that the current accounting says fits.
        budget = accounted_bytes(recent.parent)
        kept = prune(self.target, budget)
        self.assertFalse(old.parent.exists())
        self.assertTrue(recent.exists())
        self.assertTrue(lock.exists())
        self.assertEqual(kept.variants, 1)
        self.assertLessEqual(kept.charged_bytes, budget)

    def test_oversized_single_variant_is_evicted_without_removing_target(self):
        path, _ = self.session()
        kept = prune(self.target, 1)
        self.assertEqual(kept.variants, 0)
        self.assertEqual(kept.charged_bytes, 0)
        self.assertFalse(path.parent.exists())
        self.assertTrue(self.target.is_dir())

    def test_unpublished_only_variant_and_empty_old_cache_are_discarded(self):
        path, _ = self.session(suffix="working")
        empty = self.target / "edit/incremental/other-def"
        empty.mkdir(parents=True)
        prune(self.target, 10**9)
        self.assertFalse(path.parent.exists())
        self.assertFalse(empty.exists())

    def test_symlink_or_unknown_layout_refuses_before_any_eviction(self):
        old, _ = self.session(stamp="z")
        self.session(stamp="10")
        foreign = Path(self.scratch.name) / "foreign"
        foreign.mkdir()
        marker = foreign / "keep"
        marker.write_text("outside target")
        suspect = self.target / "edit/incremental"
        suspect.parent.mkdir()
        suspect.symlink_to(foreign, target_is_directory=True)
        with self.assertRaises(CacheLayoutError):
            prune(self.target, 1)
        self.assertTrue(old.is_dir())
        self.assertEqual(marker.read_text(), "outside target")
        suspect.unlink()
        unexpected = old.parent / "unexpected"
        unexpected.touch()
        with self.assertRaises(CacheLayoutError):
            prune(self.target, 1)
        self.assertTrue(old.is_dir())

    def test_symlinks_in_retained_or_discarded_sessions_are_refused(self):
        path, _ = self.session(suffix="working")
        (path / "link").symlink_to(Path(self.scratch.name) / "absent")
        with self.assertRaises(CacheLayoutError):
            prune(self.target, 1)
        self.assertTrue(path.is_dir())

    def test_missing_target_is_a_legitimate_cold_bay(self):
        self.assertEqual(prune(self.target, 1).variants, 0)
        self.assertFalse(self.target.exists())

    def test_discovery_errors_visibly_refuse_release_before_any_eviction(self):
        old, _ = self.session(stamp="z")
        self.session(stamp="10")
        triple = self.target / "aarch64-apple-darwin/edit"
        triple.mkdir(parents=True)
        # Inject OS failures even when the test runner can bypass permissions.
        # Release must see failure, not a successful empty-cache report.
        for operation, paths in (
            ("iterdir", (self.target, self.target / "debug", triple.parent)),
            ("lstat", (self.target, triple / "incremental")),
        ):
            original = getattr(Path, operation)
            for path in paths:
                with self.subTest(operation=operation, path=path):
                    def refuse(entry):
                        if entry == path:
                            raise PermissionError(errno.EACCES, "Permission denied", str(entry))
                        return original(entry)

                    stderr = io.StringIO()
                    with patch.object(Path, operation, refuse), \
                            patch.object(sys, "argv", ["prune_incremental.py", str(self.target)]), \
                            redirect_stderr(stderr):
                        self.assertEqual(main(), 1)
                    self.assertIn(str(path), stderr.getvalue())
                    self.assertIn("Permission denied", stderr.getvalue())
                    self.assertIn("bay remains leased", stderr.getvalue())
                    self.assertNotIn("retained", stderr.getvalue())
                    self.assertTrue(old.is_dir())

    def test_operator_typo_refuses_without_deleting_a_session(self):
        path, _ = self.session()
        for setting in ("", "0", "-1", "1.5", "twelve"):
            with self.subTest(setting=setting):
                result = subprocess.run(
                    [sys.executable, str(Path(__file__).with_name("prune_incremental.py")), str(self.target)],
                    env={**os.environ, "OVERMATCH_BAY_INCREMENTAL_GIB": setting},
                    capture_output=True, text=True, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("OVERMATCH_BAY_INCREMENTAL_GIB", result.stderr)
                self.assertTrue(path.exists())

    def test_hardlinked_work_products_are_charged_once_per_tree(self):
        path, _ = self.session()
        before = accounted_bytes(path)
        info = path.stat()
        old_directory_charge = max(info.st_size, info.st_blocks * 512)
        product = path / "query-cache.bin"
        os.link(product, path / "another-name.bin")
        info = path.stat()
        directory_growth = max(info.st_size, info.st_blocks * 512) - old_directory_charge
        self.assertEqual(accounted_bytes(path), before + directory_growth)


if __name__ == "__main__":
    unittest.main()
