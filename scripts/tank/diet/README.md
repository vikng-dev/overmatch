# `scripts/tank/diet/` — the tools that put the Tiger on a geometry diet

Twenty small scripts that decimated the track shoe, deduplicated the machine guns and turned
back-face culling on, plus the instruments that proved each one safe. They exist because the
assets they produced are **binary and otherwise unreproducible**: `assets/tiger_1/tiger_1.glb`
is a tracked git-lfs blob whose `Link` and `Object_0.002` meshes are no longer anything a
human authored, and the `.blend` they came from is untracked and 140 MB.

Nothing here runs in CI or in a hook. They are hand tools.

## THE ONE RULE: never re-export this glb from Blender to apply a mesh change

`tiger_1.glb` embeds 59.6 MB of mipped UASTC KTX2. Blender's glTF exporter writes PNG, and
bevy's PNG loader produces a texture with exactly ONE mip level — shimmer on every rivet at
combat range. That is the failure `scripts/encode-tank-ktx2.sh` and the `verify` gate in
`scripts/tank/glb_ktx2.py` exist to prevent, and it is why every edit in this directory is
**binary surgery on the glb** instead of a round trip. Blender is used only to compute new
geometry, in isolation, on a stripped one-mesh file.

If you re-export the tank for an actual art change, that is fine and correct — the export
helper in `.agents/blender/` bakes the KTX2 back in. What is not fine is re-exporting to
apply one of the edits below, because the bake is 60 s and the mistake is silent.

After ANY edit here, both of these must still pass:

    python3 scripts/tank/glb_ktx2.py verify assets/tiger_1/tiger_1.glb
    python3 scripts/tank/diet/validate.py assets/tiger_1/tiger_1.glb

## The tools

### glb surgery (pure Python, no dependencies)

| tool | what it does |
|---|---|
| `glblib.py` | GLB chunk reader/writer and accessor decoder. Every other pure-Python tool imports it. Owns `JSON_PAD` — see below. |
| `inject.py` | Replace one primitive's POSITION/NORMAL/TEXCOORD_0/indices from a geometry-only glb, in place. Mesh name, mesh index, the node above it and the material binding are untouched. Takes a mesh index OR a mesh name. |
| `dedupe.py` | Point named nodes at a shared mesh, proving the geometry is identical first — every decoded attribute element, every index and the effective material, not the counts — then garbage-collect the meshes and materials that orphans (with full index remapping). `dedupe.py selftest` proves the check both ways. |
| `singlesided.py` | Drop `doubleSided` from materials, optionally sparing named ones. |
| `extract.py` | Pull one mesh primitive out into a minimal geometry-only glb — the file Blender is allowed to touch. |
| `extract_all.py` | Pull EVERY visible primitive out with its world transform baked, one mesh per primitive named `<node>::<material>`. Applies the same visibility rules `src/` does. `--nodes`, `--only`, `--with-link`, `SHOW_HIDDEN=1`. |
| `rename.py` | Rename the mesh and node of a one-mesh glb and re-emit it compactly. |
| `dumpimg.py` | Write embedded glb images out by index (the KTX2 that `basisu -unpack` then turns into PNG for textured renders). |

### instruments — the things that decided the changes

| tool | what it answers |
|---|---|
| `report.py` | Triangles and vertices PER TANK, using `src/`'s own visibility rules and `link_count` x 2. This is the number the commits quote. |
| `validate.py` | Does every accessor, bufferView, index, material and texture reference still resolve, and is every view in bounds. |
| `probe.py` | What triangle count can the collapse decimator actually REACH on this mesh, and how many shells and boundary edges are holding it there. |
| `thin.py` | Which primitives are open shells or single-layer sheets — the first-pass shortlist for the culling question. Advisory only; `backface.py` is what decides. |
| `backface.py` | Renders the subject with every visible BACK FACE emitting red. A red pixel is a pixel that back-face culling turns into a hole. |
| `drive_bf.py` | Runs `backface.py` over 32 camera positions and counts red pixels per glb. Fails closed: a nonzero Blender exit, a missing frame or a frame that is not `RES` x `RES` aborts with Blender's stderr and prints no total. |

### renders for review

