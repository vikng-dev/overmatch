"""The game's damage-model tooling fixture: crew are HP pools at stations.

fixture_tank.py — the synthetic trio the door's end-to-end suite runs against.

    blender --background --factory-startup \\
      --python .agents/blender/fixture_tank.py -- --dir <workdir> [--defect <name>]

Writes a whole asset under `<workdir>`, in the layout the door derives every path from:

    assets/materials/materials.blend    the canonical substance library, linked from below
    assets/materials/materials.ron      its numeric half, which the consumer contract is handed
    assets/testbed/testbed.blend        the model: a hull, a collision proxy, two roadwheels
    assets/testbed/testbed.tank.ron     the spec sheet those nodes are declared in
    assets/testbed/testbed_*.png        one texture per colour role, so the chain has work to do

It is the smallest thing that is a TANK to every stage of the door: the source pass finds a clean
source, the consumer contract finds two watertight ballistic wheels and a usable collision proxy,
and the texture derivation finds a colour map, a normal map and a data map to bake — one per role,
which is what makes the derivation's role, transfer and tangent laws apply to it at all. Nothing
here is Tiger-shaped — the door is generic, so its fixture is a second vehicle.

The admission sheet states a nominal construction. Its sourced masses and inertias are `Derived`
from the design inputs in their notes. `Fixture` is reserved for inline Rust test data; the ordinary
door refuses it in files, including this generated sheet.

`--defect` builds the same trio with exactly one thing wrong, which is how the suite proves that a
refusal at each stage leaves the tracked glb untouched.
"""

import argparse
import os
import shutil
import sys

import bpy

#: This work tree: `<root>/.agents/blender/fixture_tank.py`.
_REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

#: A unit cube, wound outward: eight welded corners and six quads. Watertight, positive volume,
#: every directed edge once — what the consumer contract requires of a ballistic primitive.
CUBE_VERTICES = (
    (-0.5, -0.5, -0.5), (0.5, -0.5, -0.5), (0.5, 0.5, -0.5), (-0.5, 0.5, -0.5),
    (-0.5, -0.5, 0.5), (0.5, -0.5, 0.5), (0.5, 0.5, 0.5), (-0.5, 0.5, 0.5),
)
CUBE_FACES = (
    (0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7),
)

#: One face's UV corners, repeated per face: a square each, so no sampled UV triangle is collapsed.
FACE_UV = ((0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0))

#: The substances the wheels wear, linked out of the library — real registry keys, because the
#: consumer contract classifies a primitive by the material name the glTF carries.
SUBSTANCES = ("RHA", "MildSteel", "Flesh")

