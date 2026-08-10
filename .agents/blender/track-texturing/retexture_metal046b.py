"""Swap the Tiger track-link PBR maps from FreePBR "worn-metal4" to ambientCG Metal046B (CC0).

Run headless:
    blender --background assets/tiger_1/tiger_1.blend \
            --python .agents/blender/track-texturing/retexture_metal046b.py -- <src_512_dir>

This is an IMAGE swap inside the existing `Mat_Track_Link`, not a re-authoring:

* The UVs are untouched. Tiling is baked into the UV coordinates at 0.35 m per repeat and must
  stay there — `bevy_gltf` 0.19 honours `KHR_texture_transform` only on `base_color_texture`
  (bevy #15310), so a Mapping node would tile albedo and leave roughness/normal at 1x. The
  script asserts no Mapping node exists.
* The material NAME must not change: the game resolves it by name
  (`src/track/link_view.rs`, `LINK_MATERIAL = "Mat_Track_Link"`).
* Images must be pre-sized to 512x512 on disk. `bpy.types.Image.pack()` on a FILE-source image
  re-embeds the ORIGINAL file, silently discarding any in-memory `.scale()` — so downscaling
  must happen outside Blender (ImageMagick; albedo resized in linear light, data maps raw).
* Metal046B's Metalness map is NOT flat (mean 0.830, std 0.225 — a soft worn-metal mask), unlike
  worn-metal4's flat 1.0. So metallic is driven by a TEXTURE here; the glTF exporter combines the
  separate roughness + metalness images into one ORM (G=roughness, B=metallic).
* Metal046B ships no AO map, and its Displacement is unused (as Height was before).

Old FreePBR image datablocks are removed from the blend so no paid-license content survives in
either the .blend or the exported .glb.

Export goes through `scripts/tank/asset_door.py`, never `bpy.ops.export_scene.gltf` directly: the
raw exporter embeds PNG/JPEG, which bevy uploads with ONE mip level, and the door folds the KTX2
mip bake into the export so a mipless glb cannot reach the tracked path. This script saves and
hands the stored file over; every export setting is the door's frozen list.
"""
import bpy
import os
import shutil
import subprocess
import sys

MAT_NAME = "Mat_Track_Link"

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
SRC = argv[0] if argv else None
if not SRC or not os.path.isdir(SRC):
    raise SystemExit(f"need a source dir of pre-sized 512 maps, got {SRC!r}")

BLEND = bpy.data.filepath

# The blend lives at <root>/assets/<id>/, so the work tree is three levels up.
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(BLEND)))

mat = bpy.data.materials.get(MAT_NAME)
if mat is None:
    raise SystemExit(f"material {MAT_NAME!r} not found")
nt = mat.node_tree

bsdf = next((n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED'), None)
if bsdf is None:
    raise SystemExit("no Principled BSDF in the link material")

# --- binding constraint: tiling lives in the UVs, never in a Mapping node -------------------
mapping = [n for n in nt.nodes if n.type == 'MAPPING']
if mapping:
    raise SystemExit(f"Mapping node(s) present ({[n.name for n in mapping]}) — "
                     "tiling must stay baked into the UV coordinates (bevy #15310)")


def upstream_tex(socket):
    """Walk back from a BSDF input to the ShaderNodeTexImage feeding it (through NormalMap)."""
    if not socket.is_linked:
        return None
    node = socket.links[0].from_node
    if node.type == 'TEX_IMAGE':
        return node
    for inp in node.inputs:
        found = upstream_tex(inp)
        if found:
            return found
    return None


tex_albedo = upstream_tex(bsdf.inputs["Base Color"])
tex_rough = upstream_tex(bsdf.inputs["Roughness"])
tex_normal = upstream_tex(bsdf.inputs["Normal"])
tex_metal = upstream_tex(bsdf.inputs["Metallic"])  # expected None (was a flat 1.0)

for label, node in (("base color", tex_albedo), ("roughness", tex_rough), ("normal", tex_normal)):
    if node is None:
        raise SystemExit(f"no image texture feeding {label}")

old_images = {n.image for n in (tex_albedo, tex_rough, tex_normal, tex_metal) if n and n.image}
print("OLD images:", sorted(i.name for i in old_images))


def load_packed(fname, name, non_color):
    path = os.path.join(SRC, fname)
    w_h = None
    img = bpy.data.images.load(path, check_existing=False)
    img.colorspace_settings.name = 'Non-Color' if non_color else 'sRGB'
    img.alpha_mode = 'NONE'
    img.pack()          # re-embeds the file on disk — which is why it is pre-sized
    img.name = name
    w_h = tuple(img.size)
    if w_h != (512, 512):
        raise SystemExit(f"{fname} is {w_h}, expected (512, 512) — pre-size it outside Blender")
    print(f"  loaded {name:<24} {w_h[0]}x{w_h[1]} colorspace={img.colorspace_settings.name}")
    return img


print("NEW images:")
img_albedo = load_packed("albedo.png", "albedo", False)
img_rough = load_packed("roughness.png", "roughness", True)
img_normal = load_packed("normal.png", "normal", True)
img_metal = load_packed("metalness.png", "metalness", True)

tex_albedo.image = img_albedo
tex_rough.image = img_rough
tex_normal.image = img_normal

# Metal046B's metalness is a real mask -> drive Metallic from a texture instead of the flat 1.0
if tex_metal is None:
    tex_metal = nt.nodes.new("ShaderNodeTexImage")
    tex_metal.location = (tex_rough.location.x, tex_rough.location.y + 155)
    nt.links.new(bsdf.inputs["Metallic"], tex_metal.outputs["Color"])
tex_metal.image = img_metal
tex_metal.label = "Metalness"

# --- purge the retired FreePBR datablocks ---------------------------------------------------
for img in old_images:
    if img in (img_albedo, img_rough, img_normal, img_metal):
        continue
    print(f"  removing old image datablock {img.name!r} ({img.size[0]}x{img.size[1]})")
    bpy.data.images.remove(img)

# --- post-conditions -------------------------------------------------------------------------
assert mat.name == MAT_NAME, "material name must not change (game resolves it by name)"
assert not [n for n in nt.nodes if n.type == 'MAPPING'], "a Mapping node crept in"
link = bpy.data.objects["Link"]
assert len(link.data.uv_layers) >= 1, "link lost its UVs"
assert link.data.materials and link.data.materials[0].name == MAT_NAME, "link lost its material"
print(f"OK material={mat.name} uv_layers={len(link.data.uv_layers)} "
      f"metallic=texture rough=texture no_mapping_node=True")

bpy.ops.wm.save_mainfile()
print(f"SAVED {BLEND}")
# The one door, on the saved file: source pass ▸ raw candidate ▸ consumer contract ▸ KTX2
# derivation ▸ the tracked glb, which only a chain that passed every stage may replace.
door = subprocess.run(
    [shutil.which("python3") or "python3", os.path.join(ROOT, "scripts", "tank", "asset_door.py"),
     "export", BLEND],
    cwd=ROOT,
)
if door.returncode:
    raise SystemExit(door.returncode)
print("DONE")
