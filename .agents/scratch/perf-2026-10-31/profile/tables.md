### Top 20 by mean

| # | System (schedule, kind) | runs/tick | mean µs | p95 µs | share of busy | mean µs @ 2 tanks | per added tank µs (DERIVED) |
|---|---|---|---|---|---|---|---|
| 1 | `apply_track_forces` (FixedUpdate) | 1.00 | 2522 | 2698 | 26.7 % | 195 | +83.1 |
| 2 | `par: "(Entity, Ref<'_, Position>, Ref<'_, Rotation>, &mut ColliderAabb, &mut EnlargedAabb, Ref<'_, Collider...` (PhysicsSchedule, par) | 5.50 | 1753 | 2429 | 18.5 % | 1034 | +25.7 |
| 3 | `update_moved_collider_aabbs<Collider>` (PhysicsSchedule) | 1.01 | 638 | 874 | 6.7 % | 425 | +7.6 |
| 4 | `LightyearAvianPlugin::update_child_collider_position` (RunFixedMainLoop) | 2.55 | 276 | 536 | 2.9 % | 236 | +1.4 |
| 5 | `par: "(&Position, &ComputedCenterOfMass, &LinearVelocity, &AngularVelocity, &RigidBodyColliders, Has<SweptC...` (PhysicsSchedule, par) | 3.70 | 264 | 426 | 2.8 % | 10 | +9.0 |
| 6 | `par: "(Entity, Ref<'_, Transform>, &Children)" filter="(Without<ChildOf>, With<AncestorMarker<ColliderMarke...` (FixedPostUpdate, par) | 3.42 | 254 | 394 | 2.7 % | 132 | +4.4 |
| 7 | `update_solver_body_aabbs<Collider>` (PhysicsSchedule) | 1.00 | 228 | 317 | 2.4 % | 31 | +7.0 |
| 8 | `propagate_parent_transforms` (PostUpdate) | 2.55 | 219 | 332 | 2.3 % | 78 | +5.1 |
| 9 | `propagate_collider_transforms` (FixedPostUpdate) | 1.00 | 207 | 312 | 2.2 % | 95 | +4.0 |
| 10 | `integrate_projectiles` (FixedUpdate) | 1.00 | 179 | 34 | 1.9 % | 2 | +6.3 |
| 11 | `update_narrow_phase<Collider, ()>` (PhysicsSchedule) | 1.00 | 152 | 227 | 1.6 % | 45 | +3.8 |
| 12 | `mark_dirty_trees` (PostUpdate) | 2.55 | 148 | 214 | 1.6 % | 140 | +0.3 |
| 13 | `collect_collision_pairs<()>` (PhysicsSchedule) | 1.00 | 138 | 202 | 1.5 % | 15 | +4.4 |
| 14 | `apply_deposits` (FixedUpdate) | 1.00 | 111 | 5 | 1.2 % | 2 | +3.9 |
| 15 | `update_child_collider_position` (PhysicsSchedule) | 1.00 | 102 | 167 | 1.1 % | 64 | +1.3 |
| 16 | `par: "Entity" filter="Or<(Changed<Transform>, Changed<ChildOf>, Added<GlobalTransform>)>"` (PostUpdate, par) | 11.72 | 97 | 156 | 1.0 % | 71 | +0.9 |
| 17 | `drive_servos` (FixedUpdate) | 1.00 | 90 | 174 | 0.9 % | 9 | +2.9 |
| 18 | `fixed_nan_probe` (FixedPostUpdate) | 1.00 | 81 | 165 | 0.9 % | 55 | +0.9 |
| 19 | `update_solver_body_angular_inertia` (SubstepSchedule) | 6.00 | 69 | 108 | 0.7 % | 32 | +1.3 |
| 20 | `integrate_velocities` (SubstepSchedule) | 6.00 | 61 | 112 | 0.6 % | 34 | +1.0 |

### Top 20 by p95

