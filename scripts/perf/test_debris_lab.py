"""The building-debris model's lab (game state only): what `compare` may pair and how it names the
first difference. Run: python3 -m unittest scripts/perf/test_debris_lab.py"""

import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import debris_lab  # noqa: E402


def result(onset=3, after=3):
    """Fixture: a hull at rest from capture tick 1 until its drive begins at `onset`, then `after`
    ticks moving under it, and an end reading of construction 4."""
    rest = dict(drive=[0.0, 0.0], position_m=[0.0, 0.0, 0.0], forward_m_s=0.0)
    ticks = [dict(tick=tick, hull=copy.deepcopy(rest)) for tick in range(1, onset)]
    ticks += [dict(tick=onset + k, hull=dict(drive=[1.0, 0.0], position_m=[0.0, 0.0, -k], forward_m_s=float(k)))
              for k in range(after)]
    return dict(ticks=ticks, last_tick=onset + after - 1,
                structure=dict(construction=4, displaced=[7, 9], in_place_share=0.9))


class CompareTests(unittest.TestCase):
    def test_runs_pair_only_under_one_recipe(self):
        """Two passes of different recipes are no replay pair; the refusal names the field."""
        record = dict(recipe=dict(instance="house_0001", heading_deg=0.0), plan_sha256="p", clip=dict(frames=49))
        other = copy.deepcopy(record)
        other["recipe"]["heading_deg"] = 90.0
        with self.assertRaisesRegex(SystemExit, "recipe"):
            debris_lab.admit([record, other])
        debris_lab.admit([record, copy.deepcopy(record)])

    def test_runs_key_from_their_input_onset(self):
        """Over UDP each run's throttle arrives on its own tick, so runs are keyed from their onset
        and two passes whose input arrived apart still reach a state verdict. The keying needs the
        hull at rest before input: rows equal from spawn, and the later run holding the earlier
        one's last row until its own onset. A mutant keying by capture tick calls the shifted pair
        an input difference; one comparing only the ticks before both onsets misses a hull that
        crept while the later input was on its way."""
        a = result()
        self.assertEqual(debris_lab.verdict(a, result()), dict(verdict="equal", onsets=[3, 3]))
        late = result(onset=5)
        self.assertEqual(debris_lab.verdict(a, late), dict(verdict="equal", onsets=[3, 5]))
        crept = result(onset=5)
        crept["ticks"][3]["hull"]["position_m"] = [0.0, 0.0, -0.01]
        self.assertEqual(debris_lab.verdict(a, crept), dict(verdict="state differs before input", tick=4,
                                                             field="hull.position_m", onsets=[3, 5]))
        spawned = result(onset=5)
        spawned["ticks"][0]["hull"]["forward_m_s"] = 0.5
        self.assertEqual(debris_lab.verdict(a, spawned), dict(verdict="state differs before input", tick=1,
                                                               field="hull.forward_m_s", onsets=[3, 5]))

    def test_inputs_are_judged_before_state(self):
        """After the onset, a pair whose input differs is outside input, whatever state follows.
        Equal inputs that gave different state are named by the ticks after onset and the field, an
        unequal end (`last_tick`), or at the end by construction and chunk. A mutant judging state
        first calls the steered pair a state difference; one judging inputs over either run's ticks
        calls the unequal end an input difference."""
        a = result()
        steered = result(onset=4)
        steered["ticks"][4]["hull"]["drive"] = [1.0, 0.5]
        steered["ticks"][5]["hull"]["forward_m_s"] = 0.0
        self.assertEqual(debris_lab.verdict(a, steered), dict(verdict="inputs differ", after_onset=1, onsets=[3, 4]))
        moved = result()
        moved["ticks"][4]["hull"]["position_m"] = [0.1, 0.0, -2.0]
        self.assertEqual(debris_lab.verdict(a, moved),
                         dict(verdict="state differs", after_onset=2, field="hull.position_m", onsets=[3, 3]))
        longer = result(after=4)
        self.assertEqual(debris_lab.verdict(a, longer),
                         dict(verdict="state differs", after_onset=2, field="last_tick", onsets=[3, 3]))
        fallen = result()
        fallen["structure"]["displaced"] = [7, 8, 9]
        self.assertEqual(debris_lab.verdict(a, fallen),
                         dict(verdict="state differs", after_onset=2, field="construction 4 chunk 8", onsets=[3, 3]))


if __name__ == "__main__":
    unittest.main()
