"""test_report.py — the one report shape's contract: its order, its two renderings, its exit code.

    python3 scripts/tank/test_report.py

Every stage of the door emits `report.Finding` and nothing else, so what is fenced here is what a
human and a machine both read: findings sort by stage, severity, check id, subject and element;
console and JSON hold EVERY finding with every field of the declared shape; exit is non-zero
exactly when the report holds an error.

Stdlib only, no Blender: this module never imports `bpy`, and a contract test that needed a
three-second Blender launch would be a contract test nobody runs.
"""

import itertools
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import report  # noqa: E402
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402


def finding(stage=Stage.SOURCE, severity=Severity.ERROR, check="L1.CHECK",
            kind=SubjectKind.OBJECT, name="name", element="element", evidence="evidence",
            law="law", repair="repair"):
    """One finding, every field of the declared shape settable — so a fixture can differ from
    another in exactly one of them."""
    return Finding(
        Check(id=check, stage=stage, severity=severity, law=law),
        Subject(kind, name, element),
        evidence,
        repair,
    )


#: One pair per component of the declared sort key, in key order. Each pair AGREES on every
#: component before the named one, puts `first` ahead at it, and puts `second` ahead at every
#: component after it — so a key that drops that component orders the pair the other way, and a key
#: that drops the evidence tie-breaker leaves the pair in the order it was given.
#:
#: `late` is what each pair uses for the components after the decisive one: the LOW value goes to
#: the finding that must come SECOND.
def _pair(component, first, second):
    return (component, finding(**first), finding(**second))


HIGH = dict(check="z.CHECK", kind=SubjectKind.MATERIAL, name="z", element="z", evidence="z")
LOW = dict(check="a.CHECK", kind=SubjectKind.DOOR, name="a", element="a", evidence="a")

SORT_KEY = [
    _pair(
        "stage",
        dict(stage=Stage.DOOR, severity=Severity.INFO, **HIGH),
        dict(stage=Stage.SOURCE, severity=Severity.ERROR, **LOW),
    ),
    _pair(
        "severity",
        dict(severity=Severity.ERROR, **HIGH),
        dict(severity=Severity.WARNING, **LOW),
    ),
    _pair(
        "check id",
        dict(check="a.CHECK", kind=SubjectKind.MATERIAL, name="z", element="z", evidence="z"),
        dict(check="b.CHECK", kind=SubjectKind.DOOR, name="a", element="a", evidence="a"),
    ),
    _pair(
        "subject kind",
        dict(kind=SubjectKind.DOOR, name="z", element="z", evidence="z"),
        dict(kind=SubjectKind.MATERIAL, name="a", element="a", evidence="a"),
    ),
    _pair(
        "subject name",
        dict(name="a", element="z", evidence="z"),
        dict(name="b", element="a", evidence="a"),
    ),
    _pair(
        "subject element",
        dict(element="a", evidence="z"),
        dict(element="b", evidence="a"),
    ),
    _pair("evidence", dict(evidence="a"), dict(evidence="b")),
]


class SortOrder(unittest.TestCase):
    """The declared order, one key component at a time."""

    def test_every_component_of_the_key_decides_a_pair(self):
        for component, first, second in SORT_KEY:
            with self.subTest(component=component):
                # Handed in the WRONG order, so a key that ignores this component either flips the
                # pair back (every component after it disagrees) or, for the tie-breaker, leaves it.
                self.assertEqual(
                    report.sorted_findings([second, first]), [first, second],
                    "{} does not decide the order".format(component),
                )

    def test_the_sort_settles(self):
        """Sorting a sorted report is the same report: an order that moved on a second pass is one
        nobody can diff."""
        findings = [pair[1] for pair in SORT_KEY] + [pair[2] for pair in SORT_KEY]
        once = report.sorted_findings(findings)
        self.assertEqual(report.sorted_findings(list(reversed(once))), once)

    def test_the_whole_report_is_in_key_order(self):
        findings = report.sorted_findings([pair[2] for pair in SORT_KEY]
                                          + [pair[1] for pair in SORT_KEY])
        keys = [finding.sort_key() for finding in findings]
        self.assertEqual(keys, sorted(keys))


