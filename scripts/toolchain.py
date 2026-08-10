"""The foreign programs this repository pins, declared once.

Blender's collapse decides every LOD level, its glTF exporter decides the bytes of every model, and
Basis Universal decides the bytes of every texture in them. A point release of any of the three
moves shipped assets, so each version is ASSERTED before a stage runs rather than recorded after it.

Both lanes that drive these programs read the pins from here: `scripts/tank/asset_door.py` (the
asset door's preflight) and `scripts/lod/config.py`, which re-exports them under the names its
generator and manifest already use.

A pin is asserted in whichever interpreter can read it. Blender and the encoder are programs the
wrapper can run, so its preflight measures those; the glTF exporter is an add-on inside Blender and
only the source pass can. Both halves build the same `door.toolchain` row out of `finding()`, so
one law is stated once and reads the same wherever it fires.

Stdlib only apart from the report shape, and it never imports `bpy`: the door's preflight runs
under the system interpreter, before there is a Blender to ask.

It is also a PROGRAM — `python3 scripts/toolchain.py` asserts the pins, `--pins` prints them — so
the lane that must install these versions before it can assert them runs this file rather than a
few lines of Python written into a workflow step. Nothing in a YAML step can be executed by any
suite, which is how a step that imported a module from the wrong directory shipped deterministically
red.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from typing import Dict, List, Optional

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "tank"))

import report  # noqa: E402  — the path above is what makes this importable
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402

# ── the pins ─────────────────────────────────────────────────────────────────────────────────────

#: MEASURED off the build that cut the shipped LOD corpus and the shipped tank glb.
BLENDER_VERSION = "5.1.2"
BLENDER_BUILD = "ec6e62d40fa9"

#: The glTF exporter is an add-on on its own release schedule, and it is what writes the model.
#: Readable only from inside Blender, so the LOD generator asserts it there.
GLTF_EXPORTER_VERSION = "5.1.20"

#: The texture encoder, MEASURED locally: `basisu -version`.
BASISU_VERSION = "2.10.0"

#: Where a program comes from when it is not the one on PATH.
BLENDER_ENV = "OVERMATCH_BLENDER"
BASISU_ENV = "OVERMATCH_BASISU"

#: macOS installs Blender inside an app bundle and puts nothing on PATH.
_MACOS_BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"

#: `blender --version` prints the version on its first line and the build hash on a later one.
_BLENDER_VERSION = re.compile(r"^Blender\s+(\S+)", re.MULTILINE)
_BLENDER_BUILD = re.compile(r"^\s*build hash:\s*(\S+)", re.MULTILINE)

#: `basisu -version` prints its banner first, version at the end of the first line.
_BASISU_VERSION = re.compile(r"Supercompression System\s+v(\S+)")


#: The one law every pin is asserted under, wherever it is asserted.
TOOLCHAIN = Check(
    id="door.toolchain",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="the foreign programs this chain runs are the versions scripts/toolchain.py pins",
)


@dataclass(frozen=True)
class Program:
    """One pinned program as this machine holds it: where it is, what it reports, what it must
    report. `note` says why nothing was measured, and is the only field set when the program is
    absent. `override` names the variable that points this repository at another copy, and is None
    for a program that ships inside another one."""

    name: str
    binary: Optional[str]
    measured: Dict[str, str]
    expected: Dict[str, str]
    note: Optional[str] = None
    override: Optional[str] = None

    @property
    def mismatch(self) -> List[str]:
        """Every pinned field this machine disagrees with, one phrase each. Empty is the pass."""
        if self.note is not None:
            return [self.note]
        return [
            "{} is {!r}, pinned to {!r}".format(what, self.measured.get(what, "unknown"), expected)
            for what, expected in sorted(self.expected.items())
            if self.measured.get(what) != expected
        ]

    def __str__(self) -> str:
        if self.note is not None:
            return "{}: {}".format(self.name, self.note)
        return "{} {}".format(
            self.name, " ".join(self.measured[key] for key in sorted(self.measured))
        )


def _which(env: str, program: str, *fallbacks) -> Optional[str]:
    """The binary this repository means by `program`: the named override, then PATH, then the
    platform's own install location."""
    override = os.environ.get(env)
    if override:
        return override if os.path.isfile(override) else None
    found = shutil.which(program)
    if found:
        return found
    return next((path for path in fallbacks if os.path.isfile(path)), None)


