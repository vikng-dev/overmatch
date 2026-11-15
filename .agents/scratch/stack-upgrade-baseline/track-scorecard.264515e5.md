# Track scorecard

## 0. Where the tank's colliders sit (observation)

Construction facts of the spawned tank, parked 2 s on flat ground (pitch 0.39°, the ground at hull-frame y 0.0101 m): every collider on the hull body, by role, in the hull frame (forward is −z), heights above the ground under the hull origin. Nose: the most forward vertex. Reach below h: the most forward point of the role at or below height h, the first thing of the tank a step of that height meets on a level approach (null: nothing of it is that low). Forward between, in and outboard of the lanes: the most forward point (z at height) of the role at heights up to 4 m within that lateral strip, the first thing of the tank a wall over that strip meets; the lanes are each shoe's span plus 0.05 m a side (Left x -1.957 .. -1.132 m, Right x 1.132 .. 1.957 m), as line 4b's one-lane walls. The belt outline is every route circle of the rest pose at the belt's outer face.

| collider | parts | nose_z_m | nose_height_m | lowest_height_m | lowest_at_z_m | forward_between_lanes | forward_in_lanes | forward_outboard | reach_below_0.2_m | reach_below_0.4_m | reach_below_0.6_m | reach_below_0.8_m | reach_below_1.0_m | reach_below_1.2_m |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| belt outline (rest pose) | 2 | -3.0395 | 0.7585 | 0.0032 | -1.8617 | null | -3.039 at 0.76 m | null | -2.4609 | -2.8817 | -3.0125 | -3.0395 | -3.0395 | -3.0395 |
| bump stop | 2 | -3.0383 | 0.7928 | 0.1624 | 2.3881 | null | -3.037 at 0.78 m | null | -1.5350 | -2.8798 | -3.0078 | -3.0383 | -3.0383 | -3.0383 |
| hull proxy Hull_Collider | 2 | -3.1469 | 0.7636 | 0.4798 | -2.4896 | -3.147 at 1.08 m | -2.271 at 1.32 m | null | null | null | -2.7681 | -3.1469 | -3.1469 | -3.1469 |
| hull proxy Turret_Collider | 1 | -1.2741 | 1.8901 | 1.8901 | -1.2741 | -1.274 at 2.35 m | null | null | null | null | null | null | null | null |
| side plate | 4 | -3.0383 | 0.7928 | 0.0032 | -1.8617 | null | -3.038 at 0.78 m | null | -2.4612 | -2.8806 | -3.0124 | -3.0383 | -3.0383 | -3.0383 |
| soft run | 2 | -3.0381 | 0.7221 | 0.0033 | 1.7553 | null | -3.038 at 0.72 m | null | -2.4608 | -2.8765 | -3.0114 | -3.0381 | -3.0381 | -3.0381 |

## 1. Holding on a slope (law check: PASS)

Parked (spawn state, park lever on), no input; the law in the force-balance form of `slope_hold`'s doc. mean_accel is the least-squares slope of the downhill speed over the window: the last 10 of 60 s below, seconds 4 to 8 above. Below: passes iff |mean_accel| is within the precision, 1 % of the hold's reserve. Above: passes iff mean_accel reaches the law's bound (the static excess less the ground's motion resistance at the tank's nominal pressure) less the precision, 1 % of the static excess, and the hull moves downhill. Spring-back is the downhill travel over the first 2 s; creep distance the travel after it; creep and peak speed are over the window. All four are observations.

