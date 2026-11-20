"""The building-debris model's lab (game state only): what `compare` may pair and how it names the
first difference. Run: python3 -m unittest scripts/perf/test_debris_lab.py"""

import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import debris_lab  # noqa: E402


def result():
    """Fixture: three capture ticks of hull rows and an end reading of construction 4."""
    ticks = [dict(tick=tick, hull=dict(drive=[1.0, 0.0], position_m=[0.0, 0.0, -tick], forward_m_s=float(tick)))
             for tick in (1, 2, 3)]
    return dict(ticks=ticks, last_tick=3,
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

    def test_inputs_are_judged_before_state(self):
        """A pair whose input differs is outside input, whatever state follows; equal inputs make a
        state difference an order dependence, named by tick and field, or at the end by construction
        and chunk. A mutant judging state first calls the input pair an order dependence."""
        a = result()
        self.assertEqual(debris_lab.verdict(a, result()), dict(verdict="equal"))
        steered = result()
        steered["ticks"][1]["hull"]["drive"] = [1.0, 0.5]
        steered["ticks"][2]["hull"]["forward_m_s"] = 0.0
        self.assertEqual(debris_lab.verdict(a, steered), dict(verdict="inputs differ", tick=2))
        moved = result()
        moved["ticks"][2]["hull"]["position_m"] = [0.1, 0.0, -3.0]
        self.assertEqual(debris_lab.verdict(a, moved),
                         dict(verdict="state differs", tick=3, field="hull.position_m"))
        fallen = result()
        fallen["structure"]["displaced"] = [7, 8, 9]
        self.assertEqual(debris_lab.verdict(a, fallen),
                         dict(verdict="state differs", tick=3, field="construction 4 chunk 8"))


if __name__ == "__main__":
    unittest.main()
