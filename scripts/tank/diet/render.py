"""render.py — headless turntable renders of a geometry-only glb, for eyeballing a decimation.

    blender -b -P render.py -- <in.glb> <out_prefix> <mode> [tex_dir|-] [az az ...]

    mode "tex"   textured PBR; tex_dir holds albedo.png / roughness.png / normal.png, which
                 `dumpimg.py` + `basisu -unpack` produce from the glb's own KTX2
         "clay"  neutral grey, or the glb's own factors via MAT_COLOR / MAT_METAL / MAT_ROUGH
         "wire"  clay plus a black wireframe overlay, which is how topology loss reads

Env: RES (800), ELEV (22 deg), DIST_F (3.4 bounding radii), MAT_COLOR, MAT_METAL, MAT_ROUGH.

The light rig SCALES WITH THE SUBJECT — positions in units of the bounding radius, energy
with its square. A fixed rig blew the 0.15 m MG barrel to pure white while lighting the
0.73 m shoe correctly, and a blown-out render hides exactly the shading artefacts this
exists to show.
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

RES = int(os.environ.get("RES", 800))
ELEV = float(os.environ.get("ELEV", 22.0))
DIST_F = float(os.environ.get("DIST_F", 3.4))

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.engine = "BLENDER_EEVEE"
try:
    scene.eevee.taa_render_samples = 24
except AttributeError:
    pass
scene.render.resolution_x = RES
scene.render.resolution_y = RES
scene.render.film_transparent = False
scene.view_settings.view_transform = "Standard"

bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=False)
objs = [o for o in scene.objects if o.type == "MESH"]
ob = objs[0]

# --- material -------------------------------------------------------------
mat = bpy.data.materials.new("Preview")
mat.use_nodes = True
nt = mat.node_tree
bsdf = nt.nodes["Principled BSDF"]
if MODE == "tex" and TEXDIR:
    def tex(name, colorspace):
        n = nt.nodes.new("ShaderNodeTexImage")
        n.image = bpy.data.images.load(os.path.join(TEXDIR, name))
        n.image.colorspace_settings.name = colorspace
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
    # Defaults are a neutral clay; MAT_* override them with the glb's own factors so the
    # MG renders in its real near-black metal rather than a grey stand-in.
    c = float(os.environ.get("MAT_COLOR", 0.55))
    bsdf.inputs["Base Color"].default_value = (c, c, c * 1.02, 1.0)
    bsdf.inputs["Roughness"].default_value = float(os.environ.get("MAT_ROUGH", 0.45))
    bsdf.inputs["Metallic"].default_value = float(os.environ.get("MAT_METAL", 0.0))

ob.data.materials.clear()
ob.data.materials.append(mat)

if MODE == "wire":
    wire = ob.copy()
    wire.data = ob.data.copy()
    scene.collection.objects.link(wire)
    m = wire.modifiers.new("Wireframe", "WIREFRAME")
    m.thickness = 0.0016
    m.use_replace = True
    wmat = bpy.data.materials.new("Wire")
    wmat.use_nodes = True
    wb = wmat.node_tree.nodes["Principled BSDF"]
    wb.inputs["Base Color"].default_value = (0.02, 0.02, 0.02, 1)
    wb.inputs["Roughness"].default_value = 1.0
    wire.data.materials.clear()
    wire.data.materials.append(wmat)

# --- world + lights -------------------------------------------------------
world = bpy.data.worlds.new("W")
scene.world = world
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.09, 0.10, 0.12, 1)
world.node_tree.nodes["Background"].inputs[1].default_value = 1.0


def add_light(name, loc, energy, size=2.0):
    d = bpy.data.lights.new(name, "AREA")
    d.energy = energy
    d.size = size
    o = bpy.data.objects.new(name, d)
    o.location = loc
    scene.collection.objects.link(o)
    tc = o.constraints.new("TRACK_TO")
    tc.target = target
    tc.track_axis = "TRACK_NEGATIVE_Z"
    tc.up_axis = "UP_Y"
    return o


bbox = [ob.matrix_world @ mathutils.Vector(c) for c in ob.bound_box]
cx = sum(v.x for v in bbox) / 8
cy = sum(v.y for v in bbox) / 8
cz = sum(v.z for v in bbox) / 8
radius = max((v - mathutils.Vector((cx, cy, cz))).length for v in bbox)

target = bpy.data.objects.new("Target", None)
target.location = (cx, cy, cz)
scene.collection.objects.link(target)

# Rig scales with the subject: positions in units of its bounding radius and energy with
# the square of it, so a 0.15 m barrel and a 0.73 m shoe get the same illuminance.
R = radius
K = (R / 0.37) ** 2
add_light("Key", (cx + 3.2 * R, cy - 3.2 * R, cz + 3.8 * R), 320 * K, 4.0 * R)
add_light("Fill", (cx - 3.8 * R, cy - 2.2 * R, cz + 1.1 * R), 90 * K, 5.4 * R)
add_light("Rim", (cx, cy + 4.3 * R, cz + 2.4 * R), 160 * K, 4.0 * R)

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
    a = math.radians(az)
    e = math.radians(ELEV)
    cam.location = (
        cx + dist * math.cos(e) * math.cos(a),
        cy + dist * math.cos(e) * math.sin(a),
        cz + dist * math.sin(e),
    )
    scene.render.filepath = f"{PREFIX}_az{int(az):03d}.png"
    bpy.ops.render.render(write_still=True)
    print(f"[render] {scene.render.filepath}")
