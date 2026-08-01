"""Render the tank with every VISIBLE BACK FACE flashed red.

A red pixel is exactly a pixel that back-face culling would turn into a hole. This is the
decisive test for flipping `doubleSided` off: no red at any camera angle means culling is
free; red localises the primitive that must stay double-sided.

Usage: blender -b -P backface.py -- <in.glb> <out_prefix> [az,az,...] [elev,elev,...]
"""

import math
import os
import sys

import bpy
import mathutils

argv = sys.argv[sys.argv.index("--") + 1 :]
IN, PREFIX = argv[0], argv[1]
DEFAULT_AZ = [0, 45, 90, 135, 180, 225, 270, 315]
AZ = [float(a) for a in argv[2].split(",")] if len(argv) > 2 else DEFAULT_AZ
EL = [float(a) for a in argv[3].split(",")] if len(argv) > 3 else [20]
RES = int(os.environ.get("RES", 900))

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.engine = "BLENDER_EEVEE"
scene.render.resolution_x = RES
scene.render.resolution_y = RES
scene.view_settings.view_transform = "Standard"
try:
    scene.eevee.taa_render_samples = 8
except AttributeError:
    pass

bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=False)
objs = [o for o in scene.objects if o.type == "MESH"]

mat = bpy.data.materials.new("BackfaceProbe")
mat.use_nodes = True
nt = mat.node_tree
nt.nodes.clear()
geo = nt.nodes.new("ShaderNodeNewGeometry")
front = nt.nodes.new("ShaderNodeBsdfDiffuse")
front.inputs["Color"].default_value = (0.30, 0.31, 0.33, 1)
back = nt.nodes.new("ShaderNodeEmission")
back.inputs["Color"].default_value = (1.0, 0.0, 0.0, 1)
back.inputs["Strength"].default_value = 1.0
mix = nt.nodes.new("ShaderNodeMixShader")
outp = nt.nodes.new("ShaderNodeOutputMaterial")
nt.links.new(geo.outputs["Backfacing"], mix.inputs["Fac"])
nt.links.new(front.outputs["BSDF"], mix.inputs[1])
nt.links.new(back.outputs["Emission"], mix.inputs[2])
nt.links.new(mix.outputs["Shader"], outp.inputs["Surface"])

lo = mathutils.Vector((1e9, 1e9, 1e9))
hi = mathutils.Vector((-1e9, -1e9, -1e9))
for o in objs:
    o.data.materials.clear()
    o.data.materials.append(mat)
    for c in o.bound_box:
        w = o.matrix_world @ mathutils.Vector(c)
        lo = mathutils.Vector((min(lo[i], w[i]) for i in range(3)))
        hi = mathutils.Vector((max(hi[i], w[i]) for i in range(3)))
centre = (lo + hi) / 2
radius = (hi - lo).length / 2

world = bpy.data.worlds.new("W")
scene.world = world
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.5, 0.52, 0.55, 1)
world.node_tree.nodes["Background"].inputs[1].default_value = 1.0

target = bpy.data.objects.new("T", None)
target.location = centre
scene.collection.objects.link(target)
cam_d = bpy.data.cameras.new("C")
cam_d.lens = 45
cam = bpy.data.objects.new("C", cam_d)
scene.collection.objects.link(cam)
scene.camera = cam
ct = cam.constraints.new("TRACK_TO")
ct.target = target
ct.track_axis = "TRACK_NEGATIVE_Z"
ct.up_axis = "UP_Y"

dist = radius * 2.9
for el in EL:
    for az in AZ:
        a, e = math.radians(az), math.radians(el)
        cam.location = (
            centre.x + dist * math.cos(e) * math.cos(a),
            centre.y + dist * math.cos(e) * math.sin(a),
            centre.z + dist * math.sin(e),
        )
        scene.render.filepath = f"{PREFIX}_el{int(el):+03d}_az{int(az):03d}.png"
        bpy.ops.render.render(write_still=True)
        print(f"[bf] {scene.render.filepath}")
