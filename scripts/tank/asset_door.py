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

`--from-raw <candidate.glb>` is the same run continued rather than started: the caller has ALREADY
run the Blender half in a Blender of its own (the GUI adapter, `.agents/blender/addons/
overmatch_export.py`, which is inside one), so the door skips the launch and picks the chain up at
the consumer contract on the candidate it was handed. Documented-internal: every stage after the
raw candidate is the same call in the same order, so this is one implementation reached from two
places and not a second door.

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

CONTINUATION = Check(
    id="door.continuation",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="a raw candidate the door did not cut itself carries the source pass's continuation token: "
        "written for these exact bytes, by the pinned toolchain",
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


def preflight(mode: str, launches_blender: bool = True):
    """The pinned programs, before anything long runs. Blender for every mode that launches one;
    the texture encoder for the two that derive textures. Returns `(findings, blender)`, the second
    being the binary the chain then launches — asked for once, so the chain runs the program the pin
    was read off, and None for a continuation that launches no Blender at all (the caller is inside
    one, and `export_tank.check_exporter` asserted its half of the pin there)."""
    blender = toolchain.blender() if launches_blender else None
    programs = [program for program in (blender, None if mode == "lint" else toolchain.basisu())
                if program is not None]
    findings = []
    for program in programs:
        row = toolchain.finding(program)
        if row is None:
            print("door  ▸ toolchain: {} ({})".format(program, program.binary), flush=True)
            continue
        findings.append(row)
    return (findings, blender.binary if blender is not None else None)


def digest_of(handle) -> str:
    """sha256 of an OPEN file, read in blocks: a tank glb is tens of megabytes.

    Of the bytes this handle delivers, which is the point: a path is a name that can be made to
    mean another file between two opens, and a digest taken twice off one pathname is a digest of
    whatever stood there each time.
    """
    hashed = hashlib.sha256()
    for block in iter(lambda: handle.read(1 << 20), b""):
        hashed.update(block)
    return hashed.hexdigest()


def digest(path: str) -> str:
    """sha256 of a file, opened once."""
    with open(path, "rb") as handle:
        return digest_of(handle)


def candidate(work: str, name: str, stem: str, spec: str) -> str:
    """A candidate glb in its own directory, with the spec sheet beside it under the sibling name
    the consumer contract derives. `asset_verify` is handed a model and finds its spec — the pair
    is named by the model alone, here as everywhere."""
    directory = os.path.join(work, name)
    os.makedirs(directory, exist_ok=True)
    shutil.copyfile(spec, os.path.join(directory, stem + ".tank.ron"))
    return os.path.join(directory, stem + ".glb")


def replace(baked: str, glb: str) -> str:
    """Put the certified candidate at the tracked path, atomically. Returns its sha256.

    THE STAGING NAME IS UNIQUE PER INVOCATION, from `mkstemp`, and it is created in the tracked
    file's own directory so the rename is within one filesystem — which is what makes it atomic. A
    fixed staging name is not merely untidy: two exports running at once open and truncate the same
    file, and the one that renames first leaves the other writing through the renamed inode, now at
    the tracked path. The first would then report success over bytes still being written. With a
    name nobody else can hold, that finding cannot be constructed.

    The digest is of the bytes actually written, taken as they are copied, so the line the door
    prints names the file it landed rather than whatever a second read of that path would find.
    """
    directory = os.path.dirname(glb) or "."
    handle, staging = tempfile.mkstemp(
        prefix="." + os.path.basename(glb) + ".", suffix=".door", dir=directory,
    )
    try:
        hashed = hashlib.sha256()
        with os.fdopen(handle, "wb") as target, open(baked, "rb") as candidate:
            for block in iter(lambda: candidate.read(1 << 20), b""):
                hashed.update(block)
                target.write(block)
        # `mkstemp` creates 0600. A tracked model is an ordinary file, so it gets the mode an
        # ordinary write would have given it.
        mask = os.umask(0)
        os.umask(mask)
        os.chmod(staging, 0o666 & ~mask)
        os.replace(staging, glb)
        return hashed.hexdigest()
    except OSError as error:
        if os.path.exists(staging):
            os.remove(staging)
        raise Refused("replace", [Finding(
            STAGE_FAILED,
            Subject(SubjectKind.DOOR, "replace", glb),
            str(error),
            "free the space or fix the permissions on {} — the tracked glb is unchanged".format(
                directory
            ),
        )]) from error


def compare(baked: str, glb: str) -> None:
    """Verify's verdict: the candidate this chain just rebuilt against the tracked bytes.

    The tracked model is OPENED ONCE and the verdict is about the bytes that handle delivered.
    Asking whether the path exists and then opening it are two questions about two moments; so are
    two digests of one pathname. Neither can be made to disagree here, because there is one open
    and the answer is about what came out of it.
    """
    try:
        tracked_file = open(glb, "rb")  # noqa: SIM115 — closed by the `with` below
    except OSError as error:
        raise Refused("compare", [Finding(
            CANDIDATE_MISMATCH,
            Subject(SubjectKind.FILE, glb),
            "the tracked glb cannot be read ({}); the rebuilt candidate is {}".format(
                error, digest(baked)
            ),
            "run `asset_door.py export` — verify compares against a tracked model, and there is "
            "none here",
        )]) from error
    with tracked_file:
        tracked = digest_of(tracked_file)
    rebuilt = digest(baked)
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

def registry_of(blend: str) -> str:
    """The substance registry this asset is read against — `assets/materials/materials.ron`, beside
    the material library the blend links its substances out of, derived from the trio's layout the
    way every other path here is.

    THE DOOR NAMES IT rather than letting the contract use the one compiled into itself, because
    the registry is DATA: the pre-push lane hydrates a pushed revision's trio and its material
    library, and a gate reading the work tree's substance numbers over another revision's model
    certifies a pair that never existed. The game keeps the compiled-in registry, which is right —
    it ships the two together.
    """
    return os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(blend))), "materials", "materials.ron"
    )


