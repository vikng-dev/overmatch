//! Compliant contact: an absolute spring-damper law per point along a manifold's normal,
//! `F = k·(overlap + rest) + c·closing` while `overlap + rest` is above zero, in place of Avian's
//! global contact softness. Absolute units let laws compose by summation whatever the bodies'
//! masses. A writer sets a manifold's law in [`Laws`] before Avian prepares the step's contact
//! constraints (`SolverSystems::PrepareContactConstraints`), and, if its pairs can sleep, after the
//! solver's start has woken islands (before `SolverSystems::PrepareSolverBodies`): a pair woken
//! there is solved that step with the manifold it slept with. The store is cleared at the start of
//! every physics step, so a law never outlives its step.
//!
//! [`apply_laws`] runs after Avian prepares the step's contact constraints and hands each point of
//! a manifold with a law to the fork's `ContactNormalPart::{compliant, spring}` hook (the fork's
//! `OVERMATCH_PATCH.md`, "Compliant contact"). The point's separation becomes the spring's (`rest`
//! subtracted), so every solve reads the spring's coordinate; its coefficients are the law's at its
//! own effective mass over the substep. A point the law does not bear at the narrow phase bears
//! nothing that step: it starts cold, each solve returns its accumulated impulse, and it stays in
//! the manifold, whose impulses are stored back point by point. A zero `rest` is the law without it
//! bit for bit.
//!
//! Who relies on it: the game's soft run, which hands each manifold of a track run its links' law.

use avian3d::collision::contact_types::ContactId;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::softness_parameters::SoftnessCoefficients;
use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSliceMut};

/// A point's normal law: stiffness and damping per point, and the spring's compression at zero
/// overlap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactCompliance {
    /// Normal stiffness per point (N/m).
    pub stiffness: f32,
    /// Normal damping per point (N·s/m).
    pub damping: f32,
    /// The spring's compression at zero overlap (m, signed): above zero it pushes across a gap of
    /// up to `rest`, below zero it is free through `-rest` of overlap. Zero is Avian's surface.
    pub rest: f32,
}

impl ContactCompliance {
    /// Whether a point `penetration` deep bears this law: its spring is compressed.
    pub fn bears(self, penetration: f32) -> bool {
        penetration + self.rest > 0.0
    }

    /// The soft-constraint coefficients of this law for a point of effective mass `mass` over a
    /// substep `h`: Avian's `SoftnessParameters::compute_coefficients` rewritten with `ω² = k/m`
    /// and `2ζω = c/m`. With `a = h(c + hk)`: bias `k/(c + hk)`, mass scale `a/(m + a)`, impulse
    /// scale `m/(m + a)`. A law of zero stiffness and damping bears nothing: [`INERT`], at any
    /// effective mass, where the formula at zero mass would carry the point's accumulated impulse
    /// through every solve.
    fn coefficients(self, mass: f32, h: f32) -> SoftnessCoefficients {
        let response = self.damping + h * self.stiffness;
        assert!(
            response >= 0.0,
            "contact law {self:?} answers a substep of {h} s with a negative or NaN response: \
             stiffness and damping are never negative"
        );
        if response == 0.0 {
            return INERT;
        }
        let a = h * response;
        let denominator = mass + a;
        SoftnessCoefficients {
            bias: self.stiffness / response,
            mass_scale: a / denominator,
            impulse_scale: mass / denominator,
        }
    }
}

/// The coefficients of a point its law does not bear: each solve returns the accumulated impulse,
/// which starts and stays zero.
const INERT: SoftnessCoefficients = SoftnessCoefficients {
    bias: 0.0,
    mass_scale: 0.0,
    impulse_scale: 1.0,
};

/// This step's laws, by contact pair and manifold index; a manifold without one is Avian's rigid
/// contact.
#[derive(Resource, Default, Debug)]
pub struct Laws(HashMap<(u32, usize), ContactCompliance>);

impl Laws {
    /// Gives manifold `manifold` of pair `contact` its law for this step, replacing any set before.
    pub fn set(&mut self, contact: ContactId, manifold: usize, law: ContactCompliance) {
        self.0.insert((contact.0, manifold), law);
    }

    /// The law manifold `manifold` of pair `contact` holds this step.
    pub fn get(&self, contact: ContactId, manifold: usize) -> Option<ContactCompliance> {
        self.0.get(&(contact.0, manifold)).copied()
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Laws>();
    app.get_schedule_mut(PhysicsSchedule)
        .expect("avian_ext::plugin is added after Avian's PhysicsPlugins")
        .add_systems((
            clear_laws.in_set(PhysicsStepSystems::First),
            apply_laws
                .after(SolverSystems::PrepareContactConstraints)
                .before(SolverSystems::PreSubstep),
        ));
}

fn clear_laws(mut laws: ResMut<Laws>) {
    laws.0.clear();
}

/// Hands each prepared constraint of a manifold with a law its spring (see the module doc). Colors
/// own disjoint constraints and are handed in parallel.
pub(crate) fn apply_laws(
    laws: Res<Laws>,
    mut constraints: ResMut<ConstraintGraph>,
    graph: Res<ContactGraph>,
    substep: Res<Time<Substeps>>,
) {
    if laws.0.is_empty() {
        return;
    }
    let h = substep.delta_secs();
    constraints
        .colors
        .par_splat_map_mut(ComputeTaskPool::get(), None, |_, colors| {
            for constraint in colors
                .iter_mut()
                .flat_map(|color| &mut color.contact_constraints)
            {
                // A manifold without a law stays rigid.
                let Some(law) = laws.get(constraint.contact_id, constraint.manifold_index) else {
                    continue;
                };
                let (_, pair) = graph
                    .get_by_id(constraint.contact_id)
                    .expect("a prepared constraint's pair is in the contact graph");
                let manifold = &pair.manifolds[constraint.manifold_index];
                assert_eq!(
                    constraint.points.len(),
                    manifold.points.len(),
                    "Avian prepares one constraint point per manifold point, in order"
                );
                for (point, contact) in constraint.points.iter_mut().zip(&manifold.points) {
                    point.initial_separation -= law.rest;
                    point.normal_part.compliant = true;
                    point.normal_part.spring = if law.bears(contact.penetration) {
                        law.coefficients(point.normal_part.effective_mass, h)
                    } else {
                        point.normal_part.impulse = 0.0;
                        if let Some(tangent) = &mut point.tangent_part {
                            tangent.impulse = default();
                        }
                        INERT
                    };
                }
            }
        });
}
