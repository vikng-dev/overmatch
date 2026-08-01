# `scripts/tank/diet/` — the tools that put the Tiger on a geometry diet

Twenty small scripts that decimated the track shoe, deduplicated the machine guns and turned
back-face culling on, plus the instruments that proved each one safe. They exist because the
assets they act on are **binary and otherwise unreproducible**: `assets/tiger_1/tiger_1.glb`
is a tracked git-lfs blob, and the `.blend` it came from is untracked and 140 MB.

The two COLLAPSE decimations were REVERTED on 2026-08-01 — `Object_0.002` is the authored MG
barrel again, and `Link` was restored before being re-cut by a different decimator. Read
[THE SECOND RULE](#the-second-rule-a-base-mesh-is-human-domain) before reaching for one, and
[Restoration](#restoration-putting-an-original-mesh-back) for how a base mesh gets put back.

The shoe now ships a three-tier PLANAR chain instead — see
[The planar LOD chain](#the-planar-lod-chain). That is a different decimator from the one the
revert was about: planar dissolve never moves a vertex, which is why its output survived the
eyeball that rejected collapse output at a comparable budget.

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

## THE SECOND RULE: a BASE mesh is human domain

Standing instruction from Yan, 2026-08-01, after reviewing the shipped decimations:

> "518 is not good, 964 is debatable (mangled and asymmetric). same for the MG.
> model simplification is still human domain."

The decimators in here may produce **distance-LOD candidates only**, and every candidate is
an unwired asset until an eyeball says otherwise. What a player sees up close — the base
mesh a node actually points at — is not something a quadric-collapse budget gets to decide.
This is a quality rule, not a perf one, and it outranks any triangle target in this file.

So: `decimate.py` output is legitimate for `tiger_1_link.lod1.glb` at 500 m, and is NOT
legitimate for `Link` or `Object_0.002`. The 5 552-triangle shoe and the 7 701-triangle MG
barrel are the shipped base, restored on 2026-08-01 — see [Restoration](#restoration-putting-an-original-mesh-back).

Everything in the diet that was NOT a simplification stayed: back-face culling, the MG
dedupe, the deleted terrain textures. Those are lossless and they still ship.

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
| `deviation.py` | Point-to-surface distance between an LOD and its source, BOTH directions, in mm. Area-weighted deterministic sampling. This is the number the pixel arithmetic turns into a switch distance. |
| `uvcheck.py` | Whether the mapping survived: TEXCOORD_0 present, uv bbox, uv-degenerate triangles, longest uv edge vs the source's, and how many LOD verts still sit on an authored position with its authored UV. |
| `thin.py` | Which primitives are open shells or single-layer sheets — the first-pass shortlist for the culling question. Advisory only; `backface.py` is what decides. |
| `backface.py` | Renders the subject with every visible BACK FACE emitting red. A red pixel is a pixel that back-face culling turns into a hole. |
| `drive_bf.py` | Runs `backface.py` over 32 camera positions and counts red pixels per glb. Fails closed: a nonzero Blender exit, a missing frame or a frame that is not `RES` x `RES` aborts with Blender's stderr and prints no total. |

### renders for review

| tool | what it does |
|---|---|
| `render.py` | Headless turntable of a geometry-only glb: textured, clay, or clay with a wireframe overlay. Light rig scales with the subject. |
| `belt.py` | The same rig over a ROW of shoes at track pitch. The shoe is never seen alone, and the belt caught damage four turntable azimuths did not — see [the fine-cylinder threshold](#the-fine-cylinder-threshold-what-the-deviation-number-cannot-see). |
| `drive_render.py` | Runs `render.py` over several variants in both modes. |
| `sheet.py` | Composes labelled contact sheets (PIL). `.jpg` output is ~5x smaller than `.png` for these and visually identical, which matters when the sheet is committed. |
| `redcount.py` | Standalone red-pixel counter for a glob of `backface.py` frames. |

### the three decimators

`decimate2.py` adds a planar-dissolve pass before the collapse, on the theory that flat plate
faces should not spend budget. It does not pay off at a tight budget — on the shoe it returns
1 009 vertices for 502 triangles against `decimate.py`'s 993 for 518 — and it does not lift
the MG barrel's floor either. It is kept because it built the 964-triangle shoe alternative,
and because the negative result is worth not re-deriving.

`decimate_planar.py` is the one whose output ships today, at all three shoe tiers. It drives
the DISSOLVE decimate rather than the collapse: coplanar faces merge into ngons and the export
re-triangulates them, so no vertex ever moves and every surviving position is authored. It
also carries the optional collapse pass the two tight tiers need. See
[The planar LOD chain](#the-planar-lod-chain).

`decimate.py` built the shoe and MG cuts that were reverted, and is kept because the recipes
below still reference it: import with `merge_vertices`, weld at 1e-5, triangulate,
binary-search a quadric-collapse ratio to a triangle budget, then rebuild hard edges from a
dihedral angle. The collapse decimator drops custom split normals, so re-shading is not
optional — and the angle is a real lever: 30 deg gives 993 verts, 12 deg gives 1 303 for the
same 518 triangles and slightly crisper panels. 30 was chosen because the audit names vertex
fetch as a co-bottleneck.

## Reproducing the shipped assets

Requires Blender 5.1 (`BLENDER=` env var, else `blender` on PATH), Python 3 with Pillow for
the render tools only, and `basisu` for textured renders. Run from the repo root. `$W` is a
scratch directory.

Verified 2026-08-01 on Blender 5.1.2: re-running steps 1 and 2 reproduces the 518, 194 and
964-triangle shoe meshes with **bit-identical vertex positions**. The decimation is
deterministic; the recipes below are the real source of what they built.

**Only step 4 still describes shipped state.** Step 1 and step 3 were reverted on 2026-08-01
(see [Restoration](#restoration-putting-an-original-mesh-back)), and step 2's LOD1 was
replaced on the same day by [The planar LOD chain](#the-planar-lod-chain), which is where the
shoe's three shipped tiers now come from. What survives here: step 2's `alt964` line, step 4
in full, and the `$W/shoe_src.glb` extraction in step 1 that every other recipe depends on.

### 1 — track shoe, 5 552 -> 518 triangles (REVERTED — the extract line is still the entry point)

    python3 scripts/tank/diet/extract.py assets/tiger_1/tiger_1.glb 1 $W/shoe_src.glb
    blender -b -P scripts/tank/diet/decimate.py -- $W/shoe_src.glb $W/shoe_lod0.glb 520 30
    python3 scripts/tank/diet/inject.py assets/tiger_1/tiger_1.glb 1 $W/shoe_lod0.glb

Mesh 1 is `Link`. It is view-only: `src/track/marker_model.rs` measures the track from
`Link_Box`, the pin-marker empties and the sprocket/idler/wheel meshes, and its
`REQUIRED_MESHES` does not include the shoe. Support is the contact envelope and grip is
per-element analytic, so nothing samples this mesh.

### 2 — LOD1 and the 964-triangle alternative (LOD1 SUPERSEDED — see the planar chain)

The `lod1.glb` line below built the 194-triangle collapse shoe that shipped until
2026-08-01. It is no longer the recipe: `tiger_1_link.lod1.glb` is now 386 triangles from
[The planar LOD chain](#the-planar-lod-chain). Kept for the record and because the
`alt964` line still stands.

    blender -b -P scripts/tank/diet/decimate.py -- $W/shoe_src.glb $W/lod1.glb 200 30
    python3 scripts/tank/diet/rename.py $W/lod1.glb assets/tiger_1/tiger_1_link.lod1.glb Link_LOD1

    blender -b -P scripts/tank/diet/decimate2.py -- $W/shoe_src.glb $W/a964.glb 1000 30 1.0
    python3 scripts/tank/diet/rename.py $W/a964.glb assets/tiger_1/tiger_1_link.alt964.glb Link

`tiger_1_link.alt964.glb` is the candidate that keeps the pin bosses. Swapping it in is one
`inject.py` call with mesh name `Link`; that trade is Yan's, not this directory's.

### 3 — machine guns: dedupe, THEN decimate (the DEDUPE ships; the decimate was REVERTED)

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

## Restoration: putting an original mesh back

Done 2026-08-01 for `Link` (518 -> 5 552) and `Object_0.002` (1 923 -> 7 701) under THE
SECOND RULE above. The recipe is step 1 and step 3 run in the OTHER DIRECTION, and it is a
recipe rather than a file copy for one reason: the current glb also carries the diet's
lossless wins — `doubleSided` gone from all 11 materials, the MG dedupe with its garbage
collection, the terrain textures deleted — and a file copy would throw all of that away
along with the decimation. So the geometry is moved, not the file.

`$ORIG` is the PRE-diet glb, i.e. the bytes of `b86c1af:assets/tiger_1/tiger_1.glb`. It is
not in the worktree; materialise it from git-lfs and check it before trusting it, because a
plain `git show` of an lfs path yields the 133-byte POINTER, not the asset:

    git show b86c1af:assets/tiger_1/tiger_1.glb          # -> "version https://git-lfs..."

Read the `oid sha256:` out of that pointer and take the object from the lfs cache
(`.git/lfs/objects/<xx>/<yy>/<oid>`), or `git lfs fetch` it. Verify before use — 63 170 892
bytes, and `shasum -a 256` equal to the pointer's oid.

    python3 scripts/tank/diet/extract.py $ORIG 1  $W/shoe_orig.glb     # Link,         5 552 tris
    python3 scripts/tank/diet/extract.py $ORIG 15 $W/barrel_orig.glb   # Object_0.002, 7 701 tris
    python3 scripts/tank/diet/inject.py assets/tiger_1/tiger_1.glb \
        Link $W/shoe_orig.glb  Object_0.002 $W/barrel_orig.glb

Mesh 15 in the PRE-diet file is already `Object_0.002` — the dedupe GC only moved indices
ABOVE 29, so the two files agree here. Inject by NAME anyway, as above; the name is what
survives an index shift, and `inject.py` asserts the name matches exactly one mesh.

The round trip is exactly lossless on this asset, which is worth having checked rather than
assumed: both meshes store POSITION / NORMAL / TEXCOORD_0 as float32 and indices as
`5123` (u16), carry no tangents and no second UV, and have one primitive each — so
`extract.py`'s decode and `inject.py`'s re-encode reproduce every element bit-for-bit.
Confirm it, do not trust it:

Fenced rather than indented because the body is Python and the leading spaces would matter:

```sh
ORIG=/path/to/pre-diet/tiger_1.glb python3 - <<'EOF'
import os, sys; sys.path.insert(0, "scripts/tank/diet")
import glblib
cur = glblib.Glb.load("assets/tiger_1/tiger_1.glb")
org = glblib.Glb.load(os.environ["ORIG"])
for mi in (1, 15):
    c, o = (g.gltf["meshes"][mi]["primitives"][0] for g in (cur, org))
    for k in ("POSITION", "NORMAL", "TEXCOORD_0"):
        assert cur.read_accessor(c["attributes"][k]) == org.read_accessor(o["attributes"][k]), k
    assert cur.read_accessor(c["indices"]) == org.read_accessor(o["indices"])
    assert c.get("material") == o.get("material")
print("both meshes bit-exact against the pre-diet file")
EOF
```

### What the restoration must NOT disturb, and how that was shown

Injecting geometry touches accessors, so the check that matters is a whole-file diff against
the PRE-restoration glb rather than a look at the two meshes. Decode every mesh in both and
compare digests; then compare the JSON sections and the image payloads directly. The
2026-08-01 run reported **exactly two meshes changed — `Link` and `Object_0.002`** — with
`materials`, `nodes`, `scenes`, `images`, `textures` and `samplers` all structurally
identical and the KTX2 image bytes hashing equal. That is the evidence the dedupe, the
culling and the mip chains survived; the tri counts alone would not have shown it.

The four standing checks, all of which passed:

    python3 scripts/tank/diet/validate.py assets/tiger_1/tiger_1.glb   # refs resolve, views in bounds
    python3 scripts/tank/glb_ktx2.py verify assets/tiger_1/tiger_1.glb # 9 images, mip chains 10..13
    python3 scripts/tank/diet/report.py assets/tiger_1/tiger_1.glb     # per-tank counts, doubleSided 0/11
    python3 scripts/tank/diet/dedupe.py selftest                       # the equality check still refuses twins

`report.py` after restoration: shoe 5 552, visible body 69 834, belt 194 x 5 552 = 1 077 088,
**PER TANK 1 146 922 tris / 2 122 976 verts** — the pre-diet number, because the diet's
remaining wins do not change triangle count. `doubleSided materials: 0/11` is the line that
proves culling stayed on.

### Renders

Turntables of both restored meshes, beside the rejected decimations, at four azimuths in
textured/clay and wireframe:

    RES=900 python3 scripts/tank/diet/drive_render.py $OUT $W/link_tex 40,90,140,230 \
        shoe_restored_5552=$W/shoe_restored.glb shoe_rejected_518=$W/shoe_dec518.glb
    RES=900 MAT_COLOR=0.022 MAT_METAL=0.827 MAT_ROUGH=0.5 \
    python3 scripts/tank/diet/drive_render.py $OUT - 40,90,140,230 \
        barrel_restored_7701=$W/barrel_restored.glb barrel_rejected_1923=$W/barrel_dec1923.glb

The barrel takes `-` for its texture directory and the `MAT_*` factors instead: its material
(`Material.006`) carries NO textures, only a near-black base colour of 0.0220 at metallic
0.827 — passing a texture directory there would render a lie. The shoe's `Mat_Track_Link`
does have maps; build its directory with `dumpimg.py` on images 1 / 2 / 0 (albedo /
roughness / normal), `basisu -unpack` each, and copy the `*_unpacked_rgb_RGBA32_level_0_*`
PNGs — level 0 and RGBA32, not one of the transcoded formats, or the render judges the
codec instead of the mesh.

One artefact to expect and not chase: in `wire` mode the Wireframe modifier throws long
spikes off both barrels, restored and decimated alike. That is the modifier meeting the MG
jacket's 13 open shells and 207 boundary edges, not damage in the mesh — the clay and
textured rows are the ones to read for the barrel.

## The planar LOD chain

Built 2026-08-01 on Blender 5.1.2. Supersedes step 1 and step 2 as the shoe's source: all
three tiers now come from `decimate_planar.py`, and `tiger_1_link.lod1.glb` is no longer a
`decimate.py` artefact.

| tier | file | recipe | tris | verts | worst dev | p90 dev |
|---|---|---|---|---|---|---|
| authored | `tiger_1.glb` mesh 1 (was) | — | 5 552 | 10 530 | — | — |
| LOD0 | `tiger_1.glb` mesh 1 `Link` | planar 10° | 3 058 | 3 477 | 0.99 mm | 0.15 mm |
| LOD1 | `tiger_1_link.lod1.glb` | planar 60° all-boundaries + collapse 400 | 386 | 806 | 17.93 mm | 8.72 mm |
| LOD2 | `tiger_1_link.lod2.glb` | planar 60° all-boundaries + collapse 200 | 192 | 501 | 51.38 mm | 25.88 mm |

Deviation is `deviation.py`'s symmetric point-to-surface distance against the authored mesh,
in mm on a 725 mm shoe. Per tank the LOD0 swap is **1 146 922 → 663 086 tris (−42%)** and
**2 122 976 → 754 694 verts (−64%)**; the belt is 194 shoes × 2, so the shoe IS the tank.

    W=$(mktemp -d)
    python3 scripts/tank/diet/extract.py assets/tiger_1/tiger_1.glb 1 $W/shoe_src.glb

    #                                                                    angle  collapse
    ALL_BOUNDARIES=0 blender -b -P scripts/tank/diet/decimate_planar.py -- \
        $W/shoe_src.glb $W/lod0.glb 10
    ALL_BOUNDARIES=1 blender -b -P scripts/tank/diet/decimate_planar.py -- \
        $W/shoe_src.glb $W/lod1.glb 60 400
    ALL_BOUNDARIES=1 blender -b -P scripts/tank/diet/decimate_planar.py -- \
        $W/shoe_src.glb $W/lod2.glb 60 200

    python3 scripts/tank/diet/inject.py assets/tiger_1/tiger_1.glb Link $W/lod0.glb
    python3 scripts/tank/diet/rename.py $W/lod1.glb assets/tiger_1/tiger_1_link.lod1.glb Link_LOD1
    python3 scripts/tank/diet/rename.py $W/lod2.glb assets/tiger_1/tiger_1_link.lod2.glb Link_LOD2

`DELIMIT` defaults to `UV,SHARP` and is left at that everywhere. Then the four standing
checks from [Restoration](#what-the-restoration-must-not-disturb-and-how-that-was-shown), all
of which passed: exactly one mesh changed (`Link`), materials / nodes / scenes / textures /
samplers / images structurally identical, and all 9 KTX2 payloads hashing equal.

### THE WELD IS THE WHOLE TRICK, and it must be TIGHT

`bpy.ops.import_scene.gltf(merge_vertices=True)` **does not merge this mesh.** The shoe
arrives fully split — 10 530 verts for 5 552 triangles — and stays that way. Split verts mean
two faces sharing an edge do not share vertices, so the dissolve has no shared edge to work
across and barely moves: 10° returns 4 736 triangles, a 15 % cut instead of a 45 % one. An
explicit `remove_doubles` at 1e-5 takes it to **2 748 verts, one closed manifold shell, zero
boundary and zero non-manifold edges**, and the same 10° then returns 3 058.

Coarser is not better, and this is the counter-intuitive half. Widening the weld destroys the
coplanarity the dissolve depends on, so the planar pass gets WORSE:

| weld | after weld | after 60° planar |
|---|---|---|
| 1e-5 | 5 544 tris | **772** |
| 1e-4 | 5 444 | 774 |
| 1e-3 | 5 048 | 788 |
| 3e-3 | 2 208 | 1 670 |
| 6e-3 | 1 350 | 1 255 |

Weld tight, dissolve wide. Do not reach for the weld as a decimation lever.

### delimit: `{UV,SHARP}`, and why SHARP is free

`UV` costs about 1 % of the reduction (10° gives 3 024 triangles without it, 3 058 with) and
buys the guarantee that no face is dissolved across a UV seam — a face that spans two islands
drags the albedo across the join. Always worth it.

`SHARP` is a **no-op on this asset and is set anyway**: the welded shoe carries 0 sharp-flagged
edges and 0 seams, because its shading comes from custom split normals rather than edge flags.
`{UV,SHARP}` and `{UV}` produce byte-identical output at every angle tested. It costs nothing
and it is correct on the next mesh, which may not be flag-free. `NORMAL` also measured as a
no-op here; it is not set, because unlike SHARP it is not obviously harmless elsewhere.

Blender's own defaults are `delimit=set()` and `angle_limit=5°` — worth knowing when comparing
a headless run against something clicked in the GUI.

### The fine-cylinder threshold: what the deviation number cannot see

**The 10° preset flattens the pin sockets.** The socket is a finely tessellated cylinder whose
facets meet at 4–5°, so any limit at or above 5° dissolves the whole thing into one facet. It
reads as a blown-out specular smear where the authored mesh has a smooth recess.
`.agents/scratch/shoe-lod-chain-renders/socket_angle_threshold.jpg` is the ladder.

| angle | tris | worst dev | pin socket |
|---|---|---|---|
| 2° | 5 036 | 0.10 mm | intact |
| 3° | 4 932 | 0.19 mm | intact |
| 4° | 4 806 | 0.29 mm | intact |
| 5° | 4 452 | 0.55 mm | **flattened** |
| 6.5° | 4 252 | 0.58 mm | **flattened** |
| 10° | 3 058 | 0.99 mm | **flattened** (shipped) |

Two things follow, and both are general.

**Deviation cannot catch this, and no threshold on it would have.** The socket is ~16 mm
across, so flattening it moves the surface LESS than reducing the guide horn does — at 10° the
worst 1 mm sits on the horn and the socket is under 0.5 mm. Deviation weights a feature by how
far it moves; the eye weights it by whether a smooth cylinder just became a polygon. The
renders are a deliverable because of this, not a courtesy, and the belt sheet is what caught
it after four turntable azimuths did not.

**The angle limit is not a free parameter — it is a property of how the source was
tessellated.** A dihedral histogram of the welded shoe says so directly:

| band | edges | what it is |
|---|---|---|
| 0–0.5° | 3 694 (44 %) | genuinely flat plate faces — the free win |
| 0.5–5° | 865 (10 %) | **fine cylinders: the pin sockets** |
| 5–12° | 1 860 (22 %) | fillets, horn caps, coarser curvature |
| 45–91° | 1 851 (22 %) | hard plate edges, must survive any limit |

Run that histogram before picking an angle on a new mesh. The safe limit is just under the
band holding the smallest feature worth keeping; everything above it is a decision to spend
that feature. On this shoe the saving at 10° **is** the fine curved detail — protecting it
costs most of the win (4° keeps the socket at 4 806 tris, −13 %, against 3 058 at −45 %).

Scale for the judgement: the crop shows the socket at ~10 px, which the main camera reaches at
about 2.9 m and the gunner optic at about 19 m. Past ~10 m in the main view it is under 3 px
and not resolvable. That makes 10° a real call rather than an obvious defect — and the call
is Yan's, per the rule below.

### LOD0 CHANGES NEED AN IN-GAME EYEBALL BEFORE MERGE

LOD0 is the mesh a player sees up close, so
[THE SECOND RULE](#the-second-rule-a-base-mesh-is-human-domain) applies to it in full: a
triangle budget does not get to decide it, and neither does a deviation number. The 10° preset
is here because **Yan validated it in the GUI**, not because it measured well.

Any change to the LOD0 angle — including "improving" it to 4° to save the sockets — is an
asset change that ships to players and needs Yan's eyeball **in the game**, not in these
renders. LOD1 and LOD2 are distance tiers and do not: machine decimation is allowed there,
which is what makes their collapse pass legitimate.

### Why the tight tiers need a collapse pass

Planar alone **floors at 772 triangles** on this mesh (60° with all-boundaries; 89° gives 768,
so it is saturated, not merely slow). 300–500 is out of its reach, so LOD1 and LOD2 run a
quadric collapse on top of the planar result. That pass moves vertices and drops the authored
split normals, hence the re-shade at 30° — and it is flagged wherever it appears.

The planar base is not incidental to it. Feeding the collapse a mesh with the redundant
coplanar verts already gone measurably beats every other base at the same budget:

| collapse base | tris | worst dev |
|---|---|---|
| planar 10° | 382 | 27.48 mm |
| planar 20° all-boundaries | 396 | 25.29 mm |
| planar 40° all-boundaries | 392 | 21.28 mm |
| **planar 60° all-boundaries** | **386** | **17.93 mm** |

The collapse itself floors at 188 triangles here (a closed shell with pin-hole handles), which
is why the previous `lod1.glb` sat at 194 and why LOD2 targets 200.

`all_boundaries` is off for LOD0 and on for the collapse tiers. It dissolves verts along ngon
perimeters, which is most of the count at wide angles (60°: 772 triangles on, 1 758 off) but
can move a silhouette vert — a fair trade at 500 m, not one to make on the base mesh.

If the collapse is unwanted in the chain at all, a planar-only LOD1 is one command away:

    ALL_BOUNDARIES=1 blender -b -P scripts/tank/diet/decimate_planar.py -- \
        $W/shoe_src.glb $W/lod1_planar.glb 60      # 772 tris, worst 10.83 mm

### Switch distances: the pixel arithmetic

One pixel subtends `fov / height` radians, so a deviation `d` drops under a pixel beyond
`D = d / (fov / height)`. At 1440 px the main camera (0.785 rad, `src/spec.rs`) is
5.451e-4 rad/px and the gunner optic (0.12 rad, `tiger_1.tank.ron:326`) is 8.333e-5.

| tier | worst dev | < 1 px beyond (main) | < 1 px beyond (optic) |
|---|---|---|---|
| LOD0 | 0.99 mm | 1.8 m | 11.9 m |
| LOD1 | 17.93 mm | 32.9 m | 215.2 m |
| LOD2 | 51.38 mm | 94.3 m | 616.6 m |

**Suggested thresholds: D0→D1 at 250 m, D1→D2 at 650 m** — the optic numbers rounded up. The
optic binds because LOD selection is by distance and cannot see which camera is looking; a
threshold that satisfies the main camera at 33 m would show LOD1's faceting to a gunner at
6.5× magnification. LOD0 clears one pixel by 11.9 m even in the optic, which is the evidence
that it is safe as the base mesh at any range a player can get to.

If LOD selection is ever made fov-aware, the main camera can switch at 35 m / 100 m instead
and the belt gets much cheaper on every non-gunner view. And if the sight gains the discrete
4×/8× steps `src/spec.rs` anticipates, these distances scale as `1/fov` and must be recomputed
— an 8× step at 0.06 rad doubles both.

### Determinism

Same input, same output, byte for byte — verified by running each tier twice and hashing:

| tier | sha256 of the geometry-only glb |
|---|---|
| LOD0 | `51efb873f7440ea5bcd4b5b3ce3206ca887e85a2853bc4ad9d3a98681ebef45c` |
| LOD1 | `876a74a5b264ef5697fffa4861290cd8002ef3b30ff275819ed8c3b1edbe670d` |
| LOD2 | `c42a515f4ff57aedd93a52aebe0c757fa78efbb8e69d132c3848a9a4afb86ca4` |

Whole-file equality, not just vertex positions. `deviation.py` is deterministic for the same
reason it is trustworthy — area-weighted Halton sampling, no RNG.

### UV integrity

`uvcheck.py <authored> <lod>` on all three. LOD0 comes back clean on every check: uv bbox
identical, 0 degenerate triangles, longest uv edge 0.421 against the source's own 0.575, and
**100 % of its vertices still on an authored position with the authored UV** — which is the
planar dissolve's defining property stated as a measurement.

LOD1 and LOD2 anchor 24 % and 6 % (the collapse moved the rest), and where they anchor the UV
still matches exactly. The one check that does not come back clean is LOD2's longest uv edge,
0.674 against 0.575 — a 17 % stretch on its single worst triangle. Recorded rather than fixed:
it is a beyond-500 m tier where the shoe is under a pixel.

### Renders

    W=$(mktemp -d); OUT=.agents/scratch/shoe-lod-chain-renders
    RES=900 python3 scripts/tank/diet/drive_render.py $W/frames $TEX 40,90,140,230 \
        orig_5552=$W/shoe_src.glb lod0_3058=$W/lod0.glb \
        lod1_386=assets/tiger_1/tiger_1_link.lod1.glb \
        lod2_192=assets/tiger_1/tiger_1_link.lod2.glb
    RES=900 ELEV=18 DIST_F=2.8 blender -b -P scripts/tank/diet/belt.py -- \
        <glb> $W/frames/belt_<label>_tex tex $TEX 20 55 90

`$TEX` is the shoe texture directory built with `dumpimg.py` + `basisu -unpack` as described
under [Renders](#renders) above. `sheet.py` composes the four sheets. Read the belt sheets
first — they are the ones that decide, and they are the ones that found the socket.

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
