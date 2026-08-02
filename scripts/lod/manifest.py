"""Production and derivation: the code that MAKES a manifest and reads numbers back out of it.

Split from `chain.py` deliberately, and the split is load-bearing rather than tidy. This module is
hashed into every manifest's `sources_sha256` (`config.GENERATOR_SOURCES`) because it participates
in producing one — the generator calls `merge_asset_entries` when regenerating a single asset, and
`switch_distance_m` is the projection every recorded switch distance comes out of. A change here can
change what a manifest SAYS, so a manifest cut before that change is stale and must be regenerated.

`chain.py` keeps the verifier, the CLI and the formatters and is deliberately NOT hashed: it can
only change how a manifest is CHECKED, never what it contains, and forcing a twelve-minute Blender
regeneration to reword a failure message is the kind of friction that gets a check switched off.

(An earlier version of this argument put chain.py outside the hash while the generator imported it —
which was simply wrong, and an adversarial review said so: the verifier really was participating in
production. This split is that refutation applied rather than argued with.)
"""

import hashlib
import json
import math
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import config as CONFIG  # noqa: E402


def switch_distance_m(deviation_mm, radius_m, view):
    """D = dev_m * height_px / (2 tan(vfov/2) * budget_px) + radius. The ONE projection (§9).

    Exact, not small-angle. The shortcut `dev_m * height / (vfov * budget)` agrees to 0.06 % at the
    optic and is 5.5 % wrong at the commander FOV, which is precisely the kind of error that hides
    in a narrow reference view and surfaces when someone quotes a wide one.
    """
    denominator = 2.0 * math.tan(float(view["vfov_rad"]) / 2.0) * float(view["budget_px"])
    return (deviation_mm / 1000.0) * float(view["height_px"]) / denominator + radius_m


def screen_footprint_px(bbox_mm, distance_m, view):
    """The asset's projected diameter in reference-view pixels at `distance_m`.

    A property of the GEOMETRY and the distance, so the verifier can re-derive it from the recorded
    bounding box instead of trusting a pixel count the renderer reported about itself. It decides
    whether the rendered-difference gate has anything to look at (`RENDER_GATE["min_footprint_px"]`).
    """
    diameter = math.sqrt(sum((value / 1000.0) ** 2 for value in bbox_mm))
    pixels_per_radian = float(view["height_px"]) / (2.0 * math.tan(float(view["vfov_rad"]) / 2.0))
    return diameter / distance_m * pixels_per_radian


def tile_vfov_rad(render_config, view):
    """The gate tile's FOV that preserves the reference view's pixels-per-radian.

    Production, not diagnostics: it decides what the render gate actually looked at. It lives here
    so the renderer and the verifier compute it from one expression — a duplicated two-line formula
    is a drift waiting to happen, and the verifier's job is to catch drift, not to have some.
    """
    pixels_per_radian = float(view["height_px"]) / (2.0 * math.tan(float(view["vfov_rad"]) / 2.0))
    return 2.0 * math.atan(0.5 * render_config["tile_px"] / pixels_per_radian)


