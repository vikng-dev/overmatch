# Tiger 1 track authoring — discussion agenda

Status: PREP (2026-07-17), for the session after the architecture doc settles. What already
exists in `assets/tiger_1/tiger_1.glb` (surveyed): `Wheel_L_0..7` / `Wheel_R_0..7` (the
interleaved Schachtellaufwerk, each with `_Visual` + `_Ballistic` children — already sim
entities via the bake), `Sprocket_L/R_Visual`, `Idler_L/R_Visual` (visual-only today), and
static `Track_Strip_*_Visual` + `Track_Treads_*_Visual` meshes (the offline geometry-nodes
track the procedural view will REPLACE).

## To decide with Yan (v2 — reconciled with codex arch review)

1. **Rig completion**: add plain `Sprocket_L/R` + `Idler_L/R` rig nodes (pivot at wheel
   center). Radii: bake gains a subtree-bounds extraction (today it only captures
   collision/ballistic meshes) for wheel/idler VISUAL radii, spec override allowed. The
   sprocket's PITCH radius is never measured — it derives from `pitch × teeth` (§7 of the
   architecture doc); mesh bounds are render/validation only.
2. **Axle grouping for the Schachtellaufwerk** (the codex-critical one): each of the 8 axles
   per side contributes exactly ONE route circle and ONE suspension station; the interleaved
   discs at their different lateral planes are the axle's visual SUBTREE (children of the axle
   node, or an explicit `spin_nodes` list in the spec). Never one circle per disc — coincident
   circles break the tangent builder, duplicate stations double suspension forces. Question
   for the Blender side: can `Wheel_L_N` own its discs as children, or do we list them?
3. **Belt plane**: one 2D route plane per side at track centerline (all rows share the belt).
   Confirm visually against the interleaved silhouette.
4. **Link mesh**: one `TrackLink` form (Tiger single-form; pitch 130 mm → link_count set
   EXPLICITLY in spec, `MaterialLoop { pitch, link_count }` is authoritative — geometry
   reconciles via the tensioner, links are never stretched). Authoring frame convention to
   fix: tangent/outward/width axes declared in the spec's `link_mesh.frame`. Guide horn
   included? LOD mesh for far tiers?
5. **Instancing path**: entity-per-link is bring-up only; scalable path uploads packed
   instance buffers grouped by mesh form. Shadows: full links + shadows near tier only; far =
   decimated route ribbon, no per-link shadows.
6. **Sprocket tooth lock**: `angle = phase · TAU / (pitch · teeth) + baked_marker_offset`.
   Needs an authored radial marker node (`Sprocket_Phase_L/R`) — a tooth-gap alignment cannot
   be inferred from mesh bounds. Add the two empties in Blender; bake extracts the offset.
7. **Front sprocket**: derived from the typed sprocket node's position (no `sprocket: Front`
   spec field to disagree with geometry). Verify loop-direction conventions survive the
   front-drive flip — Tiger is the test case for the parked T-34 rear-sprocket item's mirror.
8. **Track type preset**: `DryPin` — preset carries the friction COEFFICIENT; pin geometry
   (radius) is authored per track; torque emerges from tension. Verify real Tiger numbers
   before authoring: pitch 130 mm, width 725 mm, link mass ~30 kg (confirm).
9. **What dies**: `Track_Strip_*` / `Track_Treads_*` are HIDDEN by phase A (double-render
   otherwise); keep behind a debug toggle during bring-up, delete after.
10. **Suspension travel source**: view wheels follow field-driven visual lift at the presented
    pose (NOT tick-world `Suspension.contact` — wrong hull position during corrections), and
    the track view writes GLB view nodes only, never the sim roadwheel entities.