| tool | what it does |
|---|---|
| `render.py` | Headless turntable of a geometry-only glb: textured, clay, or clay with a wireframe overlay. Light rig scales with the subject. |
| `drive_render.py` | Runs `render.py` over several variants in both modes. |
| `sheet.py` | Composes labelled contact sheets (PIL). `.jpg` output is ~5x smaller than `.png` for these and visually identical, which matters when the sheet is committed. |
| `redcount.py` | Standalone red-pixel counter for a glob of `backface.py` frames. |

### the two decimators

`decimate2.py` adds a planar-dissolve pass before the collapse, on the theory that flat plate
faces should not spend budget. It does not pay off at a tight budget — on the shoe it returns
1 009 vertices for 502 triangles against `decimate.py`'s 993 for 518 — and it does not lift
the MG barrel's floor either. It is kept because it built the 964-triangle shoe alternative,
and because the negative result is worth not re-deriving.

`decimate.py` is the one that shipped: import with `merge_vertices`, weld at 1e-5, triangulate,
binary-search a quadric-collapse ratio to a triangle budget, then rebuild hard edges from a
dihedral angle. The collapse decimator drops custom split normals, so re-shading is not
optional — and the angle is a real lever: 30 deg gives 993 verts, 12 deg gives 1 303 for the
same 518 triangles and slightly crisper panels. 30 was chosen because the audit names vertex
fetch as a co-bottleneck.

## Reproducing the shipped assets

Requires Blender 5.1 (`BLENDER=` env var, else `blender` on PATH), Python 3 with Pillow for
the render tools only, and `basisu` for textured renders. Run from the repo root. `$W` is a
scratch directory.

Verified 2026-08-01 on Blender 5.1.2: re-running steps 1 and 2 reproduces all three shipped
shoe meshes — 518, 194 and 964 triangles — with **bit-identical vertex positions**. The
decimation is deterministic; the recipe below is the asset's real source.

### 1 — track shoe, 5 552 -> 518 triangles

    python3 scripts/tank/diet/extract.py assets/tiger_1/tiger_1.glb 1 $W/shoe_src.glb
    blender -b -P scripts/tank/diet/decimate.py -- $W/shoe_src.glb $W/shoe_lod0.glb 520 30
    python3 scripts/tank/diet/inject.py assets/tiger_1/tiger_1.glb 1 $W/shoe_lod0.glb

Mesh 1 is `Link`. It is view-only: `src/track/marker_model.rs` measures the track from
`Link_Box`, the pin-marker empties and the sprocket/idler/wheel meshes, and its
`REQUIRED_MESHES` does not include the shoe. Support is the contact envelope and grip is
per-element analytic, so nothing samples this mesh.

### 2 — LOD1 and the 964-triangle alternative

    blender -b -P scripts/tank/diet/decimate.py -- $W/shoe_src.glb $W/lod1.glb 200 30
    python3 scripts/tank/diet/rename.py $W/lod1.glb assets/tiger_1/tiger_1_link.lod1.glb Link_LOD1

    blender -b -P scripts/tank/diet/decimate2.py -- $W/shoe_src.glb $W/a964.glb 1000 30 1.0
    python3 scripts/tank/diet/rename.py $W/a964.glb assets/tiger_1/tiger_1_link.alt964.glb Link

`tiger_1_link.alt964.glb` is the candidate that keeps the pin bosses. Swapping it in is one
`inject.py` call with mesh name `Link`; that trade is Yan's, not this directory's.

### 3 — machine guns: dedupe, THEN decimate

Order matters. `dedupe.py` proves the two meshes are geometrically identical before it
repoints anything, so decimating first makes that check fail — correctly.

"Identical" is a data comparison, not a count comparison: every POSITION, NORMAL and UV
element and every index of every primitive is decoded and hashed, and the two primitives
must resolve to the same effective material. Decoding is what makes it usable — a duplicate
mesh never shares storage with its twin, so the same geometry at a different buffer offset,
in a differently-strided bufferView, behind a different accessor index, or bound to a
duplicate material entry still compares equal. The losing mesh is garbage-collected on the
next line, so a check that passed on equal triangle and vertex counts alone would silently
swap one model for another with nothing left to notice it.

    python3 scripts/tank/diet/dedupe.py selftest

