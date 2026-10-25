# Pen cam: field footprints on the surfaces a field meets

**Status:** open · trial chosen by Yan 2026-10-25 · settles by Yan's eyes on the pen cam's glass
frames, the outside view and the victim's x-ray

Subject: Overmatch's damage model, a video game. A field (a spall cone's or a burst's dose
distribution) is game state, drawn by the pen cam as a translucent volume in its potency bands;
components and crew are HP pools.

**The question.** Should a field, besides its own volume, also tint the surfaces it meets (the
body's glass, the components and their state fills) in its potency band?

**Default — no footprints (this trial).** A field is drawn as its volume alone; a surface it
meets keeps its own look. The base and state surfaces read no field lane and no field map.

**Alternative kept — footprints in the band of the summed intensity** (as of `63ff121`). Every
base and state surface a field met took the band of the fields' summed intensity at its own point,
at opacity 0.6 over its own colour, each edge (the reach plus 1 cm, the front, the rim, the inner
edge, the facing) a pixel wide and each band's edge a pixel-wide smoothstep in `log2` of the sum.
The frames that drew it: the glass frames, the victim's x-ray and the outside view; never the
slice.

**Why it's a playtest call.** The footprint says which surfaces a field reached and in which band,
which the volume shows only by depth; it also covers the state fill, the outcome, under a teal
tint. Watch whether, without it, a player still reads which components the field reached, and
whether the state fills read more clearly.

**Revert cost.** Low — `git revert 5afeae8`: one commit, view layer only (the look's shader, its
uniform lanes, the stage and the x-ray), no sim, wire or record change. The revert also restores
the map's second support-margin diagonal, which only the footprint's edge read.

**Lives in.** Design doc §5, "Potency colour" (`.agents/docs/design/armor-penetration-and-damage.md`).
Code: `src/damage_cam/pen_cam/look.rs` (module doc), `assets/shaders/pen_cam_look.wgsl`.
