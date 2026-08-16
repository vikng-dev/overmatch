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

WHAT `verify` DOES. The same door chain, and then: the tracked trio's own coherence — its two
hashes, its staleness field, AND every constructive law its certificate body must satisfy — the
tracked view glb with its certified rung records STRIPPED held against the rebuilt candidate by the
door's own section-by-section comparison, the sim artifact re-derived from the tracked view glb and
compared byte for byte, and `mesh_count` against the meshes the source actually produces. It writes
nothing and it runs no search: a MEASUREMENT is re-derived by rebuilding, and what a verifier can
hold a certificate to for free is its shape, its ordering and its coverage.

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
import config as CONFIG  # noqa: E402
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
#: (source-geometry digest, `search_digest`), so an unchanged mesh cut by an unchanged search is
#: never searched again. The SOURCES are half of that digest; `resolved_ladder` is the other half.
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

#: What a cut chain's record is called inside its own cache directory — `chains.RECORD`, restated
#: here because this half never imports that one (it holds `bpy`).
RECORD = "chain.json"

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

CACHE_CORRUPT = Check(
    id="build.cache-corrupt",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="every chain this build reads out of its cache is a record it can parse and a set of rung "
        "glbs whose bytes hash to what that record recorded",
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


def resolved_ladder() -> dict:
    """Every value the search actually runs under, RESOLVED — not the files that spell them.

    `scripts/lod/config.py` is hashed by `sources_digest`, and hashing it is not enough: the file
    READS `src/map.rs` for the map id and that map's `level.json` for the world's extent, and the
    right wall it derives from them is where every chain terminates. A map whose extent grows moves
    `RIGHT_WALL_M` without touching one byte of config.py, so a fingerprint over the file alone
    would reuse a cached ladder cut for a smaller world and leave `blend_digest` saying the trio is
    current.

    So the fingerprint takes the VALUES. Every constant here is one the ladder, the stop rules or
    the gates consume, and this dict changes exactly when the search's inputs do.
    """
    return {
        "map_id": CONFIG.MAP_ID,
        "world_size_m": CONFIG.WORLD_SIZE_M,
        "right_wall_m": CONFIG.RIGHT_WALL_M,
        "e1_mm": CONFIG.E1_MM,
        "octave": CONFIG.OCTAVE,
        "skip_fraction": CONFIG.SKIP_FRACTION,
        "max_rungs": CONFIG.MAX_RUNGS,
        "reference_view": {key: CONFIG.REFERENCE_VIEW[key]
                           for key in ("vfov_rad", "height_px", "budget_px")},
        "gates": dict(CONFIG.GATES),
        "normal_diagnostic_samples": CONFIG.NORMAL_DIAGNOSTIC_SAMPLES,
        "verdict_nodes_per_square": CONFIG.VERDICT_NODES_PER_SQUARE,
        "verdict_nodes_cap": CONFIG.VERDICT_NODES_CAP,
    }


def ladder_digest() -> str:
    """sha256 over `resolved_ladder`, canonically encoded. Refuses a non-finite constant."""
    return hashlib.sha256(
        json.dumps(resolved_ladder(), sort_keys=True, allow_nan=False).encode()
    ).hexdigest()


def search_digest() -> str:
    """What keys the per-mesh cache: the search's own sources AND the values it runs under."""
    return hashlib.sha256(
        (sources_digest(SEARCH_SOURCES) + ladder_digest()).encode()
    ).hexdigest()


def blend_digest(blend: str, spec: str) -> str:
    """The staleness field: the source, the spec sheet, and every input that can move an artifact.

    A tank rebuilt from an unchanged blend under a changed pipeline is a different tank, and a
    certificate that only hashed the blend would call it current. `ladder_digest` is in it for the
    same reason it keys the cache — a world that grew moves the right wall and therefore the chains,
    while every file this hashes is unchanged.
    """
    digest = hashlib.sha256()
    for path in (blend, spec):
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
    digest.update(sources_digest(PIPELINE_SOURCES).encode())
    digest.update(ladder_digest().encode())
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
    """Move one worker's COMPLETE chains into the cache, as it exits — one rename per chain.

    A chain is a DIRECTORY holding its rung glbs and, written last and atomically, its record. So a
    chain the worker did not finish has no record and is left where it lies, and a chain it did
    finish crosses into the cache in a single `os.replace` of the directory: there is no moment at
    which the cache holds some of a ladder. The worker's own output lives under the cache root, so
    that rename is within one filesystem — across two it is a copy, and a copy can be observed
    half-done.

    A destination holding a RECORD is another build that won the race with an identical chain; the
    loser drops its copy rather than replacing bytes someone may be reading. Record-present is the
    whole winner test: a chain crosses in complete, so nothing that landed is ever recordless, and
    no two workers of one build share a digest — the buckets partition them. So a recordless
    destination is debris (a restored cache, an interrupted delete) and the fresh cut takes its
    place, because a chain nobody can read is one nobody can be reading.
    """
    if not os.path.isdir(out):
        return
    for name in sorted(os.listdir(out)):
        source = os.path.join(out, name)
        if not os.path.isdir(source) or not os.path.isfile(os.path.join(source, RECORD)):
            continue
        destination = os.path.join(cache, name)
        if os.path.isfile(os.path.join(destination, RECORD)):
            shutil.rmtree(source, ignore_errors=True)
            continue
        shutil.rmtree(destination, ignore_errors=True)
        os.replace(source, destination)


def read_record(path: str):
    """`(record, None)`, or `(None, why)` when that path is not a chain record this build can read.

    The probe both halves use: the cut asks it which entries are a MISS, and `cached_record` asks it
    the same question with a refusal attached.
    """
    try:
        with open(path, encoding="utf-8") as handle:
            record = json.load(handle)
    except (OSError, ValueError) as error:
        return None, str(error)
    if not isinstance(record, dict) or not isinstance(record.get("rungs"), list):
        return None, "the record is not a chain: {}".format(type(record).__name__)
    return record, None


def cached_record(cache: str, digest: str) -> dict:
    """One chain's record, or a loud refusal naming the file that cannot be read.

    THE LAST-RESORT TRIPWIRE, not the cache's integrity policy: `cut_chains` deletes and re-cuts
    every entry that fails to read, so what reaches this is an entry the cut itself was supposed to
    produce and did not. It must say WHICH file and how to be rid of it — an uncaught
    `JSONDecodeError` names a line and column of a path nobody printed.
    """
    path = os.path.join(cache, digest, RECORD)
    record, why = read_record(path)
    if record is None:
        raise Refused("chains", [finding(
            CACHE_CORRUPT, Subject(SubjectKind.FILE, path), why,
            "delete {} and build again — the cache is scratch, and a chain that cannot be read is "
            "one that has to be cut again".format(os.path.join(cache, digest)),
        )])
    return record


def cut_chains(root: str, blender: str, candidate: str, digests: List[str],
               sizes: Dict[str, int], cache: str, jobs: int) -> Dict[str, dict]:
    """Every chain, from the cache where one is valid and from Blender where none is.

    One worker per bucket, each its own Blender process. The cache holds a chain's record and its
    rung glbs under (digest, search fingerprint); a worker writes into a private directory and each
    of its COMPLETE chains is harvested as it exits, whether it exited clean or not — a chain that
    finished is a chain that never has to be cut again, and one that did not leaves nothing behind.

    A CORRUPT ENTRY IS A MISS, NEVER A REFUSAL. The cache is scratch under `target/`, and something
    that restores it — CI's — can hand one back with its record gone or unreadable. Every candidate
    is read here, before anything is cut; one that does not read has its whole entry deleted and its
    digest cut again, because a refusal over restored scratch is one every rerun restores.
    """
    os.makedirs(cache, exist_ok=True)
    missing = []
    for digest in digests:
        entry = os.path.join(cache, digest)
        record, why = read_record(os.path.join(entry, RECORD))
        if record is not None:
            continue
        if os.path.exists(entry):
            shutil.rmtree(entry, ignore_errors=True)
            print("build ▸ chains: healed {} — {}".format(entry, why), flush=True)
        missing.append(digest)
    print("build ▸ chains: {} unique source geometries, {} cached, {} to cut".format(
        len(digests), len(digests) - len(missing), len(missing)), flush=True)
    if missing:
        work = tempfile.mkdtemp(prefix=".cutting-", dir=cache)
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
    return {digest: cached_record(cache, digest) for digest in digests}


# ── assembling the trio ──────────────────────────────────────────────────────────────────────────

def assemble(candidate_blob: bytes, rows: List[dict], records: Dict[str, dict], cache: str):
    """The view artifact, the sim artifact and the certificate's chains, from a cut corpus.

    THE PACKING ORDER IS THE REPRESENTATIVE'S NAME, THEN THE RUNG. It reads nothing about how the
    chains were cut, in what order, or by how many workers, which is what makes two cold builds one
    set of bytes.

    EVERY RUNG'S BYTES ARE HELD AGAINST THE RECORD THAT MEASURED THEM. The record carries the
    sha256 the cutting worker took of the file it had just certified; a cache that has been edited,
    half-written or mixed across generations would otherwise pair one rung's `deviation_mm` with
    another rung's geometry, and every hash downstream would faithfully certify the pair.
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
            path = os.path.join(cache, digest, rung["glb"])
            try:
                with open(path, "rb") as handle:
                    blob = handle.read()
            except OSError as error:
                raise Refused("chains", [finding(
                    CACHE_CORRUPT, Subject(SubjectKind.FILE, path), str(error),
                    "delete {} and build again".format(os.path.join(cache, digest)),
                )]) from error
            landed = TRIO.sha256_bytes(blob)
            if landed != rung.get("sha256"):
                raise Refused("chains", [finding(
                    CACHE_CORRUPT, Subject(SubjectKind.FILE, path),
                    "the record measured sha256 {} and these bytes are {}".format(
                        rung.get("sha256"), landed),
                    "delete {} and build again — a rung's certified deviation belongs to the bytes "
                    "the cut measured, and these are different bytes".format(
                        os.path.join(cache, digest)),
                )])
            embedded.append((name, blob))
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
        cache = os.path.join(cache_root(root), search_digest()[:16])
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
