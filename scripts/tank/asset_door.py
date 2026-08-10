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

`verify` is the same chain into a temporary directory, ending in a section-by-section comparison
with the tracked glb — byte-exact wherever the pipeline is deterministic, by stated KTX2 header
facts over the texture payloads, which the encoder cuts differently on different architectures
(`compare`). It writes nothing.

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
import copy
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from typing import List, Optional

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import glb_ktx2  # noqa: E402  — the paths above are what make these importable
import report  # noqa: E402
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
    law="a candidate rebuilt from the stored source and the tracked glb agree section by section: "
        "every non-image bufferView's bytes are identical, the two documents are identical apart "
        "from the bufferViews' byteOffset, the image bufferViews' byteLength and the buffer's "
        "byteLength, and every tracked image payload is a KTX2 declaring the dimensions, level "
        "count, format, supercompression, colour space and per-level uncompressed size its rebuilt "
        "counterpart declares — the encoder's own output bytes are certified at export, by the "
        "machine that cut them",
)

CONTINUATION = Check(
    id="door.continuation",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="a raw candidate the door did not cut itself carries the source pass's continuation token: "
        "written by the pinned toolchain for these exact bytes, cut from this blend against this "
        "spec sheet, and carrying the passing report they earned",
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

def own_env() -> dict:
    """The environment every stage runs in: the caller's, minus git's hook exports. A hook sets
    `GIT_DIR` without `GIT_WORK_TREE`, under which `rev-parse --show-toplevel` answers with the
    subprocess's CWD — so any child asking git a location question inherits the wrong repo."""
    return {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}


def repo_root(start: Optional[str] = None) -> str:
    """The work tree this door belongs to. Every stage runs from here: the encoder resolves its own
    paths against it, and cargo needs the manifest."""
    directory = os.path.dirname(os.path.abspath(start or __file__))
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], cwd=directory, env=own_env(),
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
    result = subprocess.run(command, cwd=root, env=own_env(), stdout=stdout)
    if result.returncode:
        raise Refused(stage)


def run_tool(stage: str, command, root: str, subject: str, repair: str) -> None:
    """The same, for a stage that reports prose rather than findings — the encoder. Its exit code
    becomes one `door.stage-failed` row so the report still holds every refusal in one shape."""
    print("door  ▸ {}: {}".format(stage, shown(command, root)), flush=True)
    result = subprocess.run(command, cwd=root, env=own_env())
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


def digest(path: str) -> str:
    """sha256 of a file, opened once and read in blocks: a tank glb is tens of megabytes."""
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


def identity(status) -> tuple:
    """Which file a stat is about, and which version of it: device and inode name the file, size and
    modification time name its content's generation. Two stats that agree here are of one file that
    nobody has rewritten in between."""
    return (status.st_dev, status.st_ino, status.st_size, status.st_mtime_ns)


# ── the tracked model, section by section, against a candidate rebuilt from its source ───────────

def without_encoder_spans(js: dict, images) -> dict:
    """The document with the fields the encoder's output SIZE moves dropped from it, so that what
    remains can be compared whole. They are every bufferView's byteOffset, an image bufferView's
    byteLength, and the byteLength of the buffer they all live in — the fields the derivation law
    sanctions (`glb_ktx2._bufferviews`, `_buffers`), for the same reason, and no others."""
    document = copy.deepcopy(js)
    for index, view in enumerate(document.get("bufferViews", [])):
        view.pop("byteOffset", None)
        if index in images:
            view.pop("byteLength", None)
    buffers = document.get("buffers", [])
    if buffers:
        buffers[0].pop("byteLength", None)
    return document


def image_views(js: dict) -> set:
    """Every bufferView index this document fills with an image. Read off the CANDIDATE, which was
    cut from the stored source and certified by the derivation verifier a stage ago: which views
    hold textures is a fact about what the source produces, and that the tracked document says the
    same is the `images` row's business, not an input to it."""
    return {image["bufferView"] for image in js.get("images", []) if "bufferView" in image}


