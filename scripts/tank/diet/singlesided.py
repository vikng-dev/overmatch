"""Turn `doubleSided` off on the tank's materials (glTF default is single-sided)."""

import sys

import glblib

TARGET = sys.argv[1]
KEEP = set(sys.argv[2:])  # material names to leave double-sided

g = glblib.Glb.load(TARGET)
for m in g.gltf["materials"]:
    name = m.get("name")
    if name in KEEP:
        print(f"  keep doubleSided: {name}")
        continue
    if m.pop("doubleSided", None):
        print(f"  doubleSided -> false: {name}")

g.sync_buffer_len()
g.save(TARGET, json_pad_to=glblib.JSON_PAD)
left = [m.get("name") for m in g.gltf["materials"] if m.get("doubleSided")]
print(f"wrote {TARGET}; still doubleSided: {left or 'none'}")
