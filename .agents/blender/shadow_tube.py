"""shadow_tube.py — author a barrel's shadow tube into a tank blend, headless.

    blender -b --factory-startup assets/<id>/<id>.blend --python .agents/blender/shadow_tube.py -- \
        --barrel <barrel mesh object> --parent <recoiling node> --name <new object> \
        [--sides 12] [--tolerance-mm 4] [--material Mat_Shadow_Proxy] [--collection ShadowProxy]

The tube is a closed lathe around the barrel mesh's long axis, parented under `--parent` with an
identity transform, so it follows the barrel's elevation and recoil. It is declared in the sheet's
`shadow_proxies` and casts the barrel's shadow at range (`tank::shadow_proxy`); it collides with
nothing and no camera draws it.

THE TUBE CONTAINS THE BARREL'S OUTER ENVELOPE, so its shadow is never thinner than the barrel's:
* the profile is the largest radius of the mesh's edges over each vertex station along the axis,
  simplified by Douglas-Peucker to `--tolerance-mm`, then each kept station raised by the largest
  excess of the stations it dropped — so the profile is at or above every station and at most
  twice the tolerance over;
* the cross-section is a `--sides`-gon whose flats are tangent to the profile radius, so the
  silhouette is between 1 and `1/cos(pi/sides)` times the barrel's width (1.035 at 12 sides).

It writes the blend in place and removes Blender's `.blend1` backup. Run `scripts/tank/build.py
build` afterwards: the trio is the only way the tube reaches the glb.
"""

import argparse
import math
import os
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector


