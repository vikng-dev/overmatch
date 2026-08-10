"""asset_door.py — the one asset door, and the only thing anyone runs.

    python3 scripts/tank/asset_door.py lint   assets/<id>/<id>.blend
    python3 scripts/tank/asset_door.py export assets/<id>/<id>.blend
    python3 scripts/tank/asset_door.py verify assets/<id>/<id>.blend

The spec sheet and the model derive from the blend's stem: an asset is the sibling trio
`<id>.blend`, `<id>.tank.ron`, `<id>.glb` in `assets/<id>/`. Nothing is named twice.

WHAT EACH MODE IS
-----------------
`lint` runs the L1 source pass over the stored blend and prints one report.

`export` runs the whole chain — source pass, raw candidate, the consumer contract on it, the KTX2
texture derivation, the derivation verifier, the consumer contract again on the baked bytes — and
replaces the tracked glb only after every error-producing stage has passed. Any failure leaves the
tracked glb untouched, because the tracked path is written by one `os.replace` of a file that
already passed everything.

`verify` is the same chain into a temporary directory, ending in an exact comparison with the
tracked glb. It writes nothing.

WHY THE DOOR IS THE ORCHESTRATOR AND NOT A BLENDER SCRIPT
---------------------------------------------------------
Blender's embedded interpreter is where the source pass belongs and nowhere else: the consumer
contract is a Rust binary, the encoder is a minute of `basisu` per texture, and both would run on
Blender's main thread behind a frozen window. So the chain is phases in this file — Blender once,
for the source pass and the raw candidate, and everything after it here.

Each stage prints its OWN report verbatim: the Blender pass, the Rust contract and the derivation
verifier all emit the one finding shape (`scripts/tank/report.py`, `src/bake/report.rs`,
`scripts/tank/glb_ktx2.py`), so the door adds nothing to that vocabulary. Its own rows cover what
only it can see — the pinned toolchain (`scripts/toolchain.py`), a tool that failed mechanically,
and a rebuilt candidate that does not match the tracked bytes.

Exit is non-zero exactly when a stage refused.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from typing import List, Optional

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import report  # noqa: E402  — the paths above are what make these importable
import toolchain  # noqa: E402
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402

MODES = ("lint", "export", "verify")

#: The Blender half, run once per invocation.
SOURCE_PASS = os.path.join(".agents", "blender", "export_tank.py")

#: The texture derivation, and the verifier that reads its output.
ENCODE = os.path.join("scripts", "encode-tank-ktx2.sh")
DERIVATION_VERIFIER = os.path.join("scripts", "tank", "glb_ktx2.py")


# ── the door's own findings ──────────────────────────────────────────────────────────────────────

TOOLCHAIN = Check(
    id="door.toolchain",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="the foreign programs this chain runs are the versions scripts/toolchain.py pins",
)

STAGE_FAILED = Check(
    id="door.stage-failed",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="every derivation stage the door runs completes",
)

CANDIDATE_MISMATCH = Check(
    id="door.candidate-mismatch",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="a candidate rebuilt from the stored source is byte-identical to the tracked glb",
)


class Refused(Exception):
    """A stage said no. `stage` names it; `findings` carries the door's own rows when the stage
    reports prose rather than findings — a stage that emits the one report shape has already
    printed it, and a second row here would say the same thing in a worse place."""

    def __init__(self, stage: str, findings: Optional[List[Finding]] = None):
        super().__init__(stage)
        self.stage = stage
        self.findings = list(findings or ())


# ── running the phases ───────────────────────────────────────────────────────────────────────────

def repo_root(start: Optional[str] = None) -> str:
    """The work tree this door belongs to. Every stage runs from here: the encoder resolves its own
    paths against it, and cargo needs the manifest."""
    directory = os.path.dirname(os.path.abspath(start or __file__))
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], cwd=directory,
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    )
    if result.returncode:
        raise Refused("repo-root")
    return result.stdout.decode().strip()


def shown(command, root: str) -> str:
    """The command as a line someone can re-run, with this work tree's own prefix dropped."""
    return " ".join(
        argument[len(root) + 1:] if argument.startswith(root + os.sep) else argument
        for argument in command
    )


