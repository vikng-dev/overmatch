//! The scorecard's seam onto one track model (the game's tank driving physics, game state only).
//!
//! Everything the card sets or reads that belongs to a track model, and nothing else, lives here.
//! An arm that replaces the track internals edits this file and leaves the card unchanged. The
//! card itself reads only the shipped input (`TankCommand`), Avian body state, Avian's solver
//! contacts and construction facts (`RigGeom`, the tank spec, the transmission tables).
//!
//! This arm: the boat (`lane/rubble`). Its belt support and grip act as forces applied outside
//! Avian's solver, reported per tick in `TrackContacts`.

use bevy::prelude::*;

use crate::ground_field::{ContactGround, GroundField};
use crate::track::sim::TrackContacts;
use crate::track::terrain::TrackField;

/// A force on the hull from one world body, at a world point.
#[derive(Clone, Copy)]
pub(super) struct WorldForce {
    /// The world body, or `None` for the terrain height field the track samples itself.
    pub(super) body: Option<Entity>,
    pub(super) point: Vec3,
    /// Newtons on the hull.
    pub(super) force: Vec3,
}

/// Whether this arm claims that a piece under load takes the equal and opposite reaction. The
/// boat does not: `track::rubble` clamps a resting piece's normal reaction by design.
pub(super) const CLAIMS_PIECE_REACTION: bool = false;

/// Make the whole height field one ground type.
pub(super) fn set_ground(app: &mut App, ground: ContactGround) {
    let mut track = app.world_mut().resource_mut::<TrackField>();
    let field = track
        .field
        .take()
        .expect("the booted world built its track field");
    track.field = Some(field.with_ground(Some(GroundField::uniform(ground))));
}

/// The forces this tick that the track model applied to the hull outside Avian's solver. The
/// card adds Avian's own solver contacts itself, so an arm whose contacts live in the solver
/// returns here only what the solver does not report: avian 0.7 keeps a contact's total normal
/// impulse but not its total friction impulse, so a solver arm adds its friction here.
pub(super) fn belt_forces(app: &App, tank: Entity) -> Vec<WorldForce> {
    let world = app.world();
    let contacts = world
        .get::<TrackContacts>(tank)
        .expect("a driving tank reports its belt contacts");
    contacts
        .0
        .iter()
        .flatten()
        .map(|c| WorldForce {
            body: c.collider.map(|collider| {
                world
                    .get::<avian3d::prelude::ColliderOf>(collider)
                    .map_or(collider, |of| of.body)
            }),
            point: c.point,
            force: c.normal * c.load + c.tangential_force(),
        })
        .collect()
}

/// The arm's own work per tank this tick, beyond the production `Work` counts and the solver
/// contacts the card counts itself.
pub(super) fn work(app: &App, tank: Entity) -> Vec<(&'static str, f64)> {
    let contacts = app
        .world()
        .get::<TrackContacts>(tank)
        .expect("a driving tank reports its belt contacts");
    let count = contacts.0.iter().map(Vec::len).sum::<usize>() as f64;
    // DERIVED from `track::forces::emit_contact_forces`: a support and a tangent application
    // per contact.
    vec![
        ("belt_contacts", count),
        ("belt_force_applications", 2.0 * count),
    ]
}
