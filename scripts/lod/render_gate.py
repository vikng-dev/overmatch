"""The rendered-difference gate: the authoritative attribute check (ADR 0033 §7).

WHY THIS AND NOT A NORMAL-ANGLE THRESHOLD. Positional deviation is blind to shading, and a wrongly
lit pixel is visible over its triangle's whole projected AREA rather than its sub-pixel
displacement — so shading needs its own gate. But no fixed angle can honestly set it: how visible a
normal error is depends on roughness, on the normal map, on the light. So the gate renders instead.
Each kept level is put where the runtime will first show it — the parent->child switch distance —
under the asset's OWN material, and differenced against the level it replaces. That is the pop, in
pixels, and it subsumes normal, UV and tangent thresholds nobody can defend numerically.

WHAT IS RENDERED IS THE SHIPPED BYTES. The meshes handed to the renderer are rebuilt from the
decoded GLB — positions, split normals and UVs as the file carries them — not from the Blender
datablocks that went into the exporter. Same rule as every other gate.

THE CAMERA MATCHES ANGULAR RESOLUTION, NOT PIXEL COUNT. At its switch distance a track shoe covers
a few dozen pixels of a 2160-pixel display; rendering the reference frame would spend 4.6 megapixels
to look at forty of them, and any frame-wide statistic would divide the difference by the empty sky.
So the tile is small and its FOV is shrunk to keep pixels-per-radian identical to the reference
view: `vfov = tile_px / (height_px / (2 tan(vfov_ref/2)))`. One tile pixel subtends exactly what one
reference-display pixel subtends. It is then SUPERSAMPLED and box-averaged down, because a player's
pixel integrates over its solid angle and a one-sample-per-pixel comparison is an aliasing contest
rather than a measurement.

THREE THINGS THIS GATE DELIBERATELY DOES NOT MEASURE, each learned by measuring it wrongly first:

  * SAMPLER NOISE. Cycles is Monte-Carlo; two renders of the same mesh differ by percent-scale
    amounts. Every pair renders its parent twice, and that control is one end of the bracket the
    verdict is scored against. Without it, all four levels failed at a uniform ~0.05 — the noise.
  * THE SILHOUETTE. Silhouette error IS surface deviation, already certified under budget with a
    proof. An edge pixel allowed to move one pixel changes by 1.0 by construction, so counting the
    boundary charges the level twice and, on a 40-pixel asset, guarantees ~10 % "gross" pixels.
    Statistics are taken over the interior where both images are fully covered.
  * ABSOLUTE MAGNITUDE. "A LOD switch may change at most 2 % of its pixels by 0.1" is a number
    nobody can defend, and measured, NO geometric decimation passes it — flat-shaded facets merge
    and their shading changes, which is the mechanism working, not failing. So the verdict is a
    POSITION BETWEEN TWO MEASURED REFERENCES: the renderer's own noise at one end, and the same
    level with deliberately broken shading normals at the other. See `compare`.
"""

import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Production, shared with the verifier so the tile FOV is ONE expression, not two.
from manifest import tile_vfov_rad  # noqa: E402


def _engine(scene):
    """Cycles on the CPU.

    EEVEE would be faster and is not usable here: it needs a live GPU context, which a
    `--background` Blender does not always have, and its result depends on the box. The gate has to
    produce the same number on a laptop and in CI or it is not a gate. The scene is one object and
    two lights, so even supersampled the cost is seconds per frame.
    """
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    return scene.render.engine


def _tilted(normals, degrees):
    """Every shading normal rotated by `degrees` about +Z. The DEFECT REFERENCE, not a variant.

    This is the red-test class made renderable: geometry that is positionally perfect and lit
    wrongly. A defaulted tangent, a dropped custom-normal layer and a mis-imported smoothing group
    all present as exactly this — the surface is where it should be and the light comes off it at
    the wrong angle, over the triangle's whole projected AREA rather than at its edge.
    """
    if degrees <= 0.0:
        return normals
    angle = math.radians(degrees)
    axis = np.array([0.0, 0.0, 1.0])
    cos, sin = math.cos(angle), math.sin(angle)
    dot = normals @ axis
    tilted = (
        normals * cos
        + np.cross(np.broadcast_to(axis, normals.shape), normals) * sin
        + np.broadcast_to(axis, normals.shape) * dot[..., None] * (1.0 - cos)
    )
    lengths = np.linalg.norm(tilted, axis=-1, keepdims=True)
    return tilted / np.maximum(lengths, 1e-30)