def run_stage(stage: str, command, root: str, stdout=None) -> None:
    """Run one stage from the work tree root, its output going straight to the console. A non-zero
    exit is a `Refused` naming the stage, with no finding of the door's own: the stage that refused
    said why, in its own report."""
    print("door  ▸ {}: {}".format(stage, shown(command, root)), flush=True)
    result = subprocess.run(command, cwd=root, stdout=stdout)
    if result.returncode:
        raise Refused(stage)


def run_tool(stage: str, command, root: str, subject: str, repair: str) -> None:
    """The same, for a stage that reports prose rather than findings — the encoder. Its exit code
    becomes one `door.stage-failed` row so the report still holds every refusal in one shape."""
    print("door  ▸ {}: {}".format(stage, shown(command, root)), flush=True)
    result = subprocess.run(command, cwd=root)
    if not result.returncode:
        return
    raise Refused(stage, [Finding(
        STAGE_FAILED,
        Subject(SubjectKind.DOOR, stage, subject),
        "`{}` exited {}".format(shown(command, root), result.returncode),
        repair,
    )])


def preflight(mode: str):
    """The pinned programs, before anything long runs. Blender for every mode; the texture encoder
    for the two that derive textures. Returns `(findings, blender)`, the second being the binary
    every mode then launches — asked for once, so the chain runs the program the pin was read off."""
    blender = toolchain.blender()
    programs = [blender] if mode == "lint" else [blender, toolchain.basisu()]
    findings = []
    for program in programs:
        mismatch = program.mismatch
        if not mismatch:
            print("door  ▸ toolchain: {} ({})".format(program, program.binary), flush=True)
            continue
        findings.append(Finding(
            TOOLCHAIN,
            Subject(SubjectKind.DOOR, "toolchain", program.name),
            "; ".join(mismatch),
            "install the pinned version, or point {} at it — a program is not a specification, and "
            "a point release of this one moves the bytes of every asset it touches".format(
                toolchain.BLENDER_ENV if program.name == "blender" else toolchain.BASISU_ENV
            ),
        ))
    return (findings, blender.binary)


def digest(path: str) -> str:
    """sha256 of a file, read in blocks: a tank glb is tens of megabytes."""
    hashed = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            hashed.update(block)
    return hashed.hexdigest()


def candidate(work: str, name: str, stem: str, spec: str) -> str:
    """A candidate glb in its own directory, with the spec sheet beside it under the sibling name
    the consumer contract derives. `asset_verify` is handed a model and finds its spec — the pair
    is named by the model alone, here as everywhere."""
    directory = os.path.join(work, name)
    os.makedirs(directory, exist_ok=True)
    shutil.copyfile(spec, os.path.join(directory, stem + ".tank.ron"))
    return os.path.join(directory, stem + ".glb")


def replace(baked: str, glb: str) -> None:
    """Put the certified candidate at the tracked path, atomically. Staged beside the tracked file
    so the rename is within one filesystem, which is what makes it atomic."""
    staging = os.path.join(os.path.dirname(glb) or ".", "." + os.path.basename(glb) + ".door")
    try:
        shutil.copyfile(baked, staging)
        os.replace(staging, glb)
    except OSError as error:
        if os.path.exists(staging):
            os.remove(staging)
        raise Refused("replace", [Finding(
            STAGE_FAILED,
            Subject(SubjectKind.DOOR, "replace", glb),
            str(error),
            "free the space or fix the permissions on {} — the tracked glb is unchanged".format(
                os.path.dirname(glb) or "."
            ),
        )]) from error


def compare(baked: str, glb: str) -> None:
    """Verify's verdict: the candidate this chain just rebuilt against the tracked bytes."""
    if not os.path.isfile(glb):
        raise Refused("compare", [Finding(
            CANDIDATE_MISMATCH,
            Subject(SubjectKind.FILE, glb),
            "the tracked glb does not exist; the rebuilt candidate is {}".format(digest(baked)),
            "run `asset_door.py export` — verify compares against a tracked model, and there is "
            "none here",
        )])
    rebuilt, tracked = digest(baked), digest(glb)
    if rebuilt == tracked:
        print("door  ▸ compare: {} matches the rebuilt candidate ({})".format(glb, tracked),
              flush=True)
        return
    raise Refused("compare", [Finding(
        CANDIDATE_MISMATCH,
        Subject(SubjectKind.FILE, glb),
        "tracked sha256 {}, rebuilt sha256 {}".format(tracked, rebuilt),
        "run `asset_door.py export` and commit the result — the tracked model is not what this "
        "source, this spec sheet and this toolchain produce",
    )])


