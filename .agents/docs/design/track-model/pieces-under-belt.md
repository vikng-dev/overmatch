# Pieces under the belt: acceptance evidence

Subject: the game's tank driving physics, game state only. Implements issue #238's bottom-support
lane; material-pair friction and suspension construction remain separate lanes.

The support-call method needs a geometric constraint: a convex shape's support vertex need not
lie inside the link's footprint. In particular, a long horizontal piece under an inclined link
can supply a remote vertex and saturate its depth even though the local top is low. Projected
intervals can also overlap at an empty corner. The implementation retains the support calls as
the cheap rejection, then clips the convex boundary to the footprint before reading its extrema.
The regression `support_depth_is_clipped_to_the_footprint_before_it_is_read` holds that local depth
against a change in the piece's remote extent.

The resultants retain the support-call model's projected footprint rectangles and maximum depth
per link. Detection uses the actual clipped shape; this is not triangle-by-triangle pressure
integration. Overlapping rectangles are partitioned so only the deepest surface carries each
cell, and split cells share one material element's strain update. Ground cells retain their
original quadrature column's lever arm. A piece wholly below the field changes no force bits.

MEASURED in the headless tests, unattended, on this lane:

| Gate | Evidence |
|---|---|
| Full-width loose-piece support | Hull rise **0.099954 m** over a fixture piece **0.1 m** high; piece displacement **0.000007 m**, final speed **0 m/s** |
| Piece between every probe line | **41999.992 N** against **420000 N** for full coverage: the DERIVED width share is **0.1** |
| Piece beside the footprint | **0 N** |
| Full game's driving adapter | Hull rise **0.077062 m** over fixture pieces **0.1 m** high; peak piece centre **0.05 m**, its starting height; peak piece speed **0.153281 m/s** |
| Stacking and partitioning | Support and element strain agree with one equivalent unpartitioned surface |
| Open ground and a buried piece | Force applications and telemetry match the field-only report bit for bit |

The driving fixture fits its pieces between the rigid side slices, with clearance derived from
the shoe thickness, so they cover only part of each shoe. It exercises the production tank and
drivetrain; the pad fixture separately checks full coverage. The driving fixture's peak piece
speed is one tick of gravity (DERIVED: `9.81 / 64 = 0.15328125 m/s`); the piece never rises above
its starting centre. These are position, force and velocity measurements, not frame/tick pricing.

Validation: formatting and CI clippy pass; targeted results are **178 track**, **6 sandbox**,
**33 contact**, **17 tank**, **3 citation** and **15 headless** tests passed (MEASURED test counts).
The first-part review separately ran **168 pen-cam**, **1 field-cast** and **3 citation** tests.
No collider geometry or asset changed in this implementation. Captures and pricing remain with
the orchestrator; the side slices' rigid bottom strips are unchanged.
