import bpy, math, os, shutil, statistics, subprocess, sys
from mathutils import Vector

# This script AUTHORS; it does not export. The export is the one door's — see the tail.
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(bpy.data.filepath)))
SP = os.environ.get("OVERMATCH_TRACK_SCRATCH") or sys.exit("set OVERMATCH_TRACK_SCRATCH to the directory holding metal512/ — the session scratchpad this was authored in is gone")
SRC = SP + "/metal512"
TILE_M   = 0.35
MAT_NAME = "Mat_Track_Link"

link = bpy.data.objects["Link"]

# ---- unwrap, with the tiling baked into the UV coords (a Mapping node would only reach
# ---- base_color_texture through bevy_gltf 0.19 — see KHR_texture_transform, bevy #15310)
for o in bpy.data.objects: o.select_set(False)
bpy.context.view_layer.objects.active = link; link.select_set(True)
bpy.ops.object.mode_set(mode='EDIT'); bpy.ops.mesh.select_all(action='SELECT')
bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.003)
bpy.ops.object.mode_set(mode='OBJECT')

def uvarea(p, uvl):
    pts=[Vector(uvl[i].uv) for i in p.loop_indices]; a=0.0
    for i in range(1,len(pts)-1):
        u,v=pts[i]-pts[0],pts[i+1]-pts[0]; a+=abs(u.x*v.y-u.y*v.x)/2
    return a
uvl = link.data.uv_layers[0].data
base = statistics.median([math.sqrt(uvarea(p,uvl))/math.sqrt(p.area)
                          for p in link.data.polygons if p.area>1e-9 and uvarea(p,uvl)>1e-12])
f = (1.0/TILE_M)/base
for d in uvl: d.uv = (d.uv[0]*f, d.uv[1]*f)

def img(fname, name, non_color):
    i = bpy.data.images.load(os.path.join(SRC, fname))
    if non_color: i.colorspace_settings.name = 'Non-Color'
    i.alpha_mode = 'NONE'
    i.pack(); i.name = name
    return i

albedo = img("albedo.png",    "track_link_albedo",    False)
rough  = img("roughness.png", "track_link_roughness", True)
normal = img("normal.png",    "track_link_normal",    True)

for stale in (MAT_NAME,):
    if stale in bpy.data.materials: bpy.data.materials.remove(bpy.data.materials[stale])
m = bpy.data.materials.new(MAT_NAME); m.use_nodes = True
nt = m.node_tree; b = nt.nodes["Principled BSDF"]
ta = nt.nodes.new("ShaderNodeTexImage"); ta.image=albedo; ta.location=(-620, 320)
tr = nt.nodes.new("ShaderNodeTexImage"); tr.image=rough;  tr.location=(-620,  10)
tn = nt.nodes.new("ShaderNodeTexImage"); tn.image=normal; tn.location=(-620,-300)
nm = nt.nodes.new("ShaderNodeNormalMap"); nm.location=(-300,-300)
nt.links.new(nm.inputs["Color"],     tn.outputs["Color"])
nt.links.new(b.inputs["Base Color"], ta.outputs["Color"])
nt.links.new(b.inputs["Roughness"],  tr.outputs["Color"])
nt.links.new(b.inputs["Normal"],     nm.outputs["Normal"])
b.inputs["Metallic"].default_value = 1.0

link.data.materials.clear(); link.data.materials.append(m)
for p in link.data.polygons: p.material_index = 0
print(f"OK unwrap={len(link.data.uv_layers)}L tile={TILE_M}m mat={MAT_NAME} imgs=512px")

# Save, then hand the SAVED file to the one door. It opens the blend in its own Blender, runs the
# L1 source pass, exports the raw candidate, runs the consumer contract and the KTX2 derivation,
# cuts the LOD chains and publishes the trio — `scripts/tank/build.py`. Nothing about the export
# is decided here.
bpy.ops.wm.save_mainfile()
door = subprocess.run(
    [shutil.which("python3") or "python3", os.path.join(ROOT, "scripts", "tank", "build.py"),
     "build", bpy.data.filepath],
    cwd=ROOT,
)
if door.returncode:
    raise SystemExit(door.returncode)
print("DONE")