# ── the chain ────────────────────────────────────────────────────────────────────────────────────

def chain(mode: str, blend: str, spec: str, glb: str, root: str, work: str, blender: str) -> None:
    """Every stage, in order, raising `Refused` at the first one that says no.

    The order is what makes the door cheap to fail: the source pass and the consumer contract both
    run before the MEASURED minute of texture encoding, and the tracked glb is touched after
    everything.
    """
    stem = os.path.splitext(os.path.basename(blend))[0]
    canon = os.path.join(work, "canon.json")
    with open(canon, "wb") as handle:
        run_stage(
            "canon",
            ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", "--canon", spec],
            root, stdout=handle,
        )

    raw = candidate(work, "raw", stem, spec) if mode != "lint" else None
    command = [
        blender, "--background", "--factory-startup", blend,
        "--python", os.path.join(root, SOURCE_PASS), "--",
        "--mode", mode, "--spec", spec, "--glb", glb, "--canon", canon,
    ]
    run_stage("source", command + (["--raw", raw] if raw else []), root)
    if mode == "lint":
        return

    run_stage("consumer (raw)",
              ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", raw], root)

    baked = candidate(work, "baked", stem, spec)
    run_tool(
        "ktx2", [os.path.join(root, ENCODE), raw, baked], root, os.path.basename(baked),
        "read the encoder's output above — the tracked glb is unchanged, and the work directory it "
        "names holds the images it was encoding",
    )
    run_stage("derivation",
              [sys.executable, os.path.join(root, DERIVATION_VERIFIER), "verify", baked], root)
    run_stage("consumer (baked)",
              ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", baked], root)

    if mode == "verify":
        compare(baked, glb)
        return
    replace(baked, glb)
    print("door  ▸ export: {} — {:.1f} MB, sha256 {}".format(
        glb, os.path.getsize(glb) / 1e6, digest(glb)
    ), flush=True)


# ── the command line ─────────────────────────────────────────────────────────────────────────────

def parse(argv: Optional[List[str]] = None):
    parser = argparse.ArgumentParser(prog="asset_door.py", allow_abbrev=False)
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("blend", help="assets/<id>/<id>.blend — the sole model truth")
    parser.add_argument("--spec", help="TEST ONLY: the spec sheet, which otherwise derives from "
                                       "the blend's stem")
    parser.add_argument("--glb", help="TEST ONLY: the tracked model, which otherwise derives from "
                                      "the blend's stem")
    return parser.parse_args(argv)


def door(mode: str, blend: str, spec: Optional[str] = None, glb: Optional[str] = None) -> int:
    """One invocation. Returns the exit code: non-zero exactly when a stage refused."""
    blend = os.path.abspath(blend)
    stem = os.path.splitext(blend)[0]
    spec = os.path.abspath(spec or stem + ".tank.ron")
    glb = os.path.abspath(glb or stem + ".glb")

    findings, blender = preflight(mode)
    stage = "toolchain"
    if not findings:
        try:
            root = repo_root()
            with tempfile.TemporaryDirectory(prefix="asset-door-") as work:
                chain(mode, blend, spec, glb, root, work, blender)
        except Refused as refusal:
            stage, findings = refusal.stage, refusal.findings
        else:
            print("door  ▸ {} certified".format(mode), flush=True)
            return 0

    print(report.render_text(report.sorted_findings(findings)), end="", flush=True)
    print("door  ▸ {} refused at {}{}".format(
        mode, stage, " — {} is unchanged".format(glb) if mode == "export" else "",
    ), flush=True)
    return 1


def main() -> int:
    arguments = parse()
    return door(arguments.mode, arguments.blend, arguments.spec, arguments.glb)


if __name__ == "__main__":
    sys.exit(main())
