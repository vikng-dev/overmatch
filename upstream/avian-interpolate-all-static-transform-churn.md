# Draft: interpolate-all dirties static rigid-body hierarchies

**LOCAL DRAFT — not filed.** Integration/performance finding against `avian3d 0.7.0` and
`bevy_transform_interpolation 0.5.0` with Bevy 0.19.1. This is not a claim that the documented
meaning of `interpolate_all()` excludes static bodies: its current implementation includes them.

## Source and consequence

`avian3d::PhysicsInterpolationPlugin::interpolate_all()` registers translation and rotation
interpolation as requirements of `RigidBody`, irrespective of its variant. In
`bevy_transform_interpolation`, `complete_translation_easing` and `complete_rotation_easing`
assign the end pose in `FixedFirst`; `ease_translation_lerp` and `ease_rotation_slerp` assign an
eased pose in `RunFixedMainLoop`. These writes do not compare the result before mutating
`Transform`. `NoTransformEasing` requires the individual easing opt-outs, which all those
systems already exclude.

There are two costs: identical assignments trigger change detection, and arithmetic interpolation
of a fixed pose can introduce floating-point noise. Merely guarding the final assignment cannot
prevent noise that differs in bits. A static root can therefore propagate changes through a large
view hierarchy and trigger render extraction of hidden descendants.

## Reproduction

Overmatch's regression `trees::tests::a_static_map_body_is_not_eased` uses the actual tree spawn,
`MinimalPlugins`, `TransformPlugin` and the offline compositions' full `PhysicsPlugins`. A static
root has its real collider children and an attached view subtree. After warming the fixed clock on
10 ms frames, a `Last` system counts changed local and global transforms.

MEASURED: with `PhysicsInterpolationPlugin::interpolate_all()` in that physics, the assertion
reports `(local, global) = (1, 6)`, the root and every node beneath it, instead of `(0, 0)`. With
the plugin at its default it passes. The game-wide counts and the release timing comparison are in
`../.agents/scratch/perf-2026-10-29/bookkeeping-diagnosis.md`.

## Workaround and potential upstream change

Overmatch no longer uses `interpolate_all()`. Its offline compositions keep the interpolation
plugin at its default and opt each moving body in at the insertion that makes it dynamic, with
`TranslationInterpolation` and `RotationInterpolation`: the tanks and a launched turret. A body that
never moves takes no easing write. An opt-out (`NoTransformEasing` on every static body) also
measured clean, but it leaves the next static body type that forgets the marker paying the cost
again.

A generic upstream optimization could skip equal start/end poses and guard equal output writes.
Such a change needs tests for easing completion, manually written transforms, interpolation-state
reset, and changes of rigid-body variant. Automatically exempting static bodies is also a possible
API policy, but requires a deliberate decision about variant changes. Neither change is applied
here.
