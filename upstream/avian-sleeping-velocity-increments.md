# Sleeping bodies' velocity increments are built every step but never cleared

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`pre_process_velocity_increments` (`src/dynamics/integrator/mod.rs`) and the force builders
`apply_constant_forces`, `apply_constant_linear_acceleration` and
`apply_constant_angular_acceleration` (`src/dynamics/rigid_body/forces/plugin.rs`) write
`VelocityIntegrationData` for every body that has it. `clear_velocity_increments` clears only
`With<SolverBodyIndex>`, and a sleeping body has no solver body. So every sleeping step computes
`x ← (x + a)·h` on the stored increment (`a` = gravity plus constant accelerations, `h` = substep
length): the increment settles at `a·h/(1 − h)` instead of `a·h`, and on waking every substep of the
first step applies the stale value. The builders also visit the whole sleeping population every
step for nothing.

## Minimal repro

1. A dynamic body in free fall, put to sleep (`SleepBody`) and held asleep for many steps.
2. Wake it (`WakeBody`) and run one step.
3. Expected: `LinearVelocity` gains one step's gravity. Actual: an extra `N·g·h²/(1 − h)` along
   gravity for `N` substeps (DERIVED from the recurrence above).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/dynamics/integrator/mod.rs
+++ b/src/dynamics/integrator/mod.rs
@@ -52,6 +52,7 @@
         app.register_required_components::<SolverBodyIndex, VelocityIntegrationData>();
 
         app.init_resource::<Gravity>();
+        app.init_resource::<IncrementsBuiltFor>();
 
         app.configure_sets(
             PhysicsSchedule,
@@ -257,22 +258,37 @@
     }
 }
 
