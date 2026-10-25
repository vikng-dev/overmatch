# Pen cam: field footprints on the surfaces a field meets

**Status:** open · trial chosen by Yan 2026-10-25 · settles by Yan's eyes on the pen cam's glass
frames, the outside view and the victim's x-ray

Subject: Overmatch's damage model, a video game. A field (a spall cone's or a burst's dose
distribution) is game state, drawn by the pen cam as a translucent volume in its potency bands;
components and crew are HP pools.

**The question.** Should a field, besides its own volume, also tint the surfaces it meets (the
body's glass, the components and their state fills) in its potency band, and by which rule?

**Default — painting that follows the field law (this trial).** The fields are painted on the
components' surfaces alone (each component's mesh and its state fill), in the band of the fields'
summed intensity at the point, at opacity 0.6 over the surface's own colour, each band's edge a
pixel-wide smoothstep in `log2` of the sum, by the rule the component's role gives (ADR-0040):

- **Hard** (every role but crew): only where a field meets the surface: facing the field's origin,
  on its support, behind its front, and within each texel's reach (its reach tolerance and 1 cm).
- **Witness** (crew): wherever the surface lies inside a field, the volume's own inside test: on
  the support, behind the front, within each texel's reach and its tolerance; no facing test, no
  slack, so a crew station's far faces read the field it stands in.

The hull glass and every surface no component claims are never painted. The frames that paint:
the glass frames, the victim's x-ray and the outside view (its state fills); never the slice.

**Alternative kept — none** (`5afeae8`'s state). A field is drawn as its volume alone; a surface
it meets keeps its own look. The base and state surfaces read no field lane and no field map.

**Alternative kept — every surface by the hard rule** (as of `63ff121`). Every base and state
surface a field met, the body's glass included, took the band of the summed intensity under the
hard rule (the reach plus 1 cm, the front, the rim, the inner edge and the facing, each edge a
pixel wide), crew stations too.

**Why it's a playtest call.** The painting says which components a field reached and in which
band, which the volume shows only by depth; it also lays a teal tint over the state fill, the
outcome. Watch whether a player reads which components the field reached, whether the state fills
still read clearly under it, and whether the hull glass is missed.

**Revert cost.** Low — `git revert` of the commit `damage model: the pen cam paints a field on
the components by role, and a texel's reach holds a slope-scaled tolerance`, view layer only (the
look's shader, its uniform lanes, the stage and the x-ray), no sim, wire or record change. A
revert to none should keep that commit's reach tolerance (`REACH_TOLERANCE_DIAGONALS`, `map_at`,
`Map::lit`), which the volume reads too; the map's second support-margin diagonal serves the
painting alone. The all-surface alternative is `63ff121`'s painting with the hard rule on every
surface.

**Lives in.** Design doc §5, "Potency colour" (`.agents/docs/design/armor-penetration-and-damage.md`).
Code: `src/damage_cam/pen_cam/look.rs` (module doc, `Footprint`), `assets/shaders/pen_cam_look.wgsl`
(`footprint`).
