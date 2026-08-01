"""belt.py — render a ROW of linked shoes, which is how a player actually sees the track.

    blender -b -P belt.py -- <in.glb> <out_prefix> <mode> [tex_dir|-] [az az ...]

A single shoe on a turntable is the wrong unit of review for this asset. The shoe is never
seen alone: it is seen 194 times in a row, at a glancing angle, with its neighbours hiding
most of each one. A simplification that reads as damage in isolation can vanish in the belt,
and a seam that is invisible on one shoe can strobe down the whole run. So both renders exist
and the belt is the one that decides.

Env: COUNT (10), PITCH (0.13043 m, the marker-measured Tiger pitch), plus everything
`render.py` reads (RES, ELEV, DIST_F). Shoes are laid along Blender +Y: the glTF asset is
Y-up with the track advancing along glTF +Z, and the importer maps glTF Z to Blender Y.
"""

import math
import os
import sys

import bpy
import mathutils

argv = sys.argv[sys.argv.index("--") + 1 :]
IN, PREFIX, MODE = argv[0], argv[1], argv[2]
TEXDIR = argv[3] if len(argv) > 3 and argv[3] != "-" else None
ANGLES = [float(a) for a in argv[4:]] or [0, 45, 90, 135]

RES = int(os.environ.get("RES", 900))
ELEV = float(os.environ.get("ELEV", 22.0))
DIST_F = float(os.environ.get("DIST_F", 2.2))
COUNT = int(os.environ.get("COUNT", 10))
PITCH = float(os.environ.get("PITCH", 0.13043))

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.engine = "BLENDER_EEVEE"
try:
    scene.eevee.taa_render_samples = 24
except AttributeError:
    pass
scene.render.resolution_x = RES
scene.render.resolution_y = RES
scene.view_settings.view_transform = "Standard"

bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=False)
ob = [o for o in scene.objects if o.type == "MESH"][0]

mat = bpy.data.materials.new("Preview")
mat.use_nodes = True
nt = mat.node_tree
bsdf = nt.nodes["Principled BSDF"]
if MODE == "tex" and TEXDIR:

    def tex(name, cs):
        n = nt.nodes.new("ShaderNodeTexImage")
        n.image = bpy.data.images.load(os.path.join(TEXDIR, name))
        n.image.colorspace_settings.name = cs
        return n

    alb = tex("albedo.png", "sRGB")
    nt.links.new(alb.outputs["Color"], bsdf.inputs["Base Color"])
    rgh = tex("roughness.png", "Non-Color")
    sep = nt.nodes.new("ShaderNodeSeparateColor")
    nt.links.new(rgh.outputs["Color"], sep.inputs["Color"])
    nt.links.new(sep.outputs["Green"], bsdf.inputs["Roughness"])
    nrm = tex("normal.png", "Non-Color")
    nmap = nt.nodes.new("ShaderNodeNormalMap")
    nt.links.new(nrm.outputs["Color"], nmap.inputs["Color"])
    nt.links.new(nmap.outputs["Normal"], bsdf.inputs["Normal"])
    bsdf.inputs["Metallic"].default_value = 1.0
else:
    c = float(os.environ.get("MAT_COLOR", 0.55))
    bsdf.inputs["Base Color"].default_value = (c, c, c * 1.02, 1.0)
    bsdf.inputs["Roughness"].default_value = float(os.environ.get("MAT_ROUGH", 0.45))
    bsdf.inputs["Metallic"].default_value = float(os.environ.get("MAT_METAL", 0.0))

ob.data.materials.clear()
ob.data.materials.append(mat)

wmat = bpy.data.materials.new("Wire")
wmat.use_nodes = True
wb = wmat.node_tree.nodes["Principled BSDF"]
wb.inputs["Base Color"].default_value = (0.02, 0.02, 0.02, 1)
wb.inputs["Roughness"].default_value = 1.0

# Lay the run out along +Y, centred on the origin so the camera framing below is symmetric.
span = (COUNT - 1) * PITCH
for i in range(COUNT):
    dup = ob if i == 0 else ob.copy()
    if i:
        dup.data = ob.data
        scene.collection.objects.link(dup)
    dup.location.y = i * PITCH - span / 2
    if MODE == "wire":
        w = dup.copy()
        w.data = dup.data.copy()
        scene.collection.objects.link(w)
        m = w.modifiers.new("Wireframe", "WIREFRAME")
        m.thickness = 0.0016
        m.use_replace = True
        w.data.materials.clear()
        w.data.materials.append(wmat)

# Frame the whole run, not one shoe.
pts = []
for o in [o for o in scene.objects if o.type == "MESH"]:
    pts += [o.matrix_world @ mathutils.Vector(c) for c in o.bound_box]
ctr = mathutils.Vector(
    (sum(p.x for p in pts) / len(pts), sum(p.y for p in pts) / len(pts), sum(p.z for p in pts) / len(pts))
)
radius = max((p - ctr).length for p in pts)

target = bpy.data.objects.new("Target", None)
target.location = ctr
scene.collection.objects.link(target)

world = bpy.data.worlds.new("W")
scene.world = world
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.09, 0.10, 0.12, 1)


def add_light(name, loc, energy, size):
    d = bpy.data.lights.new(name, "AREA")
    d.energy, d.size = energy, size
    o = bpy.data.objects.new(name, d)
    o.location = loc
    scene.collection.objects.link(o)
    tc = o.constraints.new("TRACK_TO")
    tc.target = target
    tc.track_axis = "TRACK_NEGATIVE_Z"
    tc.up_axis = "UP_Y"


R = radius
K = (R / 0.37) ** 2
add_light("Key", (ctr.x + 3.2 * R, ctr.y - 3.2 * R, ctr.z + 3.8 * R), 320 * K, 4.0 * R)
add_light("Fill", (ctr.x - 3.8 * R, ctr.y - 2.2 * R, ctr.z + 1.1 * R), 90 * K, 5.4 * R)
add_light("Rim", (ctr.x, ctr.y + 4.3 * R, ctr.z + 2.4 * R), 160 * K, 4.0 * R)

cam_d = bpy.data.cameras.new("Cam")
cam_d.lens = 50
cam = bpy.data.objects.new("Cam", cam_d)
scene.collection.objects.link(cam)
scene.camera = cam
ct = cam.constraints.new("TRACK_TO")
ct.target = target
ct.track_axis = "TRACK_NEGATIVE_Z"
ct.up_axis = "UP_Y"

dist = radius * DIST_F
for az in ANGLES:
    a, e = math.radians(az), math.radians(ELEV)
    cam.location = (
        ctr.x + dist * math.cos(e) * math.cos(a),
        ctr.y + dist * math.cos(e) * math.sin(a),
        ctr.z + dist * math.sin(e),
    )
    scene.render.filepath = f"{PREFIX}_az{int(az):03d}.png"
    bpy.ops.render.render(write_still=True)
    print(f"[belt] {scene.render.filepath}")