class Tree:
    """Where verification reads its files from: the work tree, or a git revision.

    THE HOOK NEEDS THE SECOND ONE. A pre-push hook that verifies the work tree answers a question
    nobody asked — a dirty-but-coherent tree can bless a completely different commit, and pushing a
    branch that is not `HEAD`, or a tag, or several refs at once, was not covered at all. Reading
    the manifest and the assets out of the REVISION BEING PUSHED is the only version of this check
    that means what its name says.

    LFS pointers are the reason it can be done cheaply. A tracked glb in a commit is a pointer file
    whose `oid sha256:` IS the sha256 of the real bytes — the same number the manifest records — so
    the whole hash comparison works without hydrating a single object.
    """

    def __init__(self, root, rev=None):
        self.root = root
        self.rev = rev

    def read(self, relpath):
        if self.rev is None:
            with open(os.path.join(self.root, relpath), "rb") as handle:
                return handle.read()
        result = subprocess.run(
            ["git", "show", f"{self.rev}:{relpath}"],
            cwd=self.root, capture_output=True, check=False,
        )
        if result.returncode != 0:
            raise FileNotFoundError(f"{relpath} is not in {self.rev}")
        return result.stdout

    def exists(self, relpath):
        try:
            self.read(relpath)
        except FileNotFoundError:
            return False
        return True

    def blob(self, relpath):  # noqa: D401
        """The file's REAL bytes, resolving an LFS pointer to its object. None if unavailable.

        The tracked glbs are LFS pointers, so `read` gives 40 lines of text rather than a mesh. The
        verifier needs the actual bytes to re-derive anything from them, and the object is normally
        right there in the local cache — the same lookup `scripts/hooks/pre-push` already does for
        the tank glb. When it is not (a revision whose objects were never fetched or have been
        pruned), this returns None and the caller REFUSES rather than skipping quietly.
        """
        try:
            raw = self.read(relpath)
        except FileNotFoundError:
            return None
        if not raw.startswith(b"version https://git-lfs.github.com/spec/v1"):
            return raw
        oid = None
        for line in raw.splitlines():
            if line.startswith(b"oid sha256:"):
                oid = line.split(b":", 1)[1].decode().strip()
        if oid is None:
            return None
        git_dir = subprocess.run(
            ["git", "rev-parse", "--git-dir"], cwd=self.root,
            capture_output=True, text=True, check=False,
        ).stdout.strip()
        if not git_dir:
            return None
        if not os.path.isabs(git_dir):
            git_dir = os.path.join(self.root, git_dir)
        path = os.path.join(git_dir, "lfs", "objects", oid[:2], oid[2:4], oid)
        if not os.path.isfile(path):
            return None
        with open(path, "rb") as handle:
            return handle.read()

    def digest(self, relpath):
        """sha256 of the file's real content, resolving an LFS pointer to its recorded oid."""
        blob = self.read(relpath)
        if blob.startswith(b"version https://git-lfs.github.com/spec/v1"):
            for line in blob.splitlines():
                if line.startswith(b"oid sha256:"):
                    return line.split(b":", 1)[1].decode().strip()
            raise ValueError(f"{relpath} is an LFS pointer with no sha256 oid")
        return hashlib.sha256(blob).hexdigest()

    def label(self):
        return f"{self.rev}:{CONFIG.MANIFEST_RELPATH}" if self.rev else CONFIG.MANIFEST_RELPATH


def load(root=None, path=None, rev=None):
    # `root` is explicit for the hook: it runs `scripts/lod` EXTRACTED FROM the revision under
    # test, into a temp directory that is not inside any work tree, so walking up for a `.git`
    # finds nothing. Verifying a commit with a different tree's rules would be the same class of
    # mistake one level up, which is why the scripts come from the revision and the root does not.
    root = root or CONFIG.repo_root()
    tree = Tree(root, rev)
    if rev is None and path is not None:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle), root, tree
    return json.loads(tree.read(CONFIG.MANIFEST_RELPATH).decode()), root, tree