Builds five two-mesh glbs in a temp directory and runs the real repoint path over each: two
genuine twins (different offset and stride; duplicate material entry) must be accepted and
GC'd down to one mesh, and three meshes with EQUAL triangle and vertex counts but a vertex
moved 1 mm, a reversed winding, or a different base colour must each be refused. Plain
`python3`, no dependencies, about a second.

    python3 scripts/tank/diet/dedupe.py assets/tiger_1/tiger_1.glb \
        Coax_MG_Barrel_Visual=Object_0.002 \
        Coax_MG_Body_Visual=Object_8.002 \
        Coax_MG__Mag_Visual=Object_9.002

    python3 scripts/tank/diet/extract.py assets/tiger_1/tiger_1.glb 15 $W/mg.glb
    blender -b -P scripts/tank/diet/decimate.py -- $W/mg.glb $W/mg_dec.glb 1930 30
    python3 scripts/tank/diet/inject.py assets/tiger_1/tiger_1.glb Object_0.002 $W/mg_dec.glb

1 930, not 1 000: the barrel is a perforated MG34 cooling jacket, 13 shells and 207 boundary
edges, and the collapse decimator floors at 1 139 triangles with spikes off the muzzle. Run
`probe.py` on `$W/mg.glb` to see it — the ratios 0.12 through 0.001 all return the same 1 139.

Note the mesh reference switches from index (`15`) to name (`Object_0.002`) after the dedupe
step, because the GC moved every index above 29.

### 4 — back-face culling

    python3 scripts/tank/diet/extract_all.py assets/tiger_1/tiger_1.glb $W/vis.glb --with-link
    RES=2000 python3 scripts/tank/diet/drive_bf.py $W/vis.glb
    python3 scripts/tank/diet/singlesided.py assets/tiger_1/tiger_1.glb

Do not skip the probe, and do not run it at the default resolution. At 900 px the whole-tank
probe reports 11 red pixels only because the MG bore is sub-pixel; at 2 000 px over 32 camera
positions it reports 115 in 128 M, and cropping the worst frame shows they are two coincident
faces rather than a hole. `singlesided.py` takes material names to spare — the shipped run
needed none, but that is a measurement, not a property of the model.

Re-verified 2026-08-01 on Blender 5.1.2 against the committed `tiger_1.glb`, through the
fail-closed gate: **115 red pixels over 32 frames at 2 000 px, worst frame
`p_el-15_az135.png` at 38** — the same number and the same worst frame as the run that
decided the change. Frames land in `scripts/tank/diet/out/` (gitignored, never committed).

## Two design decisions that look like bugs

### The JSON chunk is padded to a fixed 96 KiB. Do not "fix" this.

`glblib.JSON_PAD` pads the glb's JSON chunk with spaces to exactly 98 304 bytes on every
write. It is not slop and it is not a rounding artefact.

The BIN chunk starts immediately after the JSON chunk, so its file offset is a function of
the JSON's length. Let the JSON shrink by one byte and all 63 MB after it shifts by one byte,
which makes every revision of a git-lfs blob a full rewrite rather than a delta. Pinning the
JSON length pins the BIN offset, so the 59.6 MB of KTX2 that never changes stays byte-aligned
across revisions and only the appended geometry differs. The original chunk is 84 880 bytes;
96 KiB leaves room for the accessors each edit adds. If a future edit overflows it, `save()`
raises rather than silently shifting — raise the constant, do not remove it.

### The buffer is never compacted, so ~1.4 MB of orphaned geometry is dead weight

`inject.py` appends new vertex data and repoints the accessors; it does not reclaim the bytes
the old accessors used, and `dedupe.py` drops mesh entries from the JSON without reclaiming
theirs. About 1.4 MB of a 63 MB file is unreachable.

This is deliberate, for the same reason as the padding: compacting means rewriting every
bufferView offset in the file, which moves the KTX2 payload and destroys the delta. A 2 %
file-size win is not worth turning every future edit into a full-blob rewrite. `validate.py`
reports orphan meshes and materials, so the dead weight stays visible rather than forgotten.

Nothing loads an orphaned bufferView — bevy reads accessors, and only through primitives — so
the cost is disk, not VRAM or frame time.
