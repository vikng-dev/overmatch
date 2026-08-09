# Shadow presets — why the ladder starts where it does

The arithmetic and the shipped rungs live in `src/settings.rs` (`ShadowDistance`, `SHADOW_CASCADES`,
`SHADOW_FIRST_CASCADE_FAR_BOUND_M`) and are not repeated here. This note holds the two facts that
decided the ladder and have no home in the code.

- **The floor is physics.** DERIVED: at this world's 17° sun a shadow runs 3.27× its caster's
  height, so 100 m of terrain relief self-shadows out to ~327 m. 350 m is the first rung above that
  and the last one worth paying for — beyond it the envelope reaches into sky nothing casts into,
  buying reach nobody sees at the price of texel density everybody sees.
- **`Off` is a competitive-integrity question, not a graphics option.** Shadows are gameplay
  information: a hull-down tank's shadow, a barrel's shadow across a slope, the dark under a
  treeline are all read to find and range targets, so a client that switches them off sees what its
  opponent cannot. A survey of shipped competitive titles found **none** that ships shadows-off as
  an advantage lever. The rule is SUSPENDED rather than withdrawn (Yan, 2026-07-27) pending the
  15v15 frame-headroom measurement, which needs the off state to exist in order to be taken;
  removing `Off` again afterwards is deliberately open.