SPEC = """\
#![enable(implicit_some)]
// Nominal construction for the asset door. The sourced assembly fields state their design inputs
// and derivations; this generated sheet crosses the ordinary, non-test admission boundary.
TankSpec(
    display_name: "Testbed",
    mass: 1000.0,
    inertia_extents: (2.0, 1.0, 3.0),
    turret: (node: Some("Hull"), mass: (100.0, Derived("nominal cuboid design: 100 kg/m^3 density x 1 m^3 volume = 100 kg")), inertia_extents: ((1.0, 1.0, 1.0), Derived("nominal uniform one-metre cube"))),
    gunner_yaw: None,
    track: (
        pin_plane_x: (left: -1.0, right: 1.0),
        substance: "RHA",
        link_count: 8,
        link_mass: (10.0, Derived("nominal link design: 20 kg/m linear mass x 0.5 m length = 10 kg")),
        hinge_torque: 1.0,
        link_angle: (inward_deg: 40.0, outward_deg: 10.0),
        sprocket: (spin_inertia_kgm2: ((left: 4.0, right: 4.0), Derived("nominal rotating design: two 8 kg point masses at 0.5 m from the axis; I = sum(m*r^2) = 4 kg m^2")), tooth_tip_rad: (left: 0.0, right: 0.0), teeth: 9, nodes: (left: "Sprocket_L", right: "Sprocket_R")),
        idler: (spin_inertia_kgm2: ((left: 4.0, right: 4.0), Derived("nominal rotating design: two 8 kg point masses at 0.5 m from the axis; I = sum(m*r^2) = 4 kg m^2")), radius: (left: 0.5, right: 0.5), nodes: (left: "Idler_L", right: "Idler_R")),
        powertrain: (
            transmission: (
                engine: (
                    idle_rpm: 600.0,
                    governed_rpm: 2000.0,
                    crank_limit_rpm: (2400.0, Unknown("provisional nominal crank ceiling; a design overspeed limit would settle it")),
                    rated_rpm: 2400.0,
                    cylinders: 1,
                    sound: None,
                    torque_curve: [(700.0, 1000.0), (2000.0, 1200.0), (2400.0, 1100.0)],
                    drag_fraction: 0.25,
                    // Synthetic crank admission uses the delivered torque slope, including fuel cut.
                    inertia_kgm2: 4.0,
                    clutch_capacity_nm: 1600.0,
                ),
                gearbox: (
                    forward_speeds_kmh: [5.0, 8.0],
                    reverse_speeds_kmh: [5.0],
                    launch_gears: 2,
                    max_step: 1,
                    shift_secs: 0.3,
                ),
                steering: (
                    radii: [(4.0, 12.0), (6.0, 18.0)],
                    reverse_radii: [(4.0, 12.0)],
                    capacity: 1.0,
                    recirculation: 0.9,
                ),
                brake_force: 1.0,
                // Static breakaway multiplier for the at-rest parking lever.
                brake_static_factor: 1.5,
            ),
        ),
        suspension: (
            ride_frequency: 1.2,
            damping_ratio: 0.4,
            bump_stop: 0.1,
            engage: 0.02,
        ),
    ),
    servos: {
        "Drive_Test": (
            role: Yaw,
            drive: Powered(
                power_share: (0.01, Unknown("provisional nominal pump share; pump design data would settle it")),
                stall_torque: (100.0, Unknown("provisional nominal motor N m; motor design data would settle it")),
                bearing_friction: (10.0, Unknown("provisional nominal bearing N m; bearing-drag measurements would settle it")),
                hand_rate: (1.0, Unknown("provisional nominal degrees/s; handwheel design data would settle it")),
            ),
            travel: Continuous,
            requires: [],
        ),
    },
    volumes: {
        "Wheel_L": (external: true),
        "Wheel_R": (external: true),
        "Driver": (crew: Driver),
        "Commander": (crew: Commander),
        "Engine": (hp: 80, function: Engine),
        "Transmission": (hp: 80, function: Transmission),
    },
    colliders: [(node: "Hull_Collider", substance: "RHA")],
    shadow_proxies: [],
    roadwheels: [
        (node: "Wheel_L", side: Left, spin_inertia_kgm2: (8.0, Derived("nominal rotating design: two 16 kg point masses at 0.5 m from the axis; I = sum(m*r^2) = 8 kg m^2")), tread: (radius: 0.5, spans: [(-0.5, 0.5)])),
        (node: "Wheel_R", side: Right, spin_inertia_kgm2: (8.0, Derived("nominal rotating design: two 16 kg point masses at 0.5 m from the axis; I = sum(m*r^2) = 8 kg m^2")), tread: (radius: 0.5, spans: [(-0.5, 0.5)])),
    ],
    weapons: {},
    views: {},
    capabilities: {Drive: [Engine, Transmission]},
    drive_controls: NotRequired,
)
"""


def purge():
    """Empty the session, libraries first — linked datablocks are freed by their library."""
    for library in list(bpy.data.libraries):
        bpy.data.libraries.remove(library)
    for collection in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.images):
        for datablock in list(collection):
            collection.remove(datablock)