| ground | side | slope_deg | grip_limit_deg | brake_limit_deg | binding | mean_accel_m_s2 | law_bound_m_s2 | precision_m_s2 | spring_back_m | creep_distance_m | window_travel_m | creep_speed_mm_s | peak_speed_mm_s | pass |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| concrete | below | 27.8674 | 30.9638 | 31.0007 | grip | 2.444e-4 | 0.0000 | 0.0047 | 0.8209 | 418.9839 | 0.0993 | 9.7327 | 15.2133 | true |
| concrete | above | 34.0601 | 30.9638 | 31.0007 | grip | 1.5448 | 0.2929 | 0.0062 | 3.1788 | 46.7574 | 37.2504 | 9347.2363 | 12424.8242 | true |
| dirt | below | 20.4164 | 22.6849 | 31.0007 | grip | 0.0000 | 0.0000 | 0.0042 | 0.0190 | 0.0000 | 0.0000 | -37.1378 | 37.1376 | true |
| dirt | above | 24.9534 | 22.6849 | 31.0007 | grip | 2.743e-5 | -0.1476 | 0.0042 | 0.3370 | 1.1147 | 0.7372 | 185.0697 | 185.2669 | true |
| gravel | below | 27.9007 | 35.4509 | 31.0007 | brake | 0.0000 | 0.0000 | 0.0046 | 0.0131 | 0.0000 | 0.0000 | -18.4831 | 18.4830 | true |
| gravel | above | 34.1008 | 35.4509 | 31.0007 | brake | 1.4671 | 0.0253 | 0.0045 | 2.2469 | 41.8469 | 33.6761 | 8450.3418 | 11373.1162 | true |
| mud | below | 13.9345 | 15.4827 | 31.0007 | grip | 0.0000 | 0.0000 | 0.0028 | 0.0226 | 0.0000 | 0.0000 | 93.5735 | 93.5726 | true |
| mud | above | 17.0310 | 15.4827 | 31.0007 | grip | 8.150e-5 | -1.3338 | 0.0028 | 0.1100 | 0.2587 | 0.1701 | 42.6876 | 43.1803 | true |
| sand | below | 25.6518 | 28.5020 | 31.0007 | grip | 0.0000 | 0.0000 | 0.0056 | 0.0370 | 0.0000 | 0.0000 | -78.2673 | 78.2679 | true |
| sand | above | 31.3522 | 28.5020 | 31.0007 | grip | 0.5172 | -0.5814 | 0.0056 | 0.9062 | 15.0404 | 12.0633 | 3027.1440 | 4055.3799 | true |
| sandy loam | below | 26.8702 | 29.8558 | 31.0007 | grip | 0.0000 | 0.0000 | 0.0059 | 0.0207 | 0.0000 | 0.0000 | -76.6414 | 76.6420 | true |
| sandy loam | above | 32.8414 | 29.8558 | 31.0007 | grip | 1.3667 | 0.2300 | 0.0059 | 1.8151 | 37.7849 | 30.5754 | 7672.2544 | 10394.6045 | true |

## 2. Turning (observation)