def arguments():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="shadow_tube.py")
    parser.add_argument("--barrel", required=True)
    parser.add_argument("--parent", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--material", default="Mat_Shadow_Proxy")
    parser.add_argument("--collection", default="ShadowProxy")
    parser.add_argument("--sides", type=int, default=12)
    parser.add_argument("--tolerance-mm", type=float, default=4.0)
    return parser.parse_args(argv)


def envelope(points, edges, axis, centre):
    """(station along the axis, largest radius of the surface there), ascending, one entry per
    vertex station.

    Taken over the mesh's EDGES, each spanning its stations with its radius interpolated, and not
    over the vertices at a station alone: a station where only inner vertices sit (a muzzle brake's
    baffle) still lies under the outer surface spanning it."""
    polar = []
    for p in points:
        offset = p - centre
        along = offset.dot(axis)
        polar.append((along, (offset - axis * along).length))
    stations = sorted({round(along, 5) for along, _ in polar})
    radius = {station: 0.0 for station in stations}
    for a, b in edges:
        (a0, r0), (a1, r1) = sorted((polar[a], polar[b]))
        for station in stations:
            if station < round(a0, 5) or station > round(a1, 5):
                continue
            if a1 - a0 > 1e-9:
                t = min(max((station - a0) / (a1 - a0), 0.0), 1.0)
                spanned = r0 + t * (r1 - r0)
            else:
                spanned = max(r0, r1)
            radius[station] = max(radius[station], spanned)
    return [(station, radius[station]) for station in stations]


def simplify(profile, tolerance):
    """Douglas-Peucker on radius over station: the indices kept."""
    keep = {0, len(profile) - 1}
    stack = [(0, len(profile) - 1)]
    while stack:
        first, last = stack.pop()
        (a0, r0), (a1, r1) = profile[first], profile[last]
        worst, at = 0.0, None
        for i in range(first + 1, last):
            a, r = profile[i]
            t = (a - a0) / (a1 - a0) if a1 != a0 else 0.0
            excess = abs(r - (r0 + t * (r1 - r0)))
            if excess > worst:
                worst, at = excess, i
        if at is not None and worst > tolerance:
            keep.add(at)
            stack += [(first, at), (at, last)]
    return sorted(keep)


def raised(profile, kept):
    """The kept stations, each raised by the largest excess of the rings dropped beside it."""
    lift = {i: 0.0 for i in kept}
    for first, last in zip(kept, kept[1:]):
        (a0, r0), (a1, r1) = profile[first], profile[last]
        excess = 0.0
        for i in range(first + 1, last):
            a, r = profile[i]
            t = (a - a0) / (a1 - a0)
            excess = max(excess, r - (r0 + t * (r1 - r0)))
        lift[first] = max(lift[first], excess)
        lift[last] = max(lift[last], excess)
    return [(profile[i][0], profile[i][1] + lift[i]) for i in kept]


def lathe(stations, axis, centre, sides, name):
    """A closed, outward-wound lathe mesh with a UV layer, in the frame `axis` and `centre` are in."""
    side_a = axis.orthogonal().normalized()
    side_b = axis.cross(side_a).normalized()
    circumscribe = 1.0 / math.cos(math.pi / sides)
    bm = bmesh.new()
    rings = []
    for along, radius in stations:
        ring = []
        for k in range(sides):
            # Vertices half a step round, so each flat's midpoint lies on the profile radius.
            theta = 2.0 * math.pi * (k + 0.5) / sides
            offset = (side_a * math.cos(theta) + side_b * math.sin(theta)) * radius * circumscribe
            ring.append(bm.verts.new(centre + axis * along + offset))
        rings.append(ring)
    faces = []
    for lower, upper in zip(rings, rings[1:]):
        for k in range(sides):
            k1 = (k + 1) % sides
            faces.append(bm.faces.new((lower[k], lower[k1], upper[k1], upper[k])))
    # Caps as fans: a polygon over four corners stops Blender's tangent export for the whole mesh.
    for ring in (rings[0], rings[-1]):
        for k in range(1, sides - 1):
            faces.append(bm.faces.new((ring[0], ring[k], ring[k + 1])))
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    uv = bm.loops.layers.uv.new("UVMap")
    span = stations[-1][0] - stations[0][0]
    for face in bm.faces:
        for loop in face.loops:
            offset = loop.vert.co - centre
            along = offset.dot(axis)
            angle = math.atan2(offset.dot(side_b), offset.dot(side_a)) % (2.0 * math.pi)
            if len(face.verts) == 3:
                # A cap: its outline, flat.
                loop[uv].uv = (0.5 + 0.5 * math.cos(angle), 0.5 + 0.5 * math.sin(angle))
            else:
                loop[uv].uv = (angle / (2.0 * math.pi), (along - stations[0][0]) / span)
    # A side quad's last column wraps round to the first; give it u = 1 rather than 0.
    for face in bm.faces:
        if len(face.verts) == 4:
            us = [loop[uv].uv.x for loop in face.loops]
            if max(us) - min(us) > 0.5:
                for loop in face.loops:
                    if loop[uv].uv.x < 0.5:
                        loop[uv].uv.x += 1.0
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def collection_for(barrel, name):
    """The shadow proxies' own collection, beside the barrel's: created under the collection that
    holds the barrel's, or under the scene's when nothing does."""
    existing = bpy.data.collections.get(name)
    if existing is not None:
        return existing
    created = bpy.data.collections.new(name)
    home = barrel.users_collection[0]
    holders = [c for c in bpy.data.collections if home.name in c.children]
    (holders[0] if holders else bpy.context.scene.collection).children.link(created)
    return created


def main():
    args = arguments()
    if bpy.data.objects.get(args.name) is not None:
        sys.exit("shadow_tube: `{}` already exists; delete it first to re-author".format(args.name))
    barrel = bpy.data.objects[args.barrel]
    parent = bpy.data.objects[args.parent]
    bpy.context.view_layer.update()
    # The barrel's vertices in the parent's frame — the frame the tube's mesh is authored in.
    to_parent = parent.matrix_world.inverted() @ barrel.matrix_world
    points = [to_parent @ v.co for v in barrel.data.vertices]
    lo = Vector([min(p[i] for p in points) for i in range(3)])
    hi = Vector([max(p[i] for p in points) for i in range(3)])
    extent = hi - lo
    long_axis = max(range(3), key=lambda i: extent[i])
    axis = Vector([1.0 if i == long_axis else 0.0 for i in range(3)])
    centre = (lo + hi) / 2.0
    centre[long_axis] = 0.0
    edges = [tuple(edge.vertices) for edge in barrel.data.edges]
    profile = envelope(points, edges, axis, centre)
    tolerance = args.tolerance_mm / 1000.0
    stations = raised(profile, simplify(profile, tolerance))
    mesh = lathe(stations, axis, centre, args.sides, args.name)
    material = bpy.data.materials.get(args.material) or bpy.data.materials.new(args.material)
    material.diffuse_color = (0.55, 0.2, 0.75, 1.0)
    mesh.materials.append(material)
    tube = bpy.data.objects.new(args.name, mesh)
    collection_for(barrel, args.collection).objects.link(tube)
    tube.parent = parent
    tube.matrix_parent_inverse = Matrix.Identity(4)
    tube.matrix_basis = Matrix.Identity(4)
    bpy.context.view_layer.update()
    print("shadow_tube: {} under {}: {} rings of {} from {} ({} triangles), stations {}".format(
        args.name, args.parent, len(stations), args.sides, args.barrel,
        sum(len(p.vertices) - 2 for p in mesh.polygons),
        [(round(a, 4), round(r, 4)) for a, r in stations]))
    bpy.ops.wm.save_mainfile()
    backup = bpy.data.filepath + "1"
    if os.path.exists(backup):
        os.remove(backup)


main()
