"""The one finding shape every asset-door stage emits, and its deterministic renderings.

Text and JSON are renderings of this shape; the exit status is `exit_code`. A check's severity is
compiled into its `Check` beside its id — there is no severity file, allowlist, per-asset override
or warning budget anywhere in the door.

The Python half of the door spans two interpreters: the Blender source pass runs inside Blender's
embedded CPython, the derivation verifier and the outer wrapper run under the system one. So this
module is stdlib-only and never imports `bpy`.

`src/bake/report.rs` is the same shape in Rust, and the two agree on stage and severity ordering:
one report reads the same rows in the same order whichever half produced them.
"""

from __future__ import annotations

import enum
import json
from dataclasses import dataclass
from typing import Optional, Sequence, Tuple


class Stage(enum.IntEnum):
    """Which pass a check belongs to. The report's primary sort key, declared in pipeline order.

    `DOOR` precedes every pass: it carries the door's own mechanical refusals, which happen before
    a check of the asset can be reached. The Rust half never emits it and so does not declare it.
    """

    DOOR = 0
    SOURCE = 1
    CONSUMER = 2
    DERIVATION = 3


class Severity(enum.IntEnum):
    """A finding's tier. Declared in report order, so a sort puts the build-failing rows first."""

    ERROR = 0
    WARNING = 1
    INFO = 2

    @property
    def label(self) -> str:
        return _SEVERITY_LABELS[self]


_SEVERITY_LABELS = {
    Severity.ERROR: "error",
    Severity.WARNING: "warning",
    Severity.INFO: "info",
}

_SEVERITY_PLURALS = {
    Severity.ERROR: "errors",
    Severity.WARNING: "warnings",
    Severity.INFO: "info",
}


class SubjectKind(enum.IntEnum):
    """What a finding is about. Declared outermost-first, which is also the report's sort order."""

    DOOR = 0
    FILE = 1
    SCENE = 2
    OBJECT = 3
    MESH = 4
    MATERIAL = 5


_SUBJECT_LABELS = {
    SubjectKind.DOOR: "door",
    SubjectKind.FILE: "file",
    SubjectKind.SCENE: "scene",
    SubjectKind.OBJECT: "object",
    SubjectKind.MESH: "mesh",
    SubjectKind.MATERIAL: "material",
}


@dataclass(frozen=True)
class Subject:
    """The thing a finding names: kind, name, and the element inside it — a modifier, a shape-key
    datablock, a transform channel — where the check found something smaller than the whole
    subject."""

    kind: SubjectKind
    name: str
    element: Optional[str] = None

    def sort_key(self) -> Tuple[int, str, str]:
        return (int(self.kind), self.name, self.element or "")

    def __str__(self) -> str:
        text = "{} `{}`".format(_SUBJECT_LABELS[self.kind], self.name)
        return "{} {}".format(text, self.element) if self.element else text


@dataclass(frozen=True)
class Check:
    """One check: its id, the pass and tier compiled in beside it, and the law it enforces."""

    id: str
    stage: Stage
    severity: Severity
    #: The law, stated as the condition that must hold.
    law: str


@dataclass(frozen=True)
class Finding:
    """One violation: which check, of what, with what was measured and what repairs it."""

    check: Check
    subject: Subject
    #: What was measured — the numbers, names and coordinates the verdict rests on.
    evidence: str
    #: The concrete edit that makes the law hold.
    repair: str

    def sort_key(self):
        """Stage, severity, check id, subject, element — the declared report order. Evidence breaks
        a remaining tie, so two findings that differ at all still order deterministically."""
        return (
            int(self.check.stage),
            int(self.check.severity),
            self.check.id,
            self.subject.sort_key(),
            self.evidence,
        )

    def as_dict(self) -> dict:
        return {
            "check": self.check.id,
            "stage": self.check.stage.name.lower(),
            "severity": self.check.severity.label,
            "subject": {
                "kind": _SUBJECT_LABELS[self.subject.kind],
                "name": self.subject.name,
                "element": self.subject.element,
            },
            "evidence": self.evidence,
            "law": self.check.law,
            "repair": self.repair,
        }

    def __str__(self) -> str:
        return (
            "{id} {severity}: {subject}\n"
            "  measured: {evidence}\n"
            "  law:      {law}\n"
            "  repair:   {repair}"
        ).format(
            id=self.check.id,
            severity=self.check.severity.label,
            subject=self.subject,
            evidence=self.evidence,
            law=self.check.law,
            repair=self.repair,
        )


def sorted_findings(findings: Sequence[Finding]) -> list:
    """Put a report in its declared order. Every producer sorts before it returns, so the console,
    the JSON and a GUI popup read the same rows in the same order."""
    return sorted(findings, key=Finding.sort_key)


def has_error(findings: Sequence[Finding]) -> bool:
    """Whether the report fails the build."""
    return any(finding.check.severity == Severity.ERROR for finding in findings)


def exit_code(findings: Sequence[Finding]) -> int:
    """Non-zero exactly when the report holds an error. Warnings never change exit status."""
    return 1 if has_error(findings) else 0


def counts(findings: Sequence[Finding]) -> dict:
    """How many findings of each tier, every tier present as a key."""
    tally = {severity: 0 for severity in Severity}
    for finding in findings:
        tally[finding.check.severity] += 1
    return tally


def summary(findings: Sequence[Finding]) -> str:
    """The one-line tally a GUI popup can show whole. It never replaces the rows: the complete
    report is the console and the JSON."""
    tally = counts(findings)
    return ", ".join(
        "{} {}".format(
            tally[severity],
            severity.label if tally[severity] == 1 else _SEVERITY_PLURALS[severity],
        )
        for severity in Severity
    )


def render_text(findings: Sequence[Finding]) -> str:
    """The complete text rendering — every finding, one block each."""
    return "".join("{}\n".format(finding) for finding in findings)


def render_json(findings: Sequence[Finding]) -> str:
    """The complete JSON rendering — the same rows in the same order, one object each."""
    return json.dumps({"findings": [finding.as_dict() for finding in findings]}, indent=2)