def cube(name, faces=CUBE_FACES, uv=False):
    """One cube mesh datablock, optionally unwrapped a face at a time."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(list(CUBE_VERTICES), [], list(faces))
    mesh.update()
    if uv:
        layer = mesh.uv_layers.new(name="UVMap")
        for index in range(len(mesh.loops)):
            layer.uv[index].vector = FACE_UV[index % 4]
    return mesh


def place(scene, name, mesh, location, material):
    obj = bpy.data.objects.new(name, mesh)
    obj.location = location
    mesh.materials.append(material)
    scene.collection.objects.link(obj)
    return obj


def png(name, directory, colour):
    """One 8x8 PNG stored beside the blend, in the colour space its slot is read in."""
    image = bpy.data.images.new(name, 8, 8)
    image.filepath_raw = os.path.join(directory, name + ".png")
    image.file_format = "PNG"
    image.colorspace_settings.name = colour
    image.save()
    return image


def painted(name, directory):
    """A material sampling one image per ROLE — colour, direction and scalar data — because the
    derivation derives its encoder flags from the slot and the roles must not collide. The normal
    map is also what makes `D.TANGENTS` apply to this fixture's hull.
    """
    material = bpy.data.materials.new(name)
    tree = material.node_tree
    tree.nodes.clear()
    output = tree.nodes.new("ShaderNodeOutputMaterial")
    shader = tree.nodes.new("ShaderNodeBsdfPrincipled")
    tree.links.new(shader.outputs["BSDF"], output.inputs["Surface"])

    for slot, colour, socket in (
        ("testbed_base", "sRGB", "Base Color"),
        ("testbed_rough", "Non-Color", "Roughness"),
    ):
        texture = tree.nodes.new("ShaderNodeTexImage")
        texture.image = png(slot, directory, colour)
        tree.links.new(texture.outputs["Color"], shader.inputs[socket])

    texture = tree.nodes.new("ShaderNodeTexImage")
    texture.image = png("testbed_normal", directory, "Non-Color")
    tangent_space = tree.nodes.new("ShaderNodeNormalMap")
    tree.links.new(texture.outputs["Color"], tangent_space.inputs["Color"])
    tree.links.new(tangent_space.outputs["Normal"], shader.inputs["Normal"])
    return material


def write_library(path):
    """The canonical material library, BOTH halves: one datablock per substance at the path the
    source pass identifies a substance by, and the registry the door hands the consumer contract as
    `--registry` beside it.

    The registry is THIS repository's copy rather than numbers of the fixture's own: the substances
    the wheels wear are real registry keys, and a fixture with its own vocabulary would be testing
    one nothing else uses.
    """
    purge()
    donors = {bpy.data.materials.new(name) for name in SUBSTANCES}
    for material in donors:
        assert material.name in SUBSTANCES, "the donor was renamed to {}".format(material.name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.data.libraries.write(path, donors, fake_user=True)
    shutil.copyfile(
        os.path.join(_REPO, "assets", "materials", "materials.ron"),
        os.path.join(os.path.dirname(path), "materials.ron"),
    )
    return path


def link(path, name):
    with bpy.data.libraries.load(path, link=True) as (_source, target):
        target.materials = [name]
    material = target.materials[0]
    assert material is not None and material.library is not None, "{} came back local".format(name)
    return material


def build(directory, library, defect):
    """The tank blend: an art hull, a collision proxy, and a ballistic roadwheel per side."""
    purge()
    scene = bpy.context.window.scene
    hull = place(scene, "Hull", cube("Hull", uv=True), (0.0, 0.0, 1.0),
                 painted("Paint_Olive", directory))
    place(scene, "Hull_Collider", cube("Hull_Collider"), (0.0, 0.0, 1.0),
          bpy.data.materials.new("Mat_Collider"))
    # An explicit actuator frame exercises the same Powered schema as shipped construction.
    scene.collection.objects.link(bpy.data.objects.new("Drive_Test", None))
    # An open shell is what the consumer contract refuses on the RAW candidate: no source law
    # measures watertightness, so this defect passes L1 and stops the chain before the encode.
    wheel_faces = CUBE_FACES[:-1] if defect == "open-wheel" else CUBE_FACES
    for name, offset in (("Wheel_L", -1.0), ("Wheel_R", 1.0)):
        place(scene, name, cube(name, faces=wheel_faces), (0.0, offset, 0.0),
              link(library, "MildSteel"))
    for name, offset in (("Driver", 2.0), ("Commander", 4.0)):
        place(scene, name, cube(name), (offset, 0.0, 0.0), link(library, "Flesh"))
    for name, offset in (("Engine", -2.0), ("Transmission", -4.0)):
        place(scene, name, cube(name), (offset, 0.0, 0.0), link(library, "MildSteel"))
    if defect == "modifier":
        hull.modifiers.new(name="Bevel", type="BEVEL")

    path = os.path.join(directory, "testbed.blend")
    with open(os.path.join(directory, "testbed.tank.ron"), "w", encoding="utf-8") as handle:
        handle.write(SPEC)
    bpy.ops.wm.save_as_mainfile(filepath=path)
    return path


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="fixture_tank.py", allow_abbrev=False)
    parser.add_argument("--dir", required=True, help="where the assets/ tree is written")
    parser.add_argument("--defect", default="none", choices=("none", "modifier", "open-wheel"),
                        help="build the trio with exactly one thing wrong")
    arguments = parser.parse_args(argv)

    library = write_library(
        os.path.join(arguments.dir, "assets", "materials", "materials.blend")
    )
    directory = os.path.join(arguments.dir, "assets", "testbed")
    os.makedirs(directory, exist_ok=True)
    print("FIXTURE {}".format(build(directory, library, arguments.defect)), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