def _mesh_from_surface(surface, name, tilt_deg=0.0):
    """Rebuild a Blender mesh from a decoded `Surface` — shipped positions, normals and UVs."""
    import bpy

    mesh = bpy.data.meshes.new(name)
    verts = [tuple(v) for v in surface.verts]
    faces = [tuple(int(i) for i in t) for t in surface.tri_v]
    mesh.from_pydata(verts, [], faces)
    mesh.update()

    uv_layer = mesh.uv_layers.new(name="UVMap")
    flat = np.zeros((len(mesh.loops), 2), dtype=np.float32)
    for triangle, polygon in zip(surface.corner_uv, mesh.polygons):
        for corner, loop_index in enumerate(polygon.loop_indices):
            flat[loop_index] = triangle[corner]
    uv_layer.uv.foreach_set("vector", flat.reshape(-1))

    normals = np.zeros((len(mesh.loops), 3), dtype=np.float32)
    for triangle, polygon in zip(_tilted(surface.corner_n, tilt_deg), mesh.polygons):
        for corner, loop_index in enumerate(polygon.loop_indices):
            normals[loop_index] = triangle[corner]
    mesh.normals_split_custom_set([tuple(n) for n in normals])
    mesh.update()
    return mesh


def _build_scene(config, material):
    """A fixed studio: one key, one rim, a dark world. Deterministic across runs and machines."""
    import bpy

    scene = bpy.context.scene
    _engine(scene)
    scene.cycles.samples = config["samples"]
    scene.cycles.use_denoising = False
    scene.cycles.seed = config["seed"]
    scene.render.resolution_x = config["tile_px"] * config["supersample"]
    scene.render.resolution_y = config["tile_px"] * config["supersample"]
    scene.render.resolution_percentage = 100
    # TRANSPARENT FILM, and it is the footprint mechanism rather than a look. The alpha channel IS
    # the silhouette: it says which pixels the asset covers without anyone having to guess what the
    # background renders as after the view transform (it is not the world colour — AgX moves it),
    # and a silhouette that MOVED shows up as an alpha difference, which is exactly the change a
    # coarse level makes and the one a background-colour threshold would hide.
    scene.render.film_transparent = True
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.render.image_settings.color_depth = "8"
    transforms = [
        item.identifier
        for item in scene.view_settings.bl_rna.properties["view_transform"].enum_items
    ]
    for wanted in ("AgX", "Filmic", "Standard"):
        if wanted in transforms:
            scene.view_settings.view_transform = wanted
            break

    world = bpy.data.worlds.new("LodGateWorld")
    world.use_nodes = True
    background = world.node_tree.nodes["Background"]
    background.inputs[0].default_value = (0.03, 0.035, 0.04, 1.0)
    background.inputs[1].default_value = 1.0
    scene.world = world

    key = bpy.data.objects.new("Key", bpy.data.lights.new("KeyL", "AREA"))
    key.data.energy = 220.0
    key.data.size = 0.55
    key.location = (0.55, -0.75, 0.75)
    key.rotation_euler = (math.radians(48), 0.0, math.radians(36))
    scene.collection.objects.link(key)

    rim = bpy.data.objects.new("Rim", bpy.data.lights.new("RimL", "AREA"))
    rim.data.energy = 90.0
    rim.data.size = 0.9
    rim.location = (-0.7, 0.6, 0.35)
    rim.rotation_euler = (math.radians(75), 0.0, math.radians(-140))
    scene.collection.objects.link(rim)

    camera = bpy.data.objects.new("LodGateCam", bpy.data.cameras.new("LodGateCamD"))
    camera.data.sensor_fit = "VERTICAL"
    scene.collection.objects.link(camera)
    scene.camera = camera
    return scene, camera, material


def _aim(camera, centre, distance, elevation_deg, azimuth_deg):
    """Place the camera `distance` metres away on the given elevation/azimuth, looking at `centre`.

    THE CLIP RANGE IS PART OF AIMING. Blender's default `clip_end` is 100 m; a level whose switch
    distance is 335 m would sit entirely behind the far plane, both renders would come back empty,
    the footprint would be zero pixels and the gate would report a flawless PASS on two blank
    images. Bracketing the clip range around the actual distance is what stops this gate from
    silently measuring nothing.
    """
    from mathutils import Vector

    camera.data.clip_start = max(0.01, distance * 0.5)
    camera.data.clip_end = distance * 2.0 + 10.0
    e, a = math.radians(elevation_deg), math.radians(azimuth_deg)
    camera.location = Vector(centre) + Vector((
        distance * math.cos(e) * math.cos(a),
        distance * math.cos(e) * math.sin(a),
        distance * math.sin(e),
    ))
    direction = (Vector(centre) - camera.location).normalized()
    camera.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def _render(scene, path, supersample):
    """Render, then box-average `supersample` x `supersample` blocks down to one player pixel."""
    import bpy

    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    image = bpy.data.images.load(path)
    pixels = np.array(image.pixels[:], dtype=np.float32).reshape(
        image.size[1], image.size[0], image.channels
    )
    bpy.data.images.remove(image)
    if pixels.shape[2] < 4:
        raise RuntimeError(f"{path}: expected RGBA, got {pixels.shape[2]} channels")
    pixels = pixels[:, :, :4]
    if supersample > 1:
        height, width, channels = pixels.shape
        pixels = pixels.reshape(
            height // supersample, supersample, width // supersample, supersample, channels
        ).mean(axis=(1, 3))
    return pixels


