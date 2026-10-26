"""render_tree.py — one headless Workbench render of a tree blend, so a human can see it.

    blender --background --factory-startup assets/trees/<id>/<id>.blend \\
        --python scripts/prop/render_tree.py -- --out <png> [--width 1280]

A camera is framed on the tree's bounds from the south-west, a sun is added, and the Workbench
engine draws material colours with studio lighting. Nothing is saved back to the blend.
"""

from __future__ import annotations

import argparse
import math
import os
import sys

import bpy
from mathutils import Vector


def bounds():
    lo = Vector((math.inf,) * 3)
    hi = Vector((-math.inf,) * 3)
    for obj in bpy.context.scene.objects:
        if obj.type != "MESH":
            continue
        for corner in obj.bound_box:
            point = obj.matrix_world @ Vector(corner)
            lo = Vector(min(a, b) for a, b in zip(lo, point))
            hi = Vector(max(a, b) for a, b in zip(hi, point))
    return lo, hi


def main(argv) -> int:
    parser = argparse.ArgumentParser(prog="render_tree.py", allow_abbrev=False)
    parser.add_argument("--out", required=True)
    parser.add_argument("--width", type=int, default=1280)
    arguments = parser.parse_args(argv)
    scene = bpy.context.scene
    bpy.context.view_layer.update()
    lo, hi = bounds()
    centre = (lo + hi) / 2.0
    height = hi.z - lo.z
    distance = height * 1.25
    eye = Vector((centre.x + distance * 0.7, centre.y - distance * 0.7, centre.z + height * 0.15))
    camera_data = bpy.data.cameras.new("RenderCamera")
    camera_data.lens = 45.0
    camera_data.clip_end = 10_000.0
    camera = bpy.data.objects.new("RenderCamera", camera_data)
    scene.collection.objects.link(camera)
    camera.location = eye
    camera.rotation_euler = (centre - eye).to_track_quat("-Z", "Y").to_euler()
    scene.camera = camera

    sun_data = bpy.data.lights.new("Sun", "SUN")
    sun_data.energy = 3.0
    sun = bpy.data.objects.new("Sun", sun_data)
    scene.collection.objects.link(sun)
    sun.rotation_euler = (math.radians(50.0), 0.0, math.radians(30.0))

    scene.render.engine = "BLENDER_WORKBENCH"
    scene.display.shading.light = "STUDIO"
    scene.display.shading.color_type = "MATERIAL"
    scene.display.shading.show_shadows = True
    scene.render.resolution_x = arguments.width
    scene.render.resolution_y = int(arguments.width * 0.75)
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.filepath = os.path.abspath(arguments.out)
    os.makedirs(os.path.dirname(scene.render.filepath), exist_ok=True)
    bpy.ops.render.render(write_still=True)
    print("render_tree ▸ {} ({} m tall)".format(scene.render.filepath, round(height, 2)),
          flush=True)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    code = main(argv)
    sys.stdout.flush()
    os._exit(code)
