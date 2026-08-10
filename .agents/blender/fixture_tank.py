"""fixture_tank.py — the synthetic trio the door's end-to-end suite runs against.

    blender --background --factory-startup \\
      --python .agents/blender/fixture_tank.py -- --dir <workdir> [--defect <name>]

Writes a whole asset under `<workdir>`, in the layout the door derives every path from:

    assets/materials/materials.blend    the canonical substance library, linked from below
    assets/testbed/testbed.blend        the model: a hull, a collision proxy, two roadwheels
    assets/testbed/testbed.tank.ron     the spec sheet those nodes are declared in
    assets/testbed/testbed_*.png        one texture per colour role, so the chain has work to do

It is the smallest thing that is a TANK to every stage of the door: the source pass finds a clean
source, the consumer contract finds two watertight ballistic wheels and a usable collision proxy,
and the texture derivation finds a colour map, a normal map and a data map to bake — one per role,
which is what makes the derivation's role, transfer and tangent laws apply to it at all. Nothing
here is Tiger-shaped — the door is generic, so its fixture is a second vehicle.

`--defect` builds the same trio with exactly one thing wrong, which is how the suite proves that a
refusal at each stage leaves the tracked glb untouched.
"""

import argparse
import os
import sys

import bpy

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
SUBSTANCES = ("RHA", "MildSteel")

SPEC = """\
#![enable(implicit_some)]
// The synthetic tank the asset door's end-to-end suite is run against. Every number is a nominal
// value `TankSpec::validate` accepts, because none of them is what is under test: the door is,
// and the door needs a sheet that parses, validates, and names real nodes.
TankSpec(
    mass: 1000.0,
    inertia_extents: (2.0, 1.0, 3.0),
    track: (
        link_count: 8,
        link_mass: 10.0,
        hinge_torque: 1.0,
        link_angle: (inward_deg: 20.0, outward_deg: 10.0),
        sprocket: (teeth: 9),
        powertrain: (
            max_speed: 10.0,
            power: 100000.0,
            force: 50000.0,
            governor_gain: 5000.0,
            inertia: 100.0,
            transmission: (architecture: Governor),
        ),
        suspension: (
            ride_frequency: 1.2,
            damping_ratio: 0.4,
            bump_stop: 0.1,
            engage: 0.02,
        ),
    ),
    servos: {},
    volumes: {},
    colliders: ["Hull_Collider"],
    roadwheels: [
        (node: "Wheel_L", side: Left),
        (node: "Wheel_R", side: Right),
    ],
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
    """The canonical material library: one datablock per substance, at the path the source pass
    identifies a substance by."""
    purge()
    donors = {bpy.data.materials.new(name) for name in SUBSTANCES}
    for material in donors:
        assert material.name in SUBSTANCES, "the donor was renamed to {}".format(material.name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.data.libraries.write(path, donors, fake_user=True)
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
    # An open shell is what the consumer contract refuses on the RAW candidate: no source law
    # measures watertightness, so this defect passes L1 and stops the chain before the encode.
    wheel_faces = CUBE_FACES[:-1] if defect == "open-wheel" else CUBE_FACES
    for name, offset in (("Wheel_L", -1.0), ("Wheel_R", 1.0)):
        place(scene, name, cube(name, faces=wheel_faces), (0.0, offset, 0.0),
              link(library, "MildSteel"))
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
