//! Buildings in the game's destruction model: hollow houses whose walls and roof are matter.
//!
//! A BUILDING IS DATA (ADR-0034). `assets/buildings/<id>/<id>.building.ron` ([`schema`]) is the
//! building as the asset tools' bake wrote it: its elements (piers, sills, lintels, gables, wall
//! plates, roof planes), the fracture chunks that tile them, the joints between chunks and the
//! chunks the ground holds. No game code builds or edits one; the bake is `tools/`, which the game
//! never depends on.

pub mod schema;