def ktx2_facts(parsed) -> dict:
    """What a KTX2 payload IS, as against how many bytes this machine's encoder spent saying it.
    Every fact here is a function of the raster the encoder read and the flags it was handed; a
    level record's offset and compressed length are the encoder's own search, and they are not
    here. MEASURED on one asset cut twice, macOS/arm64 against Linux/x86_64: all nine payloads
    differ in every compressed level length and agree on every fact below."""
    return {
        "vkFormat": parsed.vk_format,
        "typeSize": parsed.type_size,
        "pixelWidth": parsed.width,
        "pixelHeight": parsed.height,
        "pixelDepth": parsed.depth,
        "layerCount": parsed.layers,
        "faceCount": parsed.faces,
        "levelCount": parsed.levels,
        "supercompressionScheme": parsed.supercompression,
        "colourModel": parsed.model,
        "transferFunction": parsed.transfer,
        "uncompressedLevelBytes": [record[2] for record in parsed.records],
    }


REBUILD = ("run `asset_door.py export` and commit the result — the tracked model is not what this "
           "source, this spec sheet and this toolchain produce")


def mismatch(subject: Subject, evidence: str) -> Finding:
    return Finding(CANDIDATE_MISMATCH, subject, evidence, REBUILD)


def section(glb: str, element: str, evidence: str) -> Finding:
    return mismatch(Subject(SubjectKind.FILE, glb, element), evidence)


def stated(value) -> str:
    return json.dumps(value, sort_keys=True)[:120]


def difference(tracked, rebuilt) -> str:
    """How two JSON values differ: the counts when two collections are different lengths, the first
    entry that moved when they are not, the values themselves otherwise."""
    if isinstance(tracked, list) and isinstance(rebuilt, list):
        if len(tracked) != len(rebuilt):
            return "{} entries tracked, {} rebuilt".format(len(tracked), len(rebuilt))
        moved = next(at for at, (x, y) in enumerate(zip(tracked, rebuilt)) if x != y)
        return "{} entries on both sides, and entry {} differs: tracked {}, rebuilt {}".format(
            len(tracked), moved, stated(tracked[moved]), stated(rebuilt[moved]))
    return "tracked {}, rebuilt {}".format(stated(tracked), stated(rebuilt))


def document_sections(tracked: dict, rebuilt: dict, images, glb: str) -> List[Finding]:
    """The two JSON documents, key by key, with the encoder's own spans dropped from both."""
    a, b = without_encoder_spans(tracked, images), without_encoder_spans(rebuilt, images)
    findings = []
    for key in sorted(set(a) | set(b)):
        if (key in a) != (key in b):
            findings.append(section(glb, key, "present in the {} document only".format(
                "tracked" if key in a else "rebuilt")))
        elif a[key] != b[key]:
            findings.append(section(glb, key, difference(a[key], b[key])))
    return findings


def view_sections(tracked, tracked_bin, rebuilt, rebuilt_bin, images, glb: str) -> List[Finding]:
    """Every bufferView that holds no image, by its bytes. These are the exporter's output — the
    same bytes on every machine — so they are compared byte for byte and nothing less. Over the
    views both documents hold: a document with a different number of them said so in a
    `bufferViews` row already, and the ones it does hold are still answerable."""
    views = min(len(tracked.get("bufferViews", [])), len(rebuilt.get("bufferViews", [])))
    diverged = [index for index in range(views) if index not in images
                and glb_ktx2.view_bytes(tracked, tracked_bin, index)
                != glb_ktx2.view_bytes(rebuilt, rebuilt_bin, index)]
    if not diverged:
        return []
    before = glb_ktx2.view_bytes(tracked, tracked_bin, diverged[0])
    after = glb_ktx2.view_bytes(rebuilt, rebuilt_bin, diverged[0])
    return [section(glb, "bufferView {}".format(diverged[0]),
                    "holds no image, and its {} tracked bytes (sha256 {}) are {} rebuilt bytes "
                    "(sha256 {}); {} of {} non-image bufferViews diverge".format(
                        len(before), hashlib.sha256(before).hexdigest(),
                        len(after), hashlib.sha256(after).hexdigest(),
                        len(diverged), views - len(images)))]


