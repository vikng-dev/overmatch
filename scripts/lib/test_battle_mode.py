"""Runner bounds must read the mode or refuse it, never coerce unreadable input to zero."""
import subprocess
import tempfile
import unittest
from pathlib import Path

HELPER = Path(__file__).with_name("battle-mode.sh").resolve()


class FillUp(unittest.TestCase):
    def read(self, shell, text):
        with tempfile.TemporaryDirectory() as directory:
            mode = Path(directory) / "assets/modes/battle.ron"
            mode.parent.mkdir(parents=True)
            mode.write_text(text)
            return subprocess.run(
                [shell, "-c", 'source "$1"; battle_fill_up_s "$2"', "test", str(HELPER), directory],
                capture_output=True, text=True,
            )

    def test_runner_bounds_accept_numeric_formatting(self):
        # Fixture spans: the bound must ceiling fractions and accept no space after the colon.
        for shell in ("bash", "zsh"):
            for field, expected in (("60.0", "60"), ("59.25", "60"), ("6e1", "60"), ("0.0", "0")):
                for spacing in ("", " ", "\t"):
                    with self.subTest(shell=shell, field=field, spacing=spacing):
                        result = self.read(shell, f"    fill_up_s:{spacing}{field}, // span\n")
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout.strip(), expected)

    def test_unreadable_fields_refuse_the_bound(self):
        for shell in ("bash", "zsh"):
            for text in ("()", "fill_up_s:,", "fill_up_s: invalid,", "fill_up_s:60oops,", "fill_up_s:-1,"):
                with self.subTest(shell=shell, text=text):
                    result = self.read(shell, text)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, "")
                    self.assertIn("cannot read fill_up_s", result.stderr)


if __name__ == "__main__":
    unittest.main()
