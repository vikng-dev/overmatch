import bpy
SP="/private/tmp/claude-502/-Users-Yan-Desktop-github-vikng-dev-personal-overmatch/aa6ae501-da41-487d-a38f-23a2004cf55d/scratchpad"
bpy.ops.export_scene.gltf(filepath=SP+"/dryrun.glb", export_format='GLB')
print("EXPORTED")
