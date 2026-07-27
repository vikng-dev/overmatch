import bpy, bmesh, math, os
from mathutils import Vector
OUT="/private/tmp/claude-502/-Users-Yan-Desktop-github-vikng-dev-personal-overmatch/aa6ae501-da41-487d-a38f-23a2004cf55d/scratchpad/tex"

def uv_area(poly, uvl):
    pts=[Vector(uvl[i].uv) for i in poly.loop_indices]
    a=0.0
    for i in range(1,len(pts)-1):
        u,v=pts[i]-pts[0], pts[i+1]-pts[0]
        a+=abs(u.x*v.y-u.y*v.x)/2
    return a

def stats(obj, texsize, label):
    me=obj.data
    if not me.uv_layers: print(f"{label}: NO UVs"); return
    uvl=me.uv_layers[0].data
    a3=sum(p.area for p in me.polygons)
    au=sum(uv_area(p,uvl) for p in me.polygons)
    dens=[]
    for p in me.polygons:
        ua=uv_area(p,uvl)
        if p.area>1e-9 and ua>1e-12:
            dens.append(math.sqrt(ua)*texsize/math.sqrt(p.area))
    dens.sort()
    med=dens[len(dens)//2] if dens else 0
    lo=dens[int(len(dens)*0.05)] if dens else 0
    hi=dens[int(len(dens)*0.95)] if dens else 0
    print(f"{label}: 3D area={a3:.3f} m^2  UV coverage={au*100:.1f}% of the square  "
          f"texel density @{texsize}: median={med:.0f} px/m (5th={lo:.0f}, 95th={hi:.0f})  "
          f"stretch ratio p95/p5={hi/lo if lo else 0:.1f}x")

print("=== EXISTING PARTS (how your good atlas is laid out) ===")
stats(bpy.data.objects["Hull_Visual"], 4096, "Hull_Visual @4k atlas ")
stats(bpy.data.objects["Turret_Visual"], 4096, "Turret_Visual @4k atlas")
stats(bpy.data.objects["Wheel_L_3"], 4096, "Wheel_L_3 (orphan UVs) ")

print()
print("=== UNWRAPPING THE LINK ===")
link=bpy.data.objects["Link"]
for o in bpy.data.objects: o.select_set(False)
bpy.context.view_layer.objects.active=link; link.select_set(True)
bpy.ops.object.mode_set(mode='EDIT')
bpy.ops.mesh.select_all(action='SELECT')
bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.003)
bpy.ops.object.mode_set(mode='OBJECT')

bm=bmesh.new(); bm.from_mesh(link.data)
uvlay=bm.loops.layers.uv.active
seen=set(); islands=0
for f in bm.faces:
    if f.index in seen: continue
    islands+=1; stack=[f]
    while stack:
        cur=stack.pop()
        if cur.index in seen: continue
        seen.add(cur.index)
        for l in cur.loops:
            for lf in l.edge.link_faces:
                if lf.index in seen: continue
                shared=[(round(ll[uvlay].uv.x,5),round(ll[uvlay].uv.y,5)) for ll in cur.loops if ll.vert in lf.verts]
                other=[(round(ll[uvlay].uv.x,5),round(ll[uvlay].uv.y,5)) for ll in lf.loops if ll.vert in cur.verts]
                if set(shared)&set(other): stack.append(lf)
bm.free()
print(f"Smart UV Project produced {islands} islands from {len(link.data.polygons)} faces")
for ts in (512,1024,2048):
    stats(link, ts, f"Link @{ts}px          ")

# ---- draw the UV layout ----
S=900
img=bpy.data.images.new("uvlayout", S, S)
px=[0.08,0.08,0.10,1.0]*(S*S)
def plot(x,y,c=(0.55,0.95,0.65)):
    if 0<=x<S and 0<=y<S:
        i=(y*S+x)*4; px[i],px[i+1],px[i+2]=c
def line(a,b):
    x0,y0=int(a[0]*(S-1)),int(a[1]*(S-1)); x1,y1=int(b[0]*(S-1)),int(b[1]*(S-1))
    dx,dy=abs(x1-x0),-abs(y1-y0); sx=1 if x0<x1 else -1; sy=1 if y0<y1 else -1; err=dx+dy
    while True:
        plot(x0,y0)
        if x0==x1 and y0==y1: break
        e2=2*err
        if e2>=dy: err+=dy; x0+=sx
        if e2<=dx: err+=dx; y0+=sy
uvl=link.data.uv_layers[0].data
for p in link.data.polygons:
    li=list(p.loop_indices)
    for k in range(len(li)):
        line(uvl[li[k]].uv, uvl[li[(k+1)%len(li)]].uv)
img.pixels=px
img.filepath_raw=os.path.join(OUT,"G_uv_layout.png"); img.file_format='PNG'; img.save()
print("saved G_uv_layout.png")