+/// [`SolverBodies::membership_changes`] when this step's velocity increments were
+/// built. The build, the substeps' use and the clear must see one membership: a body entering in
+/// between would take its unscaled pending acceleration as a velocity every substep, and one
+/// leaving would keep a scaled increment that no clear reaches. Public only because
+/// [`pre_process_velocity_increments`] is.
+#[derive(Resource, Default)]
+pub struct IncrementsBuiltFor(u64);
+
 /// Applies gravity and locked axes to the linear and angular velocity increments of bodies.
 pub fn pre_process_velocity_increments(
-    mut bodies: Query<(
-        &RigidBody,
-        &mut VelocityIntegrationData,
-        Option<&LinearDamping>,
-        Option<&AngularDamping>,
-        Option<&GravityScale>,
-        Option<&LockedAxes>,
-    )>,
+    // build only increments that the solver will consume and clear.
+    mut bodies: Query<
+        (
+            &RigidBody,
+            &mut VelocityIntegrationData,
+            Option<&LinearDamping>,
+            Option<&AngularDamping>,
+            Option<&GravityScale>,
+            Option<&LockedAxes>,
+        ),
+        With<SolverBodyIndex>,
+    >,
     gravity: Res<Gravity>,
     time: Res<Time<Substeps>>,
+    solver_bodies: Res<SolverBodies>,
+    mut built_for: ResMut<IncrementsBuiltFor>,
     mut diagnostics: ResMut<SolverDiagnostics>,
 ) {
     let start = crate::utils::Instant::now();
 
+    built_for.0 = solver_bodies.membership_changes();
     let delta_secs = time.delta_secs();
 
     // TODO: Do we want to skip kinematic bodies here?
@@ -317,10 +333,18 @@
 /// Clears the velocity increments of bodies after the substepping loop.
 fn clear_velocity_increments(
     mut bodies: Query<&mut VelocityIntegrationData, With<SolverBodyIndex>>,
+    solver_bodies: Res<SolverBodies>,
+    built_for: Res<IncrementsBuiltFor>,
     mut diagnostics: ResMut<SolverDiagnostics>,
 ) {
     let start = crate::utils::Instant::now();
-
+    assert_eq!(
+        solver_bodies.membership_changes(),
+        built_for.0,
+        "no body enters or leaves the solver between the velocity increments' build and clear: \
+         Avian wakes and sleeps bodies outside SolverSystems::PreSubstep..=PostSubstep, and \
+         nothing inside may change a body's Sleeping, Disabled, RigidBodyDisabled or RigidBody"
+    );
     bodies.par_for_each_mut(MIN_PAR_ITER_ENTITIES, |mut integration| {
         integration.linear_increment = Vector::ZERO;
         integration.angular_increment = default();
--- a/src/dynamics/rigid_body/forces/plugin.rs
+++ b/src/dynamics/rigid_body/forces/plugin.rs
@@ -94,7 +94,11 @@
 
 /// Applies [`ConstantForce`] to the accumulated forces.
 fn apply_constant_forces(
-    mut bodies: Query<(&mut VelocityIntegrationData, &ComputedMass, &ConstantForce)>,
+    // sleepers retain pending increments until the solver takes them.
+    mut bodies: Query<
+        (&mut VelocityIntegrationData, &ComputedMass, &ConstantForce),
+        With<SolverBodyIndex>,
+    >,
 ) {
     bodies
         .iter_mut()
@@ -125,7 +129,11 @@
 
 /// Applies [`ConstantLinearAcceleration`] to the linear velocity increments.
 fn apply_constant_linear_acceleration(
-    mut bodies: Query<(&mut VelocityIntegrationData, &ConstantLinearAcceleration)>,
+    // same solver membership as increment preprocessing and clearing.
+    mut bodies: Query<
+        (&mut VelocityIntegrationData, &ConstantLinearAcceleration),
+        With<SolverBodyIndex>,
+    >,
 ) {
     bodies
         .iter_mut()
@@ -136,7 +144,11 @@
 
 /// Applies [`ConstantAngularAcceleration`] to the angular velocity increments.
 fn apply_constant_angular_acceleration(
-    mut bodies: Query<(&mut VelocityIntegrationData, &ConstantAngularAcceleration)>,
+    // same solver membership as increment preprocessing and clearing.
+    mut bodies: Query<
+        (&mut VelocityIntegrationData, &ConstantAngularAcceleration),
+        With<SolverBodyIndex>,
+    >,
 ) {
     bodies
         .iter_mut()
--- a/src/dynamics/solver/solver_body/mod.rs
+++ b/src/dynamics/solver/solver_body/mod.rs
@@ -574,6 +574,8 @@
     entities: Vec<Entity>,
     bodies: Vec<SolverBody>,
     inertias: Vec<SolverBodyInertia>,
+    // bumped by every push, removal and clear.
+    membership_changes: u64,
 }
 
 impl SolverBodies {
@@ -584,7 +586,15 @@
             entities: Vec::new(),
             bodies: Vec::new(),
             inertias: Vec::new(),
-        }
+            membership_changes: 0,
+        }
+    }
+
+    /// how many times the membership has changed. Two equal readings bound a
+    /// window in which no body entered or left.
+    #[inline]
+    pub(crate) fn membership_changes(&self) -> u64 {
+        self.membership_changes
     }
 
     /// Returns the number of solver bodies.
@@ -689,6 +699,7 @@
         self.entities.push(entity);
         self.bodies.push(body);
         self.inertias.push(inertia);
+        self.membership_changes += 1;
         index
     }
 
@@ -705,6 +716,7 @@
         self.entities.swap_remove(index.0 as usize);
         self.bodies.swap_remove(index.0 as usize);
         self.inertias.swap_remove(index.0 as usize);
+        self.membership_changes += 1;
         // TODO: Do this differently
         // If the removed body was not the last one, a body was swapped into its slot.
         self.entities.get(index.0 as usize).copied()
@@ -716,6 +728,7 @@
         self.entities.clear();
         self.bodies.clear();
         self.inertias.clear();
+        self.membership_changes += 1;
     }
 
     /// Returns a [`SolverBodiesAccess`] that allows obtaining mutable references to solver bodies
```

## Notes

Filtering the builders by the same membership the clear uses makes build, consume and clear agree.
That is exact while membership holds from the build to the clear in one step; a body that joins or
leaves the solver in between would take an unscaled or keep a scaled increment, as either side
could today.


## Membership invariant

The patch also guards its own ownership interval: every velocity increment built for a solver
member must be cleared for that same membership. The solver membership flag is captured when
increments are built and asserted when they clear. Waking a body between those boundaries must
fail immediately rather than consume increments built while it was outside the solver. This
tripwire stays with the increment fix; it does not validate application-owned component bundles.