def derive(manifest, view=None):
    """The runtime chain, derived from measured deviations. The only place a threshold is computed.

    Per level: the glb it loads, its triangle count, and the distance at or beyond which it is the
    honest choice. That distance is the WORSE of two bounds (ADR 0033 §4):

      * source-relative: the level's own lie must be under budget,
      * pairwise: the level and the one it replaces may lie in OPPOSITE directions, so their
        separation can reach e_{N-1} + e_N = 1.5 e_N. That separation is the pop, and pricing the
        switch on source-relative deviation alone under-states it by up to a half octave.
    """
    view = view or manifest["ladder"]["reference_view"]
    chains = []
    for asset in manifest["assets"]:
        levels = asset["levels"]
        # THE SLACK IS AN ORIGIN RADIUS, OVER BOTH ADJACENT LEVELS.
        #
        # `VisibilityRange` tests the distance to the entity ORIGIN; the guarantee is about the
        # surface, so the slack must be the farthest any shipped vertex sits FROM THAT ORIGIN — not
        # half the AABB diagonal, which bounds distance from the box centre and is a different point
        # entirely. Measured on the shipped Link: 0.400124 m from the origin against a 0.384004 m
        # half-diagonal, so every switch was landing 16 mm early.
        #
        # And over BOTH levels at the boundary, because either one may be the mesh on screen there;
        # taking the child's alone would under-slack whenever the parent is the bigger shape.
        origin_radius = [
            (level.get("validity") or {}).get("origin_radius_m", asset["source"]["radius_m"])
            for level in levels
        ]
        rows = []
        for index, level in enumerate(levels):
            if level["role"] == "source":
                rows.append({
                    "level": level["level"], "rung": 0, "glb": level["glb"],
                    "node": level.get("node"), "tris": level["tris"],
                    "dev_source_mm": 0.0, "pairwise_mm": None,
                    "switch_m": 0.0, "role": "source",
                })
                continue
            radius = max(origin_radius[index - 1], origin_radius[index])
            from_source = switch_distance_m(level["dev_source_mm_upper"], radius, view)
            from_pairwise = switch_distance_m(level["pairwise_mm_upper"], radius, view)
            rows.append({
                "level": level["level"], "rung": level["rung"], "glb": level["glb"],
                "node": level.get("node"), "tris": level["tris"],
                "e_target_mm": level["e_target_mm"],
                "dev_source_mm": level["dev_source_mm_upper"],
                "pairwise_mm": level["pairwise_mm_upper"],
                "switch_from_source_m": from_source,
                "switch_from_pairwise_m": from_pairwise,
                "switch_m": max(from_source, from_pairwise),
                "origin_radius_m": radius,
                "role": "generated",
            })
        chains.append({
            "asset": asset["name"], "radius_m": max(origin_radius),
            "termination": asset["termination"],
            "right_wall_m": manifest["ladder"]["right_wall_m"], "levels": rows,
        })
    return chains


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


#: Numeric fields every generated level must carry, all of which must be finite. A manifest whose
#: numbers are NaN passed every comparison in the first version of this file: NaN fails every `>`
#: and every `!=` test silently, so a corrupted manifest verified clean.
def asset_provenance(generator):
    """The toolchain fingerprint an asset chain was cut under, taken from a generator block."""
    return {
        "version": generator.get("version"),
        "sources_sha256": generator.get("sources_sha256"),
        "blender": generator.get("blender"),
        "blender_build": generator.get("blender_build"),
        "gltf_exporter": generator.get("gltf_exporter"),
    }


def merge_asset_entries(regenerated, existing, configured_names, provenance, existing_generator):
    """Fold a targeted regeneration back into a full asset list. Returns it, or raises ValueError.

    `--asset` regenerates ONE chain, and a manifest is required to cover every configured asset — so
    writing only the selected one would replace a verifiable manifest with an unverifiable subset,
    and the first anyone would know is verification failing on a corpus nobody touched.

    CARRIED ENTRIES DO NOT INHERIT THE NEW ATTESTATION. The manifest has ONE generator block, so an
    unselected chain carried into a manifest written today would be re-attested to today's generator
    version, source digest, Blender build and exporter — geometry cut by a toolchain that has since
    changed, wearing a certificate that says otherwise. That is a forged provenance, arrived at by
    accident, and it is the exact failure this pipeline's manifest exists to make impossible.

    So a carry-over is only legal when the provenance it was generated under MATCHES the one about
    to be written. When it does not, this refuses and names the difference: the honest answer is a
    full regeneration, which is also the only thing that would make the new certificate true.

    Lives here rather than in the generator because it is manifest shape, not geometry, and because
    here it can be tested without Blender.
    """
    by_name = {entry["name"]: entry for entry in existing}
    fresh = {entry["name"]: entry for entry in regenerated}
    carried_provenance = asset_provenance(existing_generator or {})
    merged = []
    for name in configured_names:
        if name in fresh:
            merged.append(fresh[name])
            continue
        if name not in by_name:
            raise ValueError(
                f"targeted regeneration has no entry for {name!r} and the existing manifest has "
                f"none to carry over — run a full generation"
            )
        differences = [
            f"{key}: was {carried_provenance.get(key)!r}, now {provenance.get(key)!r}"
            for key in provenance
            if carried_provenance.get(key) != provenance.get(key)
        ]
        if differences:
            raise ValueError(
                f"cannot carry {name!r} forward: it was generated under a different toolchain "
                f"({'; '.join(differences)}), and this manifest's single generator block would "
                f"re-attest it to the current one. Run a full generation instead."
            )
        merged.append(by_name[name])
    return merged