def image_sections(tracked, tracked_bin, rebuilt, rebuilt_bin, glb: str) -> List[Finding]:
    """Every image payload, by what its header says it is. The bytes themselves are the encoder's,
    and `basisu` is SIMD-dependent: one raster encoded on two machines is two payloads that decode
    to the same image. So the payload the tracked model carries is read as a KTX2 and held against
    the facts its rebuilt counterpart declares — and that its pixels are the ones this source
    produced is certified at export, where the encoder that cut them ran.

    Over the images both documents hold, for the reason the bufferViews above are.
    """
    images = rebuilt.get("images", [])
    payloads = (glb_ktx2.payloads_in_memory(tracked, tracked_bin),
                glb_ktx2.payloads_in_memory(rebuilt, rebuilt_bin))
    findings = []
    for index in range(min(len(tracked.get("images", [])), len(images))):
        subject = glb_ktx2.image_subject(glb, index, images[index])
        data = payloads[0](index)
        if data is None:
            findings.append(mismatch(subject, "carries no embedded payload"))
            continue
        parsed = glb_ktx2.parse_ktx2(data)
        if parsed is None:
            findings.append(mismatch(subject, "is {} byte(s) that are not a KTX2 file".format(
                data.size)))
            continue
        # Only the tracked side is read for shape: the candidate answered `D.KTX2_MIPS` a stage ago
        # (`derive`), so a candidate that is not a mipped KTX2 is a refusal the door already made.
        facts = (ktx2_facts(parsed), ktx2_facts(glb_ktx2.parse_ktx2(payloads[1](index))))
        differ = [name for name in facts[0] if facts[0][name] != facts[1][name]]
        if differ:
            findings.append(mismatch(subject, "; ".join(
                "{}: tracked {}, rebuilt {}".format(name, facts[0][name], facts[1][name])
                for name in differ
            )))
    return findings


def container_sections(raw: bytes, tracked, tracked_bin, glb: str) -> List[Finding]:
    """The bytes no section above names: the container's declared size, and the alignment padding
    between bufferViews. Their content is what the repack writes on every machine — the file's own
    length, and zero — so they are stated against that rather than against the candidate, whose
    padding follows its own image sizes. Nothing in a glb is then unexamined."""
    findings = []
    declared = struct.unpack_from("<I", raw, 8)[0]
    if declared != len(raw):
        findings.append(section(glb, "container", "the header declares a {} byte file and the file "
                                                  "is {} bytes".format(declared, len(raw))))
    spans = sorted(
        (view.get("byteOffset", 0), view.get("byteOffset", 0) + view.get("byteLength", 0))
        for view in tracked.get("bufferViews", [])
    )
    at = 0
    for start, end in spans + [(len(tracked_bin), len(tracked_bin))]:
        gap = tracked_bin[at:start] if start > at else b""
        if gap.strip(b"\0"):
            findings.append(section(glb, "container",
                                    "the BIN chunk holds {} byte(s) outside every bufferView at "
                                    "{}..{}, and they are not the zero padding the derivation "
                                    "writes".format(len(gap), at, start)))
            break
        at = max(at, end)
    return findings


def divergence(raw: bytes, glb: str, baked: str) -> List[Finding]:
    """Every section in which the tracked model is not what a candidate rebuilt from the stored
    source says it must be. No rows is the verdict `verify` certifies.

    A document neither half can read is one row of its own: `verify` is run by a hook and a CI lane,
    and a traceback there is a refusal nobody can act on.
    """
    try:
        tracked, tracked_bin = glb_ktx2.parse_glb(raw, glb)
        rebuilt, rebuilt_bin = glb_ktx2.read_glb(baked)
        images = image_views(rebuilt)
        findings = document_sections(tracked, rebuilt, images, glb)
        findings += view_sections(tracked, tracked_bin, rebuilt, rebuilt_bin, images, glb)
        findings += image_sections(tracked, tracked_bin, rebuilt, rebuilt_bin, glb)
        return findings + container_sections(raw, tracked, tracked_bin, glb)
    except (SystemExit, ValueError, TypeError, KeyError, IndexError, struct.error) as error:
        return [section(glb, "document", "cannot be read as the glb this law compares: {}".format(
            error))]


