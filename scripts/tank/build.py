"""build.py — the tank build: one command per tank, three artifacts (ADR 0035).

    python3 scripts/tank/build.py build  assets/<id>/<id>.blend
    python3 scripts/tank/build.py verify assets/<id>/<id>.blend
    python3 scripts/tank/build.py lint   assets/<id>/<id>.blend

A `.blend` goes in; `<id>.glb`, `<id>.sim.glb` and `<id>.lod.json` come out. The asset door
(`scripts/tank/asset_door.py`) is this build's CERTIFICATION STEP rather than a second entrance: it
cuts the candidate from the stored source, runs the consumer contract, the texture derivation and
the contract again, and hands back bytes nothing has published yet. Every one of its checks is
still the law, stated once, where it always was.

WHAT `build` DOES, IN ORDER
--------------------------
1. the door's chain, ending at a certified candidate in this invocation's work directory;
2. a census of that candidate's primitives — the seam is the PRIMITIVE, so a multi-material object
   is several chains and not a refusal — grouped by SOURCE-GEOMETRY DIGEST, so geometry that ships
   twice is searched once;
3. per digest, a chain cut by `scripts/tank/chains.py` inside Blender, or the one a previous build
   left in the cache under (digest, search fingerprint);
4. the rungs appended to the candidate as additional mesh records — the view artifact;
5. the byte-strip of that — the sim artifact;
6. the five-field certificate;
7. a STAGED publish: both binaries land, and the certificate last, so an interruption leaves a
   certificate naming bytes that are not there and every reader says so.

WHAT `verify` DOES. The same door chain, and then: the tracked trio's own coherence, the tracked
view glb with its certified rung records STRIPPED held against the rebuilt candidate by the door's
own section-by-section comparison, the sim artifact re-derived from the tracked view glb and
compared byte for byte, and `mesh_count` against the meshes the source actually produces. It writes
nothing and it runs no search — the certificate is what carries the measurements forward.

Exit is non-zero exactly when a stage refused.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Dict, List, Optional

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import asset_door  # noqa: E402  — the paths above are what make these importable
import glb_ktx2  # noqa: E402
import measure  # noqa: E402
import report  # noqa: E402
import toolchain  # noqa: E402
import trio as TRIO  # noqa: E402
from asset_door import Refused  # noqa: E402
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402

MODES = ("build", "verify", "lint")

#: The Blender half of the LOD stage.
CHAINS = os.path.join("scripts", "tank", "chains.py")

#: What can move a RUNG'S BYTES given a source geometry. The per-mesh cache is keyed by
#: (source-geometry digest, this digest), so an unchanged mesh cut by an unchanged search is never
#: searched again.
#:
#: `trio.py` is deliberately not in it: the only thing it decides here is what the source SURFACE
#: is, and the digest that keys the cache is taken over that surface — a decode that changed would
#: change the key by identity rather than by a fingerprint anyone has to remember to include.
SEARCH_SOURCES = (
    "chains.py", "../lod/config.py", "../lod/measure.py", "../lod/generate.py", "../toolchain.py",
)

#: What can move ANY of the three artifacts. `blend_digest` is taken over the source, the spec sheet
#: and this, which is what makes it the staleness check ADR 0035 asks for.
PIPELINE_SOURCES = SEARCH_SOURCES + (
    "build.py", "trio.py", "asset_door.py", "glb_ktx2.py", "../encode-tank-ktx2.sh",
    "../../.agents/blender/export_tank.py",
)

#: Where a cut chain is kept between builds. Under `target/`, which is not tracked.
CACHE_RELPATH = os.path.join("target", "tank-build")
CACHE_ENV = "OVERMATCH_TANK_BUILD_CACHE"

#: How many chains are cut at once by default. MEASURED (ADR 0035's YAGNI condition, satisfied by
#: measurement): the serial cold tiger is over an hour, one primitive of it — the 13 171-triangle,
#: 8 029 mm, 95-component `Hull_Decor#1` the widened seam admits — being most of that. Eight
#: workers on a 4P+6E machine hold the wall clock at that one chain's own cost.
#:
#: IT IS NOT A CORRECTNESS LEVER. The buckets are LPT over triangle counts and the assembly order is
#: the representative chain's name, so the trio is the same bytes at any job count; `test_build.py`
#: proves both halves of that.
DEFAULT_JOBS = min(8, os.cpu_count() or 1)


# ── the build's own findings ─────────────────────────────────────────────────────────────────────

TRIO_INCOHERENT = Check(
    id="build.trio-incoherent",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="the certificate names the bytes beside it: view_glb_sha and sim_glb_sha hash the tracked "
        "binaries, blend_digest is this source and this pipeline, mesh_count is the number of "
        "meshes the source produces, and every rung a chain names is a mesh record in the view "
        "artifact no scene node references",
)

SIM_NOT_DERIVED = Check(
    id="build.sim-not-derived",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="the sim artifact is the byte-strip of the certified view artifact — rung-0 geometry and "
        "material names, no textures, no UVs, no rungs — and its geometry accessors carry the view "
        "artifact's own bytes",
)


def finding(check: Check, subject: Subject, evidence: str, repair: str) -> Finding:
    return Finding(check, subject, evidence, repair)


REBUILD = ("run `scripts/tank/build.py build` and commit the trio — the tracked artifacts are not "
           "what this source, this spec sheet and this toolchain produce")


# ── the tree ─────────────────────────────────────────────────────────────────────────────────────

def sources_digest(names) -> str:
    """sha256 over a fixed list of pipeline sources, path and bytes, in the order given."""
    here = os.path.dirname(os.path.abspath(__file__))
    digest = hashlib.sha256()
    for name in names:
        with open(os.path.join(here, name), "rb") as handle:
            digest.update(name.encode())
            digest.update(handle.read())
    return digest.hexdigest()


def blend_digest(blend: str, spec: str) -> str:
    """The staleness field: the source, the spec sheet, and every source that can move an artifact.

    A tank rebuilt from an unchanged blend under a changed pipeline is a different tank, and a
    certificate that only hashed the blend would call it current.
    """
    digest = hashlib.sha256()
    for path in (blend, spec):
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
    digest.update(sources_digest(PIPELINE_SOURCES).encode())
    for _key, value in toolchain.ENVIRONMENT:
        digest.update(value.encode())
    return digest.hexdigest()


def cache_root(root: str) -> str:
    return os.environ.get(CACHE_ENV) or os.path.join(root, CACHE_RELPATH)


# ── the stages, timed ────────────────────────────────────────────────────────────────────────────

class Timeline:
    """Every stage's wall clock, in the order they ran. Printed whether the build lands or refuses:
    a build that refused at minute nine is the number that decides whether a pool is due."""

    def __init__(self):
        self.rows: List[tuple] = []
        self.started = time.monotonic()

    def stage(self, name: str):
        timeline = self

        class Span:
            def __enter__(self):
                self.at = time.monotonic()
                print("build ▸ {}".format(name), flush=True)
                return self

            def __exit__(self, *_):
                timeline.rows.append((name, time.monotonic() - self.at))
                return False

        return Span()

    def render(self) -> str:
        width = max([len(name) for name, _ in self.rows] + [5])
        lines = ["timings ▸ stage wall clock"]
        for name, seconds in self.rows:
            lines.append("  {}  {:>8.1f} s".format(name.ljust(width), seconds))
        lines.append("  {}  {:>8.1f} s".format("TOTAL".ljust(width),
                                               time.monotonic() - self.started))
        return "\n".join(lines) + "\n"


# ── the LOD stage ────────────────────────────────────────────────────────────────────────────────

def partition(digests: List[str], sizes: Dict[str, int], jobs: int) -> List[List[str]]:
    """Digests split across `jobs` workers, largest first into the emptiest bucket.

    Deterministic in the input, and the assembly order does not read it: chains are embedded in
    representative order, so which worker cut which chain cannot move a byte of the trio.
    """
    buckets: List[List[str]] = [[] for _ in range(max(1, jobs))]
    load = [0] * len(buckets)
    for digest in sorted(digests, key=lambda d: (-sizes.get(d, 0), d)):
        index = load.index(min(load))
        buckets[index].append(digest)
        load[index] += sizes.get(digest, 0)
    return [bucket for bucket in buckets if bucket]


def harvest(out: str, cache: str) -> None:
    """Move one worker's COMPLETE chains into the cache, as it exits.

    A chain's record is written after its last rung glb, so a digest with no record is a chain the
    worker did not finish — its rung files are left in the work directory and the next build cuts
    it again, rather than the cache holding half a ladder nothing will ever notice.
    """
    if not os.path.isdir(out):
        return
    complete = {name[:-len(".json")] for name in os.listdir(out) if name.endswith(".json")}
    for name in sorted(os.listdir(out)):
        if name.split(".", 1)[0] in complete:
            shutil.move(os.path.join(out, name), os.path.join(cache, name))


def cut_chains(root: str, blender: str, candidate: str, digests: List[str],
               sizes: Dict[str, int], cache: str, jobs: int) -> Dict[str, dict]:
    """Every chain, from the cache where one is valid and from Blender where none is.

    One worker per bucket, each its own Blender process. The cache holds a chain's record and its
    rung glbs under (digest, search fingerprint); a worker writes into a private directory and each
    of its COMPLETE chains is harvested as it exits, whether it exited clean or not — a chain that
    finished is a chain that never has to be cut again, and one that did not leaves nothing behind.
    """
    os.makedirs(cache, exist_ok=True)
    missing = [d for d in digests if not os.path.isfile(os.path.join(cache, d + ".json"))]
    print("build ▸ chains: {} unique source geometries, {} cached, {} to cut".format(
        len(digests), len(digests) - len(missing), len(missing)), flush=True)
    if missing:
        work = tempfile.mkdtemp(prefix="tank-chains-")
        try:
            buckets = partition(missing, sizes, jobs)
            running = []
            for index, bucket in enumerate(buckets):
                out = os.path.join(work, str(index))
                command = [
                    blender, "--background", "--factory-startup",
                    "--python", os.path.join(root, CHAINS), "--",
                    "--glb", candidate, "--out", out,
                ]
                for digest in bucket:
                    command += ["--select", digest]
                print("build ▸ chains: worker {} cuts {} chain(s)".format(index, len(bucket)),
                      flush=True)
                running.append((
                    subprocess.Popen(command, cwd=root, env=asset_door.own_env()), out, bucket,
                ))
            refused = []
            pending = list(running)
            while pending:
                for entry in list(pending):
                    process, out, bucket = entry
                    if process.poll() is None:
                        continue
                    pending.remove(entry)
                    if process.returncode:
                        refused.append(bucket)
                    harvest(out, cache)
                if pending:
                    time.sleep(0.5)
            if refused:
                raise Refused("chains")
        finally:
            shutil.rmtree(work, ignore_errors=True)
    records = {}
    for digest in digests:
        with open(os.path.join(cache, digest + ".json"), encoding="utf-8") as handle:
            records[digest] = json.load(handle)
    return records


# ── assembling the trio ──────────────────────────────────────────────────────────────────────────

def assemble(candidate_blob: bytes, rows: List[dict], records: Dict[str, dict], cache: str):
    """The view artifact, the sim artifact and the certificate's chains, from a cut corpus.

    THE PACKING ORDER IS THE REPRESENTATIVE'S NAME, THEN THE RUNG. It reads nothing about how the
    chains were cut, in what order, or by how many workers, which is what makes two cold builds one
    set of bytes.
    """
    groups = TRIO.chains_by_digest(rows)
    representative = {digest: keys[0] for digest, keys in groups.items()}
    order = sorted(groups, key=lambda digest: representative[digest])
    embedded = []
    mesh_names: Dict[str, List[dict]] = {}
    for digest in order:
        rungs = []
        for rung in records[digest]["rungs"]:
            name = "{}_LOD{}".format(representative[digest], rung["rung"])
            with open(os.path.join(cache, rung["glb"]), "rb") as handle:
                embedded.append((name, handle.read()))
            rungs.append({"mesh": name, "deviation_mm": rung["deviation_mm"]})
        mesh_names[digest] = rungs

    view_blob, mesh_count = TRIO.embed_rungs(candidate_blob, embedded)
    chains = {}
    for digest, keys in groups.items():
        if not mesh_names[digest]:
            continue
        record = records[digest]
        radius = max(
            [record["source"]["origin_radius_m"]]
            + [rung["origin_radius_m"] for rung in record["rungs"]]
        )
        for key in keys:
            chains[key] = {"radius_m": round(radius, 6), "rungs": mesh_names[digest]}
    return view_blob, mesh_count, chains


# ── the modes ────────────────────────────────────────────────────────────────────────────────────

def staged_candidate(blend: str, spec: str, glb: str, root: str, work: str, blender: str) -> str:
    """The door's certified candidate, in this invocation's work directory and published nowhere.

    Every check the door makes has run by the time this returns; what has NOT happened is the
    replacement of the tracked model, which is this build's to do once the rungs are in.

    The door's CHAIN is driven rather than its wrapper, so a refusal arrives as the door's own
    `Refused` — its stage and its findings — and this build's verdict line names the stage that
    actually said no instead of naming the door.
    """
    out = os.path.join(work, os.path.basename(glb))
    asset_door.chain("export", blend, spec, glb, root, work, blender, stage_to=out)
    return out


def build(blend: str, spec: str, glb: str, root: str, work: str, blender: str,
          jobs: int, timeline: Timeline) -> int:
    with timeline.stage("door (export chain)"):
        candidate = staged_candidate(blend, spec, glb, root, work, blender)
    with open(candidate, "rb") as handle:
        candidate_blob = handle.read()

    with timeline.stage("census"):
        gltf, binary = measure.glb_chunks_from_bytes(candidate_blob, candidate)
        rows = TRIO.census(gltf, binary)
        groups = TRIO.chains_by_digest(rows)
        sizes = {row["digest"]: row["tris"] for row in rows if "digest" in row}
        for row in rows:
            if "refusal" in row:
                print("build ▸ census: {} carries no chain — {}".format(
                    row["chain"], row["refusal"]), flush=True)
        print("build ▸ census: {} primitive(s) over {} mesh(es), {} unique source geometr(ies), "
              "{} triangles".format(len(rows), len(gltf.get("meshes", [])), len(groups),
                                    sum(sizes.values())), flush=True)

    with timeline.stage("chains (directed search)"):
        cache = os.path.join(cache_root(root), sources_digest(SEARCH_SOURCES)[:16])
        records = cut_chains(root, blender, candidate, sorted(groups), sizes, cache, jobs)

    with timeline.stage("assemble"):
        view_blob, mesh_count, chains = assemble(candidate_blob, rows, records, cache)
        sim_blob = TRIO.sim_bytes(view_blob, mesh_count)
        cert = TRIO.certificate(
            blend_digest(blend, spec), TRIO.sha256_bytes(view_blob), TRIO.sha256_bytes(sim_blob),
            mesh_count, chains,
        )

    with timeline.stage("publish"):
        view_path, sim_path, cert_path = TRIO.publish(glb, view_blob, sim_blob, cert)
    for path in (view_path, sim_path, cert_path):
        print("build ▸ published {} — {:.1f} MB, sha256 {}".format(
            os.path.relpath(path, root), os.path.getsize(path) / 1e6, TRIO.sha256_file(path),
        ), flush=True)
    print("build ▸ {} chain(s) over {} unique source geometr(ies); {} rung mesh record(s) "
          "embedded, {} rung reference(s) certified".format(
              len(chains), len(groups),
              sum(len(records[digest]["rungs"]) for digest in groups),
              sum(len(chain["rungs"]) for chain in chains.values()),
          ), flush=True)
    return 0


def verify(blend: str, spec: str, glb: str, root: str, work: str, blender: str,
           timeline: Timeline) -> int:
    view_path, sim_path, cert_path = TRIO.paths(glb)
    findings: List[Finding] = []
    for path in (view_path, sim_path, cert_path):
        if not os.path.isfile(path):
            raise Refused("trio", [finding(
                TRIO_INCOHERENT, Subject(SubjectKind.FILE, os.path.relpath(path, root)),
                "the trio is incomplete: this artifact is not here",
                REBUILD,
            )])
    with open(view_path, "rb") as handle:
        view_blob = handle.read()
    with open(sim_path, "rb") as handle:
        sim_blob = handle.read()
    with open(cert_path, "rb") as handle:
        cert = json.loads(handle.read().decode())

    with timeline.stage("certificate"):
        failures = TRIO.coherence(cert, view_blob, sim_blob, blend_digest(blend, spec))
        findings += [
            finding(TRIO_INCOHERENT, Subject(SubjectKind.FILE, os.path.relpath(cert_path, root)),
                    text, REBUILD)
            for text in failures
        ]
    if findings:
        raise Refused("certificate", findings)

    with timeline.stage("sim artifact"):
        rebuilt = TRIO.sim_bytes(view_blob, cert["mesh_count"])
        if rebuilt != sim_blob:
            findings.append(finding(
                SIM_NOT_DERIVED, Subject(SubjectKind.FILE, os.path.relpath(sim_path, root)),
                "the sim artifact is {} byte(s) and the strip of the tracked view artifact is {} "
                "byte(s)".format(len(sim_blob), len(rebuilt)),
                REBUILD,
            ))
        else:
            view_geometry = TRIO.geometry_payloads(view_blob, cert["mesh_count"])
            sim_geometry = TRIO.geometry_payloads(sim_blob, cert["mesh_count"])
            moved = sorted(
                key for key in view_geometry
                if view_geometry[key] != sim_geometry.get(key)
            )
            if moved:
                findings.append(finding(
                    SIM_NOT_DERIVED, Subject(SubjectKind.FILE, os.path.relpath(sim_path, root)),
                    "{} geometry accessor(s) hold different bytes on the two sides, the first "
                    "being {}".format(len(moved), moved[0]),
                    REBUILD,
                ))
    if findings:
        raise Refused("sim artifact", findings)

    with timeline.stage("door (export chain)"):
        candidate = staged_candidate(blend, spec, glb, root, work, blender)

    with timeline.stage("compare"):
        # THE TRACKED FILE'S OWN CONTAINER, before anything is taken out of it. `strip_rungs`
        # re-serializes canonically, so framing, an extra chunk or non-canonical JSON/padding would
        # be laundered by the comparison below — the door's container law is asked here, of the
        # bytes on disk.
        label = os.path.relpath(view_path, root)
        view_js, view_bin = glb_ktx2.parse_glb(view_blob, label)
        findings += asset_door.container_sections(view_blob, view_js, view_bin, label)
        stripped = TRIO.strip_rungs(view_blob, cert["mesh_count"])
        source_js, _ = glb_ktx2.read_glb(candidate)
        if len(source_js.get("meshes", [])) != cert["mesh_count"]:
            findings.append(finding(
                TRIO_INCOHERENT, Subject(SubjectKind.FILE, os.path.relpath(cert_path, root)),
                "mesh_count is {} and the source produces {} mesh(es)".format(
                    cert["mesh_count"], len(source_js.get("meshes", []))),
                REBUILD,
            ))
        findings += asset_door.divergence(stripped, os.path.relpath(view_path, root), candidate)
    if findings:
        raise Refused("compare", findings)
    print("build ▸ compare: {} carries the rebuilt candidate under {} chain(s) naming {} rung(s)"
          .format(os.path.relpath(view_path, root), len(cert["chains"]),
                  sum(len(chain["rungs"]) for chain in cert["chains"].values())), flush=True)
    return 0


# ── the command line ─────────────────────────────────────────────────────────────────────────────

def parse(argv: Optional[List[str]] = None):
    parser = argparse.ArgumentParser(prog="build.py", allow_abbrev=False)
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("blend", help="assets/<id>/<id>.blend — the sole model truth")
    parser.add_argument("--jobs", type=int, default=DEFAULT_JOBS,
                        help="how many Blender processes cut chains at once; the assembly order "
                             "does not read it, so the trio is the same bytes at any value")
    parser.add_argument("--spec", help="TEST ONLY: the spec sheet, which otherwise derives from "
                                       "the blend's stem")
    parser.add_argument("--glb", help="TEST ONLY: the tracked model, which otherwise derives from "
                                      "the blend's stem")
    return parser.parse_args(argv)


def run(mode: str, blend: str, spec: Optional[str] = None, glb: Optional[str] = None,
        jobs: int = 1) -> int:
    """One invocation. Returns the exit code: non-zero exactly when a stage refused."""
    blend = os.path.abspath(blend)
    stem = os.path.splitext(blend)[0]
    spec = os.path.abspath(spec or stem + ".tank.ron")
    glb = os.path.abspath(glb or stem + ".glb")
    if mode == "lint":
        return asset_door.door("lint", blend, spec, glb)

    timeline = Timeline()
    findings, blender = asset_door.preflight(mode)
    stage = "toolchain"
    if not findings:
        try:
            root = asset_door.repo_root()
            with tempfile.TemporaryDirectory(prefix="tank-build-") as work:
                if mode == "build":
                    build(blend, spec, glb, root, work, blender, jobs, timeline)
                else:
                    verify(blend, spec, glb, root, work, blender, timeline)
        except Refused as refusal:
            stage, findings = refusal.stage, refusal.findings
        else:
            print(timeline.render(), end="", flush=True)
            print("build ▸ {} certified".format(mode), flush=True)
            return 0

    print(report.render_text(report.sorted_findings(findings)), end="", flush=True)
    print(timeline.render(), end="", flush=True)
    print("build ▸ {} refused at {}{}".format(
        mode, stage, " — the tracked trio is unchanged" if mode == "build" else "",
    ), flush=True)
    return 1


def main() -> int:
    arguments = parse()
    return run(arguments.mode, arguments.blend, arguments.spec, arguments.glb, arguments.jobs)


if __name__ == "__main__":
    sys.exit(main())