Base ground (sandy loam), full throttle run-up then a 12 s steer hold; means over the last 3 s. states_in_window lists (gear, reverse, detent). Moments are about the hull's up axis through its centre of mass, from every world force on the hull split into its longitudinal and lateral parts; in a steady turn they cancel, and the lateral part is the resisting moment Steeds' μWL/4 estimates (DERIVED: μ the ground's grip, W the hull weight, L the mean road-wheel span).

| run | steer | states_in_window | speed_m_s | yaw_rate_rad_s | yaw_rate_rms | radius_m | geared_radius_m | radius_over_geared | moment_longitudinal_n_m | moment_lateral_n_m | steeds_n_m | lateral_over_steeds |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| wide, short run-up | 0.4750 | {(6, false, 1)} | 4.9277 | -0.0488 | 3.512e-5 | 101.0037 | 76.0000 | 1.3290 | -2.506e5 | 2.517e5 | 2.893e5 | 0.8702 |
| tight, short run-up | 1.0000 | {(6, false, 2)} | 4.9063 | -0.1377 | 1.417e-5 | 35.6273 | 25.6800 | 1.3874 | -2.694e5 | 2.726e5 | 2.893e5 | 0.9423 |
| wide, long run-up | 0.4750 | {(8, false, 1)} | 9.9540 | -0.0468 | 7.705e-4 | 212.4781 | 165.0000 | 1.2877 | -1.974e5 | 2.016e5 | 2.893e5 | 0.6969 |
| tight, long run-up | 1.0000 | {(8, false, 2)} | 9.2563 | -0.1221 | 0.0015 | 75.8272 | 55.7800 | 1.3594 | -2.618e5 | 2.712e5 | 2.893e5 | 0.9376 |
| neutral pivot | 1.0000 | {(1, false, 2)} | 1.635e-5 | -0.1282 | 6.551e-5 | 1.275e-4 | 3.4400 | 3.706e-5 | -2.662e5 | 2.662e5 | 2.893e5 | 0.9203 |

## 3. Acceleration and top speed per ground (observation)

Full throttle from rest, 40 s on level ground; top speed is the last second's mean.

| ground | grip | rolling_resistance | v_2s | v_5s | v_10s | v_20s | v_40s | time_to_5_m_s | top_speed_m_s | top_speed_rms | final_gear |
|---|---|---|---|---|---|---|---|---|---|---|---|
| concrete | 0.6000 | 0.0400 | 2.6296 | 4.8898 | 7.4110 | 10.6151 | 10.6489 | 5.1719 | 10.6489 | 7.684e-6 | 8 |
| dirt | 0.4180 | 0.0623 | 2.4223 | 4.8280 | 6.8005 | 8.3002 | 10.5481 | 6.0781 | 10.5474 | 6.368e-4 | 8 |
| gravel | 0.7120 | 0.0512 | 2.5369 | 4.8376 | 7.0129 | 9.5030 | 10.5992 | 5.5781 | 10.5992 | 8.380e-6 | 8 |
| mud | 0.2770 | 0.1629 | 1.5673 | 3.0970 | 3.0761 | 3.7063 | 4.3614 | null | 4.3582 | 0.0019 | 6 |
| sand | 0.5430 | 0.1294 | 1.8399 | 3.2770 | 4.7775 | 4.8496 | 4.8496 | null | 4.8496 | 1.276e-5 | 6 |
| sandy loam | 0.5740 | 0.0433 | 2.6104 | 4.8094 | 7.2737 | 10.3344 | 10.6328 | 5.2812 | 10.6328 | 8.890e-6 | 8 |

## 4. Climbing (observation)

Full throttle from rest. Steps: face 6 m ahead of the belt's front point, climbed iff the hull centre passes 3 m beyond the face risen by 0.8 × the step within 15 s. Highest step climbed: 0.8 m. first_touch names what of the tank met the obstacle on the first tick any did: `belt` for the sampled support, then each solver collider's role.

| case | climbed | time_s | max_rise_m | max_pitch_deg | end_z_past_face_m | first_touch |
|---|---|---|---|---|---|---|
| step 0.2 m | true | 4.5625 | 0.2108 | 6.0228 | 3.0482 | belt |
| step 0.4 m | true | 5.0781 | 0.4481 | 10.3454 | 3.0453 | bump stop |
| step 0.6 m | true | 6.4688 | 0.6339 | 15.8108 | 3.0351 | bump stop |
| step 0.8 m | true | 10.3438 | 0.8109 | 21.0567 | 3.0219 | hull proxy Hull_Collider |
| step 1 m | false | null | 0.0039 | 3.3341 | -3.0978 | hull proxy Hull_Collider |
| step 1.2 m | false | null | 0.0046 | 2.8256 | -3.1252 | hull proxy Hull_Collider |

## 4b. Walls: full width, and one belt lane wide (observation)

A wall 4 m tall and 2 m thick, its face 2 m ahead of the belt's front point, pushed at full throttle from rest for 8 s. The one-lane walls span one shoe's width (its grip columns) plus 0.05 m each side, centred on that belt. The wall's force on the hull is split per solver contact into the part along the contact normal and the friction across it; vertical parts are upward positive, means over the last 2 s, peaks the largest upward value. The sampled support's force is unsplit. first_touch names what of the tank met the wall on the first tick any did; pitch is nose-up positive.

| case | first_touch | solver_colliders_over_push | belt_channel_touched | normal_force_last_2s_n | normal_vertical_last_2s_n | normal_vertical_peak_up_n | friction_vertical_last_2s_n | friction_vertical_peak_up_n | belt_channel_vertical_last_2s_n | max_rise_m | pitch_range_deg | end_z_past_face_m |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| full width, 10 m | hull proxy Hull_Collider | {"hull proxy Hull_Collider"} | false | 3.631e5 | 0.0097 | 4932.1011 | -73508.8906 | 0.0000 | 0.0000 | 0.0051 | -0.58 .. 2.83 | -3.1338 |
| Left belt lane only, x -1.957 .. -1.132 m | bump stop | {"bump stop", "hull proxy Hull_Collider"} | false | 1.943e5 | -2.186e-4 | 4023.9026 | 1.209e5 | 1.338e6 | 0.0000 | 0.1187 | -0.70 .. 6.66 | -2.4580 |
| Right belt lane only, x 1.132 .. 1.957 m | bump stop | {"bump stop", "hull proxy Hull_Collider"} | false | 1.942e5 | -1.564e-4 | 6642.9395 | 1.217e5 | 1.338e6 | 0.0000 | 0.1208 | -0.70 .. 6.71 | -2.4579 |

## 5. Trenches (observation)

Road-wheel span 3.605 m (construction). A 2 m deep cut between two static plateaus; full throttle from rest, the near edge 6 m ahead. Crossed iff, when the hull centre first passes 6 m beyond the far edge (within 15 s), it is within half the depth of its start level. Sink is the hull's deepest drop below its start.

| width_m | width_over_span | crossed | time_s | rise_6m_beyond_m | max_sink_m | pitch_range_deg | end_z_past_far_edge_m |
|---|---|---|---|---|---|---|---|
| 0.9013 | 0.2500 | true | 5.3281 | -0.0027 | 0.0581 | -1.1 .. 3.6 | 6.0043 |
| 1.8025 | 0.5000 | true | 5.5000 | -7.056e-4 | 0.1118 | -2.8 .. 4.3 | 6.0800 |
| 2.7038 | 0.7500 | true | 5.6875 | 0.0069 | 0.1810 | -3.0 .. 6.6 | 6.0377 |
| 4.5062 | 1.2500 | false | null | null | 0.8899 | -30.1 .. 2.8 | -3.3069 |

## 6. Curb under one edge, half off a ledge (observation)

Curb: means over seconds 9 to 14. Ledge: placed with the edge under the right shoe's centre line, means over seconds 4 to 6. Rise is the hull centre against its rest height over the supporting level.

| case | roll_deg | roll_rms_deg | hull_rise_m | lateral_drift_m | travel_m |
|---|---|---|---|---|---|
| curb 0.15 m under right outer edge, 0.3 throttle | 0.5403 | 0.2083 | 0.0119 | -0.0071 | 14.5903 |
| right track half off a 1 m ledge, parked | -3.4440 | 2.134e-6 | -0.0784 | -0.0035 | -7.169e-4 |

## 7. Ride, and energy decay after a kick (law check: PASS)

Washboard (observation): RMS over the ticks the belt spans the bars. Kick (law check, passivity): parked with no input the system takes no energy in, so the peak hull kinetic energy in the last second must fall below the first second's (each of the last two half-second peaks below its counterpart). The decay rate is an observation.

| case | samples | speed_m_s | pitch_rms_deg | roll_rms_deg | vertical_speed_rms_m_s | energy_by_half_second_j |
|---|---|---|---|---|---|---|
| washboard, full throttle | 340 | 7.6907 | 0.2758 | 4.955e-4 | 0.0233 | null |
| parked, pitch + heave kick | 256 | 0.0000 | null | null | null | kick 11903.7; peaks 11903.73, 2587.35, 2570.20, 2570.20, 2570.20, 2570.20, 2570.20, 2570.20 |

## 8. Rubble (law check: PASS)

Pieces of Brick (shipped density and friction), [0.8, 0.5, 0.6] m. Pile ahead: 5 × 6 under 4 × 5, near edge 6 m ahead of the belt's front point; drive until the hull centre is over the pile's centre (15 s at most), brake, park 4 s. Layer under: one row under each track's road wheels, the tank placed on it at rest, parked 4 s. Parked values are over those 4 s; rise is against the hull's rest level on the ground. Sink is the rest-pose belt bottom's depth inside pieces. The reaction ratio is (world support on the awake pieces − their weight) / (the pieces' support on the hull): 1 when every piece under load takes the equal and opposite reaction, 0 when the pieces hold the tank up unloaded. The tank is held awake while parked (Avian's `SleepingDisabled`), so its island's contacts are solved and read. The reader's control, the settling pile's world support over its awake weight: 1.0000 (RMS 0.0000), 1 when the reader is right.

| case | reached_pile_centre_s | hull_centre_past_near_edge_m | piece_speed_max_drive_m_s | hull_rise_m | vertical_jitter_rms_m | vertical_speed_rms_m_s | sink_mean_m | sink_max_m | piece_speed_max_parked_m_s | support_from_pieces_share_of_weight | reaction_ratio | reaction_ratio_rms | reaction_symmetric |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| pile ahead, 0.6 throttle | 11.0156 | 1.9510 | 2.9197 | 0.8386 | 0.0020 | 0.0103 | 0.1773 | 0.1779 | 0.0986 | 0.9957 | 1.0000 | 0.0023 | true |
| pile ahead, 1 throttle | 5.8594 | 1.9523 | 3.5944 | 0.8246 | 0.0064 | 0.0256 | 0.1828 | 0.1862 | 0.4299 | 1.0002 | 0.9999 | 0.0034 | true |
| a layer under the parked tank's tracks | null | null | null | 0.3310 | 0.0289 | 0.1792 | 0.1792 | 0.1976 | 0.1533 | 1.0000 | 1.0000 | 3.775e-4 | true |

## 8c. Loose heap (observation)

The track sandbox's rubble yard pile, built by its own builder from `assets/dev/track_sandbox.ron` (108 pieces of its source construction's chunks, its drop grid, taper and yaw step), centred 9.0 m ahead of the hull origin as the sandbox's `R` tour parks it, settled with the tank parked until every piece sleeps (30 s at most; asleep-after is null when the bound ran out). Then drive straight over the heap's centre until the belt's rear point clears the heap's far edge (15 s at most). Rise is against the hull's rest level on the ground; pitch is nose up and nose down. The heap's top and far edge are read in a strip one track gauge wide about the drive line, by rays cast down on a 0.05 m grid; past-far-edge is the hull centre's distance beyond it at the end, negative short of it. Piece motion is horizontal, from each piece's settled pose to the drive's end; moved is past 0.5 m, split by direction against the tank's starting heading (within 45° forward or back, else sideways). No reaction ratio: it reads static balance (the world's support on the pieces less their weight is the load), which holds only for pieces at rest; on a drive the heap's pieces gain and shed momentum, so a ratio off 1 breaches no law, and a sleeping piece holds no solver contact to read. Line 8's parked rows carry it.

| case | heap_asleep_after_s | pieces_awake_at_drive | hull_rise_max_m | hull_rise_over_heap_centre_m | pitch_max_nose_up_deg | pitch_max_nose_down_deg | reached_heap_centre_s | drive_s | hull_past_far_edge_m | heap_top_before_m | heap_top_after_m | pieces_moved | moved_mean_m | moved_max_m | moved_forward | moved_back | moved_sideways | pieces_removed |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0.6 throttle | 2.9531 | 0 | 0.0969 | 0.0967 | 3.8507 | 2.3684 | 5.2031 | 6.4375 | 2.8540 | 0.6915 | 0.5186 | 25 | 0.9505 | 1.7262 | 25 | 0 | 0 | 0 |
| 1 throttle | 2.9531 | 0 | 0.1045 | 0.1037 | 4.3034 | 2.2516 | 3.9844 | 4.9688 | 2.8376 | 0.6915 | 0.5209 | 30 | 1.0360 | 1.8869 | 30 | 0 | 0 | 0 |

## 9. Work per tank per tick (observation)

MEASURED counts per tick for the one tank in the world: the production `Work` counters, the belt's contact counts and Avian's solver contacts with the tank as a party. No timing.

| case | belt_near_colliders | belt_candidates | belt_slab_checks | belt_slab_aabb_passes | belt_overlaps | belt_dynamic_contacts | belt_dispatches | belt_boundary_builds | belt_partition_faces | belt_partition_cells | belt_contacts | belt_force_applications | solver_pairs | solver_manifolds | solver_points | contact_points |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| driving on open ground | 9.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 103.4437 | 206.8875 | 0.0000 | 0.0000 | 0.0000 | 0.0000 |
| parked on the pile ahead, 0.6 throttle | 8.9648 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 12.6953 | 12.6953 | 44.3906 | 0.0000 |
| parked on the pile ahead, 1 throttle | 9.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 8.1992 | 8.1992 | 30.3906 | 0.0000 |
| parked on a layer under its tracks | 8.9648 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 0.0000 | 20.8516 | 20.8516 | 78.4688 | 0.0000 |
