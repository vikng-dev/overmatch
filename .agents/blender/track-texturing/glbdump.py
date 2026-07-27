import json, struct, sys, hashlib
def load(p):
    f=open(p,'rb'); struct.unpack("<III",f.read(12))
    ln,_=struct.unpack("<II",f.read(8)); return json.loads(f.read(ln))
d=load(sys.argv[1])
out={}
out['nodes']=[n.get('name','?') for n in d.get('nodes',[])]
out['meshes']=[m.get('name','?') for m in d.get('meshes',[])]
out['materials']=[m.get('name','?') for m in d.get('materials',[])]
out['extras_nodes']=sorted(n.get('name','?') for n in d.get('nodes',[]) if 'extras' in n)
out['extensions']=d.get('extensionsUsed',[])
out['scenes']=[s.get('nodes') for s in d.get('scenes',[])]
prims={}
for m in d.get('meshes',[]):
    for i,p in enumerate(m.get('primitives',[])):
        prims[f"{m.get('name')}#{i}"]=sorted(p.get('attributes',{}).keys())+[f"mat={p.get('material')}"]
out['primitives']=prims
out['generator']=d.get('asset',{}).get('generator')
json.dump(out, open(sys.argv[2],'w'), indent=1, sort_keys=True)
print(f"{sys.argv[1]}: nodes={len(out['nodes'])} meshes={len(out['meshes'])} mats={len(out['materials'])} extras_on={len(out['extras_nodes'])} gen={out['generator']}")