| # | System (schedule, kind) | mean µs | p95 µs | max µs |
|---|---|---|---|---|
| 1 | `apply_track_forces` (FixedUpdate) | 2522 | 2698 | 5439 |
| 2 | `par: "(Entity, Ref<'_, Position>, Ref<'_, Rotation>, &mut ColliderAabb, &mut EnlargedAabb, Ref<'_, Collider...` (PhysicsSchedule, par) | 1753 | 2429 | 5208 |
| 3 | `update_moved_collider_aabbs<Collider>` (PhysicsSchedule) | 638 | 874 | 2128 |
| 4 | `LightyearAvianPlugin::update_child_collider_position` (RunFixedMainLoop) | 276 | 536 | 1258 |
| 5 | `par: "(&Position, &ComputedCenterOfMass, &LinearVelocity, &AngularVelocity, &RigidBodyColliders, Has<SweptC...` (PhysicsSchedule, par) | 264 | 426 | 674 |
| 6 | `par: "(Entity, Ref<'_, Transform>, &Children)" filter="(Without<ChildOf>, With<AncestorMarker<ColliderMarke...` (FixedPostUpdate, par) | 254 | 394 | 790 |
| 7 | `propagate_parent_transforms` (PostUpdate) | 219 | 332 | 447 |
| 8 | `update_solver_body_aabbs<Collider>` (PhysicsSchedule) | 228 | 317 | 545 |
| 9 | `propagate_collider_transforms` (FixedPostUpdate) | 207 | 312 | 719 |
| 10 | `update_narrow_phase<Collider, ()>` (PhysicsSchedule) | 152 | 227 | 584 |
| 11 | `mark_dirty_trees` (PostUpdate) | 148 | 214 | 295 |
| 12 | `collect_collision_pairs<()>` (PhysicsSchedule) | 138 | 202 | 581 |
| 13 | `drive_servos` (FixedUpdate) | 90 | 174 | 225 |
| 14 | `update_child_collider_position` (PhysicsSchedule) | 102 | 167 | 484 |
| 15 | `fixed_nan_probe` (FixedPostUpdate) | 81 | 165 | 306 |
| 16 | `par: "Entity" filter="Or<(Changed<Transform>, Changed<ChildOf>, Added<GlobalTransform>)>"` (PostUpdate, par) | 97 | 156 | 222 |
| 17 | `integrate_velocities` (SubstepSchedule) | 61 | 112 | 261 |
| 18 | `update_solver_body_angular_inertia` (SubstepSchedule) | 69 | 108 | 184 |
| 19 | `update_mass_properties` (FixedPostUpdate) | 23 | 97 | 306 |
| 20 | `integrate_positions` (SubstepSchedule) | 53 | 97 | 168 |

### Per added tank, top 15 (DERIVED: (mean@30 − mean@2) / 28)

| # | System (schedule) | per tank µs | mean @30 | mean @2 |
|---|---|---|---|---|
| 1 | `apply_track_forces` (FixedUpdate) | +83.1 | 2522 | 195 |
| 2 | `par: "(Entity, Ref<'_, Position>, Ref<'_, Rotation>, &mut ColliderAabb, &mut EnlargedAabb, Ref<'_, Collider...` (PhysicsSchedule, par) | +25.7 | 1753 | 1034 |
| 3 | `par: "(&Position, &ComputedCenterOfMass, &LinearVelocity, &AngularVelocity, &RigidBodyColliders, Has<SweptC...` (PhysicsSchedule, par) | +9.0 | 264 | 10 |
| 4 | `update_moved_collider_aabbs<Collider>` (PhysicsSchedule) | +7.6 | 638 | 425 |
| 5 | `update_solver_body_aabbs<Collider>` (PhysicsSchedule) | +7.0 | 228 | 31 |
| 6 | `integrate_projectiles` (FixedUpdate) | +6.3 | 179 | 2 |
| 7 | `propagate_parent_transforms` (PostUpdate) | +5.1 | 219 | 78 |
| 8 | `collect_collision_pairs<()>` (PhysicsSchedule) | +4.4 | 138 | 15 |
| 9 | `par: "(Entity, Ref<'_, Transform>, &Children)" filter="(Without<ChildOf>, With<AncestorMarker<ColliderMarke...` (FixedPostUpdate, par) | +4.4 | 254 | 132 |
| 10 | `propagate_collider_transforms` (FixedPostUpdate) | +4.0 | 207 | 95 |
| 11 | `apply_deposits` (FixedUpdate) | +3.9 | 111 | 2 |
| 12 | `update_narrow_phase<Collider, ()>` (PhysicsSchedule) | +3.8 | 152 | 45 |
| 13 | `drive_servos` (FixedUpdate) | +2.9 | 90 | 9 |
| 14 | `LightyearAvianPlugin::update_child_collider_position` (RunFixedMainLoop) | +1.4 | 276 | 236 |
| 15 | `update_solver_body_angular_inertia` (SubstepSchedule) | +1.3 | 69 | 32 |

