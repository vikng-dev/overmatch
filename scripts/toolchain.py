"""The foreign programs this repository pins, declared once.

Blender's collapse decides every LOD level, its glTF exporter decides the bytes of every model, and
Basis Universal decides the bytes of every texture in them. A point release of any of the three
moves shipped assets, so each version is ASSERTED before a stage runs rather than recorded after it.

Both lanes that drive these programs read the pins from here: `scripts/tank/asset_door.py` (the
asset door's preflight) and `scripts/lod/config.py`, which re-exports them under the names its
generator and manifest already use.

Stdlib only, and it never imports `bpy`: the door's preflight runs under the system interpreter,
before there is a Blender to ask.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
from dataclasses import dataclass
from typing import Dict, List, Optional

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


@dataclass(frozen=True)
class Program:
    """One pinned program as this machine holds it: where it is, what it reports, what it must
    report. `note` says why nothing was measured, and is the only field set when the program is
    absent."""

    name: str
    binary: Optional[str]
    measured: Dict[str, str]
    expected: Dict[str, str]
    note: Optional[str] = None

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


def blender(binary: Optional[str] = None) -> Program:
    """The Blender this machine would run, and what it reports about itself. Version AND build:
    two builds report the same version and write different bytes."""
    binary = binary or _which(BLENDER_ENV, "blender", _MACOS_BLENDER)
    expected = {"version": BLENDER_VERSION, "build": BLENDER_BUILD}
    printed, note = _report(binary, ["--version"], "blender", "https://www.blender.org/download/")
    if printed is None:
        return Program("blender", binary, {}, expected, note)
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
    )


def basisu(binary: Optional[str] = None) -> Program:
    """The texture encoder `scripts/encode-tank-ktx2.sh` will call, and its version."""
    binary = binary or _which(BASISU_ENV, "basisu")
    expected = {"version": BASISU_VERSION}
    printed, note = _report(binary, ["-version"], "basisu", "brew install basis_universal")
    if printed is None:
        return Program("basisu", binary, {}, expected, note)
    version = _BASISU_VERSION.search(printed)
    return Program(
        "basisu", binary, {"version": version.group(1) if version else "unknown"}, expected
    )