def _report(binary: Optional[str], arguments, name: str, install: str):
    """`(stdout, note)` of a version query, exactly one of which is None."""
    if binary is None:
        return (None, "{} is not on PATH — install it ({})".format(name, install))
    try:
        result = subprocess.run(
            [binary] + list(arguments), stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
    except OSError as error:
        return (None, "{} could not be run: {}".format(binary, error))
    return (result.stdout.decode(errors="replace"), None)


def finding(program: Program) -> Optional[Finding]:
    """The `door.toolchain` row this program owes, or None when it is the pinned one."""
    mismatch = program.mismatch
    if not mismatch:
        return None
    return Finding(
        TOOLCHAIN,
        Subject(SubjectKind.DOOR, "toolchain", program.name),
        "; ".join(mismatch),
        "install the pinned version, {} — a program is not a specification, and a point release of "
        "this one moves the bytes of every asset it touches".format(
            "or point {} at it".format(program.override) if program.override else
            "or move the pin in scripts/toolchain.py deliberately"
        ),
    )


def blender(binary: Optional[str] = None) -> Program:
    """The Blender this machine would run, and what it reports about itself. Version AND build:
    two builds report the same version and write different bytes."""
    binary = binary or _which(BLENDER_ENV, "blender", _MACOS_BLENDER)
    expected = {"version": BLENDER_VERSION, "build": BLENDER_BUILD}
    printed, note = _report(binary, ["--version"], "blender", "https://www.blender.org/download/")
    if printed is None:
        return Program("blender", binary, {}, expected, note, BLENDER_ENV)
    version = _BLENDER_VERSION.search(printed)
    build = _BLENDER_BUILD.search(printed)
    return Program(
        "blender",
        binary,
        {
            "version": version.group(1) if version else "unknown",
            "build": build.group(1) if build else "unknown",
        },
        expected,
        override=BLENDER_ENV,
    )


def basisu(binary: Optional[str] = None) -> Program:
    """The texture encoder `scripts/encode-tank-ktx2.sh` will call, and its version."""
    binary = binary or _which(BASISU_ENV, "basisu")
    expected = {"version": BASISU_VERSION}
    printed, note = _report(binary, ["-version"], "basisu", "brew install basis_universal")
    if printed is None:
        return Program("basisu", binary, {}, expected, note, BASISU_ENV)
    version = _BASISU_VERSION.search(printed)
    return Program(
        "basisu", binary, {"version": version.group(1) if version else "unknown"}, expected,
        override=BASISU_ENV,
    )


# ── the continuation token ───────────────────────────────────────────────────────────────────────

#: What the source pass leaves beside a raw candidate it cut, and the only thing that tells such a
#: candidate from any other file of the right shape. `asset_door.py --from-raw` continues a chain
#: somebody else's Blender started; without this the continuation is an unauthenticated entrance at
#: L2, and an L2-clean model cut from a source violating an L1-only law could replace the tracked
#: glb. It lives here because both halves already read this file and neither imports the other.
CONTINUATION_SUFFIX = ".continuation.json"

#: The pins a candidate is cut under, as the token records them.
PINNED = {
    "blender version": BLENDER_VERSION,
    "blender build": BLENDER_BUILD,
    "glTF exporter": GLTF_EXPORTER_VERSION,
}


def continuation_path(raw: str) -> str:
    return raw + CONTINUATION_SUFFIX


def _digest(path: str) -> str:
    """sha256 of a file, read in blocks: a tank glb is tens of megabytes."""
    hashed = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            hashed.update(block)
    return hashed.hexdigest()


def write_continuation(raw: str, measured: Dict[str, str], report_digest: str) -> str:
    """Write the token beside `raw`: the sha256 of the bytes that were just written, the toolchain
    that wrote them AS MEASURED, and a digest of the source report those bytes passed."""
    document = {
        "raw_sha256": _digest(raw),
        "toolchain": dict(measured),
        "report_sha256": report_digest,
    }
    path = continuation_path(raw)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle, sort_keys=True)
    return path