### Busy time by schedule (leaf systems + their commands + parallel batches), µs per tick

| Schedule | 30 tanks | share | 2 tanks |
|---|---|---|---|
| PhysicsSchedule | 3384 | 35.8 % | 1692 |
| FixedUpdate | 3030 | 32.0 % | 262 |
| PostUpdate | 1135 | 12.0 % | 893 |
| FixedPostUpdate | 722 | 7.6 % | 358 |
| RunFixedMainLoop | 293 | 3.1 % | 252 |
| PreUpdate | 276 | 2.9 % | 286 |
| SubstepSchedule | 256 | 2.7 % | 138 |
| Update | 139 | 1.5 % | 91 |
| StateTransition | 94 | 1.0 % | 108 |
| Last | 52 | 0.6 % | 55 |
| SpawnScene | 31 | 0.3 % | 29 |
| First | 29 | 0.3 % | 30 |
| FixedLast | 8 | 0.1 % | 5 |
| FixedFirst | 5 | 0.1 % | 5 |
| **all** | **9458** | 100 % | **4209** |

### Schedule wall time per tick period (traced), µs

| Schedule | runs/tick @30 | mean @30 | p95 @30 | mean @2 |
|---|---|---|---|---|
| Main | 2.55 | 9768 | 11412 | 6001 |
| RunFixedMainLoop | 2.55 | 6129 | 6952 | 2194 |
| FixedMain | 1.00 | 5765 | 6405 | 1869 |
| FixedUpdate | 1.00 | 3211 | 3185 | 424 |
| FixedPostUpdate | 1.00 | 2453 | 3144 | 1351 |
| PhysicsSchedule | 1.00 | 1884 | 2436 | 1001 |
| PostUpdate | 2.55 | 1781 | 2408 | 1711 |
| PreUpdate | 2.55 | 721 | 980 | 838 |
| SubstepSchedule | 6.00 | 410 | 653 | 266 |
| StateTransition | 2.55 | 377 | 536 | 450 |
| Update | 2.55 | 278 | 332 | 262 |
| Last | 2.55 | 158 | 240 | 178 |
| First | 2.55 | 150 | 237 | 175 |
| SpawnScene | 2.55 | 124 | 189 | 140 |
| FixedLast | 1.00 | 33 | 51 | 29 |
| FixedFirst | 1.00 | 28 | 44 | 27 |
| FixedPreUpdate | 1.00 | 25 | 44 | 24 |

bots (drive_bot + spawn/schedule/respawn incl. commands): 54.3 µs/tick = 0.57 % of busy; drive_bot alone 1.2 µs
   system           respawn_dead_bots: mean 37.3 p95 4.5
   system_commands  respawn_dead_bots: mean 10.2 p95 1.5
   system           schedule_bot_respawn: mean 2.0 p95 3.8
   system           spawn_bots: mean 1.7 p95 3.2
   system           drive_bot: mean 1.2 p95 3.4
   system_commands  spawn_bots: mean 1.0 p95 1.7
   system_commands  schedule_bot_respawn: mean 0.8 p95 1.3
physics leaf (PhysicsSchedule + SubstepSchedule): 3640 µs/tick = 38.5 % of busy; @2: 1830
busy @30 9458 (p95 11310); @2 4209; periods 3839 / 3840; lines 34004874 / 37382928; mismatches 0 / 0
   multithreaded executor:  mean 500 runs/tick 449.24
   update:  mean 9780 runs/tick 2.55
