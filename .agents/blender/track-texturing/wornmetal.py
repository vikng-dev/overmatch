import bpy, math, os, statistics
from mathutils import Vector
SP="/private/tmp/claude-502/-Users-Yan-Desktop-github-vikng-dev-personal-overmatch/aa6ae501-da41-487d-a38f-23a2004cf55d/scratchpad"
OUT=SP+"/tex"; PACK=os.path.expanduser("~/Downloads/worn-metal4-bl")

def mat():
    m=bpy.data.materials.new("WornMetal4"); m.use_nodes=True
    nt=m.node_tree; b=nt.nodes["Principled BSDF"]
    alb=nt.nodes.new("ShaderNodeTexImage"); alb.image=bpy.data.images.load(PACK+"/worn_metal4_albedo.png")
    rgh=nt.nodes.new("ShaderNodeTexImage"); rgh.image=bpy.data.images.load(PACK+"/worn_metal4_Roughness.png")
    rgh.image.colorspace_settings.name='Non-Color'
    nrm=nt.nodes.new("ShaderNodeTexImage"); nrm.image=bpy.data.images.load(PACK+"/worn_metal4_Normal-ogl.png")
    nrm.image.colorspace_settings.name='Non-Color'
    nmap=nt.nodes.new("ShaderNodeNormalMap"); nt.links.new(nmap.inputs["Color"], nrm.outputs["Color"])
    nt.links.new(b.inputs["Base Color"], alb.outputs["Color"])
    nt.links.new(b.inputs["Roughness"], rgh.outputs["Color"])
    nt.links.new(b.inputs["Normal"], nmap.outputs["Normal"])
    b.inputs["Metallic"].default_value=1.0     # map measured flat 1.0 — no texture needed
    return m

link=bpy.data.objects["Link"]
for o in bpy.data.objects: o.select_set(False)
bpy.context.view_layer.objects.active=link; link.select_set(True)
bpy.ops.object.mode_set(mode='EDIT'); bpy.ops.mesh.select_all(action='SELECT')
bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.003)
bpy.ops.object.mode_set(mode='OBJECT')

def uvarea(p,uvl):
    pts=[Vector(uvl[i].uv) for i in p.loop_indices]; a=0.0
    for i in range(1,len(pts)-1):
        u,v=pts[i]-pts[0],pts[i+1]-pts[0]; a+=abs(u.x*v.y-u.y*v.x)/2
    return a
uvl=link.data.uv_layers[0].data
base=statistics.median([math.sqrt(uvarea(p,uvl))/math.sqrt(p.area)
                        for p in link.data.polygons if p.area>1e-9 and uvarea(p,uvl)>1e-12])
orig=[tuple(d.uv) for d in uvl]
print(f"unwrap gives {base:.3f} UV/m (1 tile = {1/base:.3f} m). Link is 0.725 m long.")

m=mat(); link.data.materials.clear(); link.data.materials.append(m)
for p in link.data.polygons: p.material_index=0

sc=bpy.context.scene; sc.render.engine='BLENDER_EEVEE'
sc.render.resolution_x,sc.render.resolution_y=1000,620
w=bpy.data.worlds.new("W"); sc.world=w; w.use_nodes=True
w.node_tree.nodes["Background"].inputs[0].default_value=(0.32,0.36,0.44,1)
w.node_tree.nodes["Background"].inputs[1].default_value=1.5
sd=bpy.data.lights.new("S",'SUN'); sd.energy=4.5
su=bpy.data.objects.new("S",sd); sc.collection.objects.link(su); su.rotation_euler=(math.radians(52),0,math.radians(-135))
cd=bpy.data.cameras.new("C"); cam=bpy.data.objects.new("C",cd); sc.collection.objects.link(cam); sc.camera=cam
for o in bpy.data.objects:
    if o.type=='MESH': o.hide_render=True
link.hide_render=False
lc=link.matrix_world.translation
cam.data.lens=60; cam.location=Vector((lc.x+0.5,lc.y-0.72,lc.z+0.46))
cam.rotation_euler=(Vector(lc)-cam.location).to_track_quat('-Z','Y').to_euler()

for tile_m in (0.15, 0.35, 0.80):
    f=(1.0/tile_m)/base
    for d,o0 in zip(uvl, orig): d.uv=(o0[0]*f, o0[1]*f)
    sc.render.filepath=os.path.join(OUT,f"K_link_tile_{int(tile_m*100):03d}cm.png")
    bpy.ops.render.render(write_still=True)
    print(f"rendered tile={tile_m} m  ({0.725/tile_m:.1f} repeats along the link)")