def contract(registry: str, *arguments) -> List[str]:
    """The consumer contract's command line, with the registry it reads substances from."""
    return ["cargo", "run", "--quiet", "--bin", "asset_verify", "--",
            "--registry", registry] + list(arguments)


def canon_file(spec: str, root: str, work: str, registry: str) -> str:
    """The two canonical lists the source pass may not maintain for itself, written where Blender
    can read them. Public because the GUI adapter runs the source pass in its own Blender and needs
    the same file from the same generator."""
    path = os.path.join(work, "canon.json")
    with open(path, "wb") as handle:
        run_stage("canon", contract(registry, "--canon", spec), root, stdout=handle)
    return path


def source_pass(mode: str, blend: str, spec: str, glb: str, canon: str, raw: Optional[str],
                root: str, blender: str) -> None:
    """The Blender half: the L1 source pass, and — for the modes with a chain behind them — the raw
    candidate the rest of it reads."""
    command = [
        blender, "--background", "--factory-startup", blend,
        "--python", os.path.join(root, SOURCE_PASS), "--",
        "--mode", mode, "--spec", spec, "--glb", glb, "--canon", canon,
    ]
    run_stage("source", command + (["--raw", raw] if raw else []), root)


def derive(mode: str, raw: str, stem: str, spec: str, glb: str, root: str, work: str,
           registry: str) -> None:
    """Everything after the raw candidate: the consumer contract on it, the texture derivation, the
    derivation verifier, the contract again on the baked bytes, and the tracked path.

    This is where `--from-raw` joins, so a candidate cut inside somebody's Blender and one cut by
    the door's own launch travel exactly the same stages in exactly the same order.
    """
    run_stage("consumer (raw)", contract(registry, raw), root)

    baked = candidate(work, "baked", stem, spec)
    run_tool(
        "ktx2", [os.path.join(root, ENCODE), raw, baked], root, os.path.basename(baked),
        "read the encoder's output above — the tracked glb is unchanged, and the work directory it "
        "names holds the images it was encoding",
    )
    run_stage("derivation",
              [sys.executable, os.path.join(root, DERIVATION_VERIFIER), "verify", baked], root)
    run_stage("consumer (baked)", contract(registry, baked), root)

    if mode == "verify":
        compare(baked, glb)
        return
    landed = replace(baked, glb)
    print("door  ▸ export: {} — {:.1f} MB, sha256 {}".format(
        glb, os.path.getsize(baked) / 1e6, landed
    ), flush=True)


def chain(mode: str, blend: str, spec: str, glb: str, root: str, work: str, blender: str) -> None:
    """Every stage, in order, raising `Refused` at the first one that says no.

    The order is what makes the door cheap to fail: the source pass and the consumer contract both
    run before the MEASURED minute of texture encoding, and the tracked glb is touched after
    everything.
    """
    stem = os.path.splitext(os.path.basename(blend))[0]
    registry = registry_of(blend)
    canon = canon_file(spec, root, work, registry)
    raw = candidate(work, "raw", stem, spec) if mode != "lint" else None
    source_pass(mode, blend, spec, glb, canon, raw, root, blender)
    if mode == "lint":
        return
    derive(mode, raw, stem, spec, glb, root, work, registry)