def continuation_mismatch(raw: str) -> List[str]:
    """Every reason this candidate is not one a passing source pass cut, one phrase each. Empty is
    the pass. An absent, unreadable or malformed token is a refusal like any other: a continuation
    that cannot be authenticated is not a continuation."""
    path = continuation_path(raw)
    try:
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
        recorded = str(document["raw_sha256"])
        measured = {str(key): str(value) for key, value in dict(document["toolchain"]).items()}
        str(document["report_sha256"])
    except OSError as error:
        return ["no continuation token at {}: {}".format(path, error)]
    except (ValueError, KeyError, TypeError) as error:
        return ["{} does not hold the continuation token's shape: {}".format(path, error)]
    mismatch = []
    actual = _digest(raw)
    if actual != recorded:
        mismatch.append(
            "the token was written for sha256 {}, and these bytes are sha256 {}".format(
                recorded, actual
            )
        )
    mismatch += [
        "{} was {!r} when this candidate was cut, pinned to {!r}".format(
            what, measured.get(what, "unknown"), expected
        )
        for what, expected in sorted(PINNED.items())
        if measured.get(what) != expected
    ]
    return mismatch


def gltf_exporter() -> Program:
    """The glTF exporter add-on loaded in THIS Blender. Readable nowhere else, which is why the
    source pass asserts it and the wrapper's preflight cannot."""
    expected = {"version": GLTF_EXPORTER_VERSION}
    try:
        import io_scene_gltf2  # noqa: PLC0415 — only exists inside Blender
    except ImportError:
        return Program("glTF exporter", None, {}, expected,
                       "the glTF exporter add-on is not importable — this is not a Blender")
    version = getattr(io_scene_gltf2, "bl_info", {}).get("version")
    return Program(
        "glTF exporter", getattr(io_scene_gltf2, "__file__", None),
        {"version": ".".join(str(part) for part in version) if version else "unknown"},
        expected,
    )


# ── the pins as a program ────────────────────────────────────────────────────────────────────────

#: The pins a lane must know BEFORE it can assert them: it downloads Blender by version and build,
#: and builds the encoder from the tag its version names. Printed as environment lines, so the
#: version a cache key is cut from is the one this file declares and never a second copy in YAML.
ENVIRONMENT = (
    ("OVERMATCH_BLENDER_VERSION", BLENDER_VERSION),
    ("OVERMATCH_BLENDER_BUILD", BLENDER_BUILD),
    ("OVERMATCH_BASISU_VERSION", BASISU_VERSION),
)


def main(argv: Optional[List[str]] = None) -> int:
    """`--pins` prints what to install; no argument asserts what is installed, in the door's own
    report shape and with the door's own exit status."""
    parser = argparse.ArgumentParser(prog="toolchain.py", allow_abbrev=False)
    parser.add_argument("--pins", action="store_true",
                        help="print the pinned versions as KEY=VALUE lines, for a lane that must "
                             "install these programs before it can assert them")
    if parser.parse_args(argv).pins:
        for key, value in ENVIRONMENT:
            print("{}={}".format(key, value))
        return 0
    programs = [blender(), basisu()]
    findings = [row for row in (finding(program) for program in programs) if row is not None]
    print(report.render_text(report.sorted_findings(findings)), end="")
    for program in programs:
        print("toolchain ▸ {} ({})".format(program, program.binary))
    return report.exit_code(findings)


if __name__ == "__main__":
    sys.exit(main())