def _difference(a, b, render_config):
    """Shading difference over the INTERIOR of the silhouette. All 0..1 sRGB.

    THE SILHOUETTE BAND IS EXCLUDED, AND THAT IS THE WHOLE DESIGN OF THIS METRIC. This gate exists
    to measure what positional deviation CANNOT SEE — shading. Silhouette is not in that category:
    silhouette error IS surface deviation, and the deviation gate has already certified it to be
    under the pixel budget with a proof. Counting the boundary here would charge the level twice
    for the same, already-certified, already-within-budget error.

    And it would do so at a scale that makes the gate meaningless. A 1-pixel budget means the
    surface may sit up to one pixel from where the source put it; an edge pixel that moves by one
    pixel goes from covered to uncovered, a difference of 1.0 — thirty times the "large difference"
    threshold. On a 40-pixel-wide asset the boundary is a tenth of every pixel it covers, so a level
    sitting exactly ON its budget scores ~10 % of pixels "grossly different". Measured: the first
    two runs of this gate failed every level at 10-25 %, which is the perimeter-to-area ratio of a
    small object, not a defect in any of them.

    So the statistics are taken where BOTH images are fully covered, eroded by one pixel to drop
    partial-coverage antialiasing, and over RGB only (alpha is 1 everywhere in that region by
    construction). The boundary is still reported, as `silhouette_band_frac`, because it is
    informative — it is simply not what this gate is asking about.
    """
    # Full coverage in BOTH images is the interior test, and supersampling is what makes it exact:
    # alpha is the fraction of sub-samples that hit, so alpha == 1 means every one of the 16
    # sub-pixels landed on both meshes. No morphological erosion on top — at the far switches the
    # asset is thirteen pixels across, and eroding that leaves nothing to measure, which is how the
    # previous run produced a gate result of 0.0000 for a level it had simply failed to look at.
    interior = (a[:, :, 3] >= 0.999) & (b[:, :, 3] >= 0.999)
    any_cover = (a[:, :, 3] > 0.01) | (b[:, :, 3] > 0.01)
    band = int(any_cover.sum()) - int(interior.sum())

    delta = np.abs(a[:, :, :3] - b[:, :, :3]).max(axis=2)
    count = int(interior.sum())
    if count == 0:
        return {"footprint_px": 0, "silhouette_band_px": band, "silhouette_band_frac": 1.0,
                "mean_abs_diff": 0.0, "p99_abs_diff": 0.0, "max_abs_diff": 0.0, "frac_over": 0.0}
    values = delta[interior]
    return {
        "footprint_px": count,
        "silhouette_band_px": band,
        "silhouette_band_frac": round(band / max(1, int(any_cover.sum())), 6),
        "mean_abs_diff": round(float(values.mean()), 6),
        "p99_abs_diff": round(float(np.percentile(values, 99)), 6),
        "max_abs_diff": round(float(values.max()), 6),
        "frac_over": round(float((values > render_config["over_threshold"]).mean()), 6),
    }