def compare(baked: str, glb: str) -> None:
    """Verify's verdict: the candidate this chain just rebuilt against the tracked bytes.

    SECTION BY SECTION, because one of the sections is not a function of the machine that cut it.
    `basisu` selects its UASTC blocks with SIMD, so the same raster encoded on macOS/arm64 and on
    Linux/x86_64 gives two payloads of different lengths that carry the same image (MEASURED: one
    asset cut twice, 9 of 9 image payloads differ, 289 of 289 non-image bufferViews byte-identical).
    A whole-file digest therefore says "not what this source produces" about a candidate that IS
    what this source produces, on any machine but the one that last exported. Every section that is
    deterministic is compared byte for byte; the image payloads are compared by the facts their
    headers declare. Nothing here has a tolerance: a section is either identical or equal in stated
    facts. This is one law with no environment in it, and it runs the same everywhere.

    WHERE THE PIXEL BYTES ARE CERTIFIED, then: at export, by the machine that ran the encoder. That
    machine wrote the tracked path with bytes its own chain had just produced and verified
    (`derive`), and the byte-stable double export proves an encoder is deterministic with itself.
    What travels to another machine is the claim that the tracked payloads are a KTX2 of the same
    image, which is what this comparison re-establishes there.

    The tracked model is OPENED ONCE and the verdict is about the bytes that handle delivered.
    Asking whether the path exists and then opening it are two questions about two moments; so are
    two digests of one pathname. Neither can be made to disagree here, because there is one open
    and the answer is about what came out of it.

    AND THE ANSWER IS ABOUT THE FILE STILL AT THE PATH WHEN IT IS GIVEN. An open handle survives its
    own pathname: another process replacing the tracked model mid-comparison leaves this one hashing
    an inode nothing can reach any more, and `verify` would then certify a path holding bytes it
    never read — the verdict pre-push and CI both act on. So the open file is `fstat`ed, and the
    pathname is `stat`ed again after the comparison: same device, inode, size and mtime, or this is
    a refusal. A precondition of the door rather than a defect of the model, and fail-closed — a
    path that cannot be stated at all is also not one this verdict can describe.
    """
    try:
        tracked_file = open(glb, "rb")  # noqa: SIM115 — closed by the `with` below
        opened = identity(os.fstat(tracked_file.fileno()))
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
        raw = tracked_file.read()
    tracked = hashlib.sha256(raw).hexdigest()
    rebuilt = digest(baked)

    try:
        landed = identity(os.stat(glb))
    except OSError as error:
        landed = str(error)
    if landed != opened:
        raise Refused("compare", [Finding(
            CANDIDATE_MISMATCH,
            Subject(SubjectKind.FILE, glb),
            "the file at this path changed while it was being compared: it was {} and it is now "
            "{}; the bytes read were sha256 {}".format(opened, landed, tracked),
            "run `asset_door.py verify` again with nothing else writing this path — a verdict is "
            "about the model that is there when it is given, and another writer landed one here "
            "mid-comparison",
        )])
    findings = divergence(raw, glb, baked)
    if not findings:
        print("door  ▸ compare: {} matches the rebuilt candidate ({}, rebuilt {})".format(
            glb, tracked, rebuilt), flush=True)
        return
    # The candidate lives in this invocation's temporary directory, which is gone by the time
    # anyone reads the refusal — and on a CI runner, so is the machine. OVERMATCH_DOOR_KEEP names
    # a directory that receives the refused candidate, so the mismatch can be diffed byte by byte.
    kept = os.environ.get("OVERMATCH_DOOR_KEEP")
    note = ""
    if kept:
        os.makedirs(kept, exist_ok=True)
        copied = os.path.join(kept, os.path.basename(glb) + ".rebuilt")
        shutil.copyfile(baked, copied)
        note = "; the rebuilt candidate is kept at {}".format(copied)
    print("door  ▸ compare: {} is not the rebuilt candidate — tracked sha256 {}, rebuilt sha256 "
          "{}{}".format(glb, tracked, rebuilt, note), flush=True)
    raise Refused("compare", findings)


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
    where it is spent: EVERY input the L1 verdict is a function of is re-measured HERE, off the
    files this invocation was handed — the candidate, the blend it claims to come from, the spec
    sheet it was cut against, the pinned toolchain — and the report it carries is re-read rather
    than believed. Any of them disagreeing is not a continuation.

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
    mismatch = toolchain.continuation_mismatch(from_raw, blend, spec)
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