def continued(mode: str, blend: str, spec: str, glb: str, root: str, work: str,
              from_raw: str) -> None:
    """The chain picked up at the candidate somebody else's Blender already wrote.

    The raw candidate must exist: the caller's source pass either wrote it or refused, and a missing
    file is that refusal arriving here as silence.

    AND IT MUST BE AUTHENTICATED, before anything reads it. A continuation enters the chain at the
    consumer contract, past every L1 law — so a file of the right shape handed to `--from-raw` would
    be an entrance to the tracked path that no source pass ever looked at, and a model that is
    L2-clean but cut from a source violating an L1-only law would replace the tracked glb. The
    source pass leaves a token beside every candidate it cuts (`scripts/toolchain.py`), and this is
    where it is spent: written for these exact bytes, by the pinned toolchain, or there is no
    continuation here.

    The candidate is then staged into the door's own layout rather than read where it lies, because
    the consumer contract names a pair by the model alone and finds the spec sheet beside it. The
    caller is free to have called its temporary file anything.
    """
    if not os.path.isfile(from_raw):
        raise Refused("from-raw", [Finding(
            STAGE_FAILED,
            Subject(SubjectKind.DOOR, "from-raw", from_raw),
            "no raw candidate at this path",
            "run the source pass that writes it, or drop --from-raw and let the door launch "
            "Blender itself — the chain continues from a candidate, and there is none",
        )])
    mismatch = toolchain.continuation_mismatch(from_raw)
    if mismatch:
        raise Refused("from-raw", [Finding(
            CONTINUATION,
            Subject(SubjectKind.DOOR, "from-raw", from_raw),
            "; ".join(mismatch),
            "run the source pass over the blend, which writes the token beside the candidate it "
            "cuts, or drop --from-raw and let the door launch Blender itself — the chain "
            "continues from a candidate the L1 pass PASSED, not from a file of the right shape",
        )])
    stem = os.path.splitext(os.path.basename(blend))[0]
    print("door  ▸ from-raw: {} — the source pass ran in the caller's Blender".format(
        shown([from_raw], root)
    ), flush=True)
    raw = candidate(work, "raw", stem, spec)
    shutil.copyfile(from_raw, raw)
    derive(mode, raw, stem, spec, glb, root, work, registry_of(blend))


# ── the command line ─────────────────────────────────────────────────────────────────────────────

def parse(argv: Optional[List[str]] = None):
    parser = argparse.ArgumentParser(prog="asset_door.py", allow_abbrev=False)
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("blend", help="assets/<id>/<id>.blend — the sole model truth")
    parser.add_argument("--spec", help="TEST ONLY: the spec sheet, which otherwise derives from "
                                       "the blend's stem")
    parser.add_argument("--glb", help="TEST ONLY: the tracked model, which otherwise derives from "
                                      "the blend's stem")
    parser.add_argument("--from-raw", help="INTERNAL: continue the chain from a raw candidate a "
                                           "caller's own Blender already wrote, instead of "
                                           "launching one")
    arguments = parser.parse_args(argv)
    if arguments.from_raw and arguments.mode == "lint":
        parser.error("--from-raw continues the derivation chain, and lint has none behind it")
    return arguments


def door(mode: str, blend: str, spec: Optional[str] = None, glb: Optional[str] = None,
         from_raw: Optional[str] = None) -> int:
    """One invocation. Returns the exit code: non-zero exactly when a stage refused."""
    blend = os.path.abspath(blend)
    stem = os.path.splitext(blend)[0]
    spec = os.path.abspath(spec or stem + ".tank.ron")
    glb = os.path.abspath(glb or stem + ".glb")

    findings, blender = preflight(mode, launches_blender=from_raw is None)
    stage = "toolchain"
    if not findings:
        try:
            root = repo_root()
            with tempfile.TemporaryDirectory(prefix="asset-door-") as work:
                if from_raw is None:
                    chain(mode, blend, spec, glb, root, work, blender)
                else:
                    continued(mode, blend, spec, glb, root, work, os.path.abspath(from_raw))
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
    return door(arguments.mode, arguments.blend, arguments.spec, arguments.glb,
                arguments.from_raw)


if __name__ == "__main__":
    sys.exit(main())