def compare(pairs, material, render_config, view, out_dir):
    """Render each (label, parent_surface, child_surface, distance_m) pair and difference them.

    THE THRESHOLD IS BRACKETED BY TWO MEASURED REFERENCES, because an absolute pixel number for
    "how different may a LOD switch look" is not a thing anyone can defend. Every pair renders four
    frames per view:

        parent(seed)     the level being replaced
        parent(seed+1)   the NOISE FLOOR — Cycles disagreeing with itself on identical geometry
        child(seed)      the level under test: the SIGNAL
        parent, tilted   the DEFECT FLOOR — the PARENT with every shading normal rotated by a
                         declared angle: same geometry, lit wrong, i.e. the red-test class in
                         isolation. Tilting the child instead would fold the child's ordinary LOD
                         difference into the denominator and deflate every score.

    and the verdict is where the signal sits between them:

        score = (signal - noise) / (defect - noise)

    0 means the new level is as good as re-rendering the old one; 1 means it looks as wrong as a
    level with broken normals. `defect_fraction` declares how far along that line a switch may land.
    Nothing here is in pixel units, so the verdict does not move when the sample count, the
    denoiser, the tile size or the machine changes — the three references move together.

    The absolute thresholds remain as a floor, so a degenerate bracket (a defect reference that
    somehow renders identically) cannot license an arbitrarily large difference.

    Renders are written to `out_dir` so a failure can be looked at rather than argued about.
    """
    import bpy

    os.makedirs(out_dir, exist_ok=True)
    scene, camera, material = _build_scene(render_config, material)

    # One object, whose mesh datablock is swapped per variant — the material, the transform and the
    # scene membership stay put, so the only thing that changes between two renders is the geometry.
    # `parked` exists so a mesh is never removed while it is still the holder's data.
    parked = bpy.data.meshes.new("lod_gate_parked")
    holder = bpy.data.objects.new("LodGateProbe", parked)
    scene.collection.objects.link(holder)
    seed = render_config["seed"]
    tilt = render_config["defect_normal_deg"]

    results = []
    for label, parent, child, distance in pairs:
        centre = tuple(0.5 * (parent.bbox_min + parent.bbox_max))
        camera.data.lens_unit = "FOV"
        camera.data.angle = tile_vfov_rad(render_config, view)

        images = {}
        for role, surface, role_seed, role_tilt in (
            ("parent", parent, seed, 0.0),
            ("control", parent, seed + 1, 0.0),
            ("child", child, seed, 0.0),
            # THE DEFECT IS THE PARENT WITH BROKEN NORMALS, NOT THE CHILD. Tilting the child put its
            # ordinary LOD difference into the denominator alongside the injected defect, so the
            # bracket measured "decimation plus broken normals" and every score was deflated by
            # however large the decimation difference already was. Tilting the PARENT isolates the
            # defect: parent-vs-tilted-parent differs by the normals and by nothing else.
            ("defect", parent, seed, tilt),
        ):
            mesh = _mesh_from_surface(surface, f"{label}_{role}", role_tilt)
            mesh.materials.clear()
            mesh.materials.append(material)
            holder.data = mesh
            scene.cycles.seed = role_seed
            for view_name, elevation, azimuth in render_config["views"]:
                _aim(camera, centre, distance, elevation, azimuth)
                path = os.path.join(out_dir, f"{label}__{view_name}__{role}.png")
                images[(view_name, role)] = _render(scene, path, render_config["supersample"])
            holder.data = parked
            bpy.data.meshes.remove(mesh)

        per_view = {}
        passed = True
        worst_mean, worst_frac, worst_score = 0.0, 0.0, 0.0
        for view_name, _elevation, _azimuth in render_config["views"]:
            frame = images[(view_name, "parent")]
            signal = _difference(frame, images[(view_name, "child")], render_config)
            noise = _difference(frame, images[(view_name, "control")], render_config)
            defect = _difference(frame, images[(view_name, "defect")], render_config)

            span = defect["mean_abs_diff"] - noise["mean_abs_diff"]
            score = (
                (signal["mean_abs_diff"] - noise["mean_abs_diff"]) / span if span > 1e-9 else 1.0
            )
            score = max(0.0, score)
            floor_ok = (
                signal["mean_abs_diff"] <= render_config["max_mean_abs_diff"]
                and signal["frac_over"] <= render_config["max_footprint_frac_over"]
            )
            # AN UNRESOLVABLE VIEW IS A FAILURE, NOT A PASS. Two frames with no interior in common
            # differ by nothing, so a camera that framed the asset out — behind a clip plane, or too
            # small to cover a whole pixel — would score a flawless zero.
            view_pass = signal["footprint_px"] > 0 and (
                score <= render_config["defect_fraction"] or floor_ok
            )
            passed = passed and view_pass
            per_view[view_name] = {
                "signal": signal,
                "noise_floor": noise,
                "defect_floor": defect,
                "defect_score": round(score, 6),
                "under_absolute_floor": bool(floor_ok),
                "pass": bool(view_pass),
            }
            worst_mean = max(worst_mean, signal["mean_abs_diff"])
            worst_frac = max(worst_frac, signal["frac_over"])
            worst_score = max(worst_score, score)

        results.append({
            "label": label,
            "distance_m": round(distance, 3),
            "tile_px": render_config["tile_px"],
            "tile_vfov_rad": round(tile_vfov_rad(render_config, view), 8),
            "samples": render_config["samples"],
            "supersample": render_config["supersample"],
            "views": per_view,
            "worst_mean_abs_diff": round(worst_mean, 6),
            "worst_frac_over": round(worst_frac, 6),
            "worst_defect_score": round(worst_score, 6),
            "thresholds": {
                "max_mean_abs_diff": render_config["max_mean_abs_diff"],
                "max_footprint_frac_over": render_config["max_footprint_frac_over"],
                "over_threshold": render_config["over_threshold"],
                "defect_fraction": render_config["defect_fraction"],
                "defect_normal_deg": render_config["defect_normal_deg"],
            },
            "pass": bool(passed),
        })
    bpy.data.objects.remove(holder, do_unlink=True)
    bpy.data.meshes.remove(parked)
    return results