class Renderings(unittest.TestCase):
    """Text and JSON are renderings of ONE report: the same rows, in the same order, holding every
    field of the declared shape."""

    #: The finding shape the design enumerates, and how each field is read out of each rendering.
    FIELDS = ("check", "severity", "kind", "name", "element", "evidence", "law", "repair")

    def report_of(self, count=3):
        """Findings whose every field is a distinct sentinel, so a renderer that drops one — or
        prints another field in its place — cannot pass."""
        return report.sorted_findings([
            finding(
                stage=Stage.SOURCE,
                severity=[Severity.ERROR, Severity.WARNING, Severity.INFO][index % 3],
                check="L1.CHECK_{}".format(index),
                kind=SubjectKind.MESH,
                name="name-{}".format(index),
                element="element-{}".format(index),
                evidence="evidence-{}".format(index),
                law="law-{}".format(index),
                repair="repair-{}".format(index),
            )
            for index in range(count)
        ])

    def test_json_holds_exactly_the_declared_shape(self):
        rows = json.loads(report.render_json(self.report_of()))["findings"]
        self.assertEqual(len(rows), 3)
        for row in rows:
            self.assertEqual(
                set(row), {"check", "stage", "severity", "subject", "evidence", "law", "repair"}
            )
            self.assertEqual(set(row["subject"]), {"kind", "name", "element"})

    def test_text_and_json_carry_the_same_rows_field_by_field(self):
        findings = self.report_of()
        rows = json.loads(report.render_json(findings))["findings"]
        lines = report.render_text(findings).rstrip("\n").split("\n")
        self.assertEqual(len(lines), 4 * len(findings), "one four-line block per finding")

        self.assertEqual([row["check"] for row in rows],
                         [finding.check.id for finding in findings])
        for index, (row, finding_) in enumerate(zip(rows, findings)):
            block = "\n".join(lines[index * 4:index * 4 + 4])
            with self.subTest(check=row["check"]):
                # Every field of the shape, out of BOTH renderings and the finding itself.
                self.assertEqual(row["check"], finding_.check.id)
                self.assertEqual(row["severity"], finding_.check.severity.label)
                self.assertEqual(row["stage"], finding_.check.stage.name.lower())
                self.assertEqual(row["subject"]["name"], finding_.subject.name)
                self.assertEqual(row["subject"]["element"], finding_.subject.element)
                self.assertEqual(row["evidence"], finding_.evidence)
                self.assertEqual(row["law"], finding_.check.law)
                self.assertEqual(row["repair"], finding_.repair)
                for field in self.FIELDS:
                    value = {
                        "check": row["check"],
                        "severity": row["severity"],
                        "kind": row["subject"]["kind"],
                        "name": row["subject"]["name"],
                        "element": row["subject"]["element"],
                        "evidence": row["evidence"],
                        "law": row["law"],
                        "repair": row["repair"],
                    }[field]
                    self.assertIn(value, block, "the text rendering dropped the {}".format(field))

    def test_an_element_less_subject_renders_both_ways(self):
        """The element is optional — a finding about a whole object has none — and neither
        rendering may invent one."""
        bare = [finding(element=None)]
        row = json.loads(report.render_json(bare))["findings"][0]
        self.assertIsNone(row["subject"]["element"])
        self.assertNotIn("None", report.render_text(bare))


class ExitStatus(unittest.TestCase):
    """Non-zero exactly when the report holds an error — every combination of what a report can
    hold, including the empty one."""

    def test_the_truth_table(self):
        tiers = (Severity.ERROR, Severity.WARNING, Severity.INFO)
        for size in range(len(tiers) + 1):
            for held in itertools.combinations(tiers, size):
                findings = [finding(severity=severity, check=severity.label) for severity in held]
                expected = 1 if Severity.ERROR in held else 0
                with self.subTest(held=[severity.label for severity in held]):
                    self.assertEqual(report.exit_code(findings), expected)
                    self.assertEqual(report.has_error(findings), Severity.ERROR in held)
                    self.assertEqual(
                        report.counts(findings),
                        {severity: (1 if severity in held else 0) for severity in tiers},
                    )

    def test_the_summary_names_every_tier_and_counts_them(self):
        findings = [finding(severity=Severity.ERROR), finding(severity=Severity.WARNING),
                    finding(severity=Severity.WARNING)]
        self.assertEqual(report.summary(findings), "1 error, 2 warnings, 0 info")


if __name__ == "__main__":
    unittest.main(verbosity=2)
