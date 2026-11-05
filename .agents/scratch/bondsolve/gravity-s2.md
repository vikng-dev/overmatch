# Gravity audit, session 2

Subject: the video game's environment destruction model, game state only.

## Baseline observation

MEASURED by the ignored `gravity_census` instrument after rebasing onto main, before the polygon correction. Unit scale, no external loads. The table is evidence, never an assertion oracle. The asset folders contain two shipped constructions: `house_1` and `fir_1`.

| Part pair / material | Attachment and mode | Count | Demand range | Capacity range |
|---|---|---:|---:|---:|
| gable (Brick) / roof board (Wood) | resting: LiftForce | 9 | 0.805601–54.8554 N | 0–0 N |
| gable (Brick) / roof board (Wood) | resting: Tip | 19 | 0.03531–19.4962 N m | 0.025124–11.428 N m |
| gable (Brick) / roof board (Wood) | resting: Slide | 27 | 0.211778–113.408 N | 0.092293–77.2016 N |
| roof board / roof board (Wood / Wood) | nailed: Tension | 22 | 3252.77–13185.9 Pa | 3240–3240 Pa |
| roof board / roof board (Wood / Wood) | nailed: Shear | 3 | 3377.5–4046.64 Pa | 3240–3240 Pa |

MEASURED baseline first stages: the house cracks 80, 63, 89, 36, 4, 2, 10 and 4 joints in ticks 0–7; standing-owner departures are 0, 28, 29, 31, 0, 0, 0 and 10 chunks. It reaches equilibrium at tick 32. Detached groups and subsequent physical landings are outside this law-only probe. The tree cracks nothing, releases nothing, and takes zero sparse rounds on its first tick.

## Individual overloaded joints

MEASURED demands and capacities below. Tension/shear use Pa, lift/slide use N, tip uses N m; coordinates and area are m and m². Addresses are local to this exact authored graph.

| Joint | A | B | Attachment | Mode | Demand | Capacity | Area | Position (x, y, z) |
|---:|---|---|---|---|---:|---:|---:|---|
| 2023 | north/gable:Brick | west/board_9:Wood | resting | LiftForce | 54.855381 | 0.000000 | 0.020833 | (-1.566667, 2.900000, 2.875000) |
| 2024 | north/gable:Brick | west/board_10:Wood | resting | Tip | 6.857203 | 3.226593 | 0.046875 | (-1.675000, 2.818750, 2.875000) |
| 2056 | north/gable:Brick | east/board_9:Wood | resting | Slide | 33.638977 | 9.238644 | 0.020833 | (1.566667, 2.900000, 2.875000) |
| 2057 | north/gable:Brick | east/board_10:Wood | resting | Slide | 56.212952 | 32.765373 | 0.046875 | (1.675000, 2.818750, 2.875000) |
| 2060 | north/gable:Brick | west/board_7:Wood | resting | Tip | 1.340213 | 1.266100 | 0.014583 | (-1.256667, 3.132500, 2.875000) |
| 2061 | north/gable:Brick | west/board_8:Wood | resting | Tip | 12.945775 | 11.427988 | 0.050000 | (-1.360000, 3.055000, 2.875000) |
| 2087 | north/gable:Brick | east/board_7:Wood | resting | Tip | 0.200982 | 0.142736 | 0.014583 | (1.256667, 3.132500, 2.875000) |
| 2089 | north/gable:Brick | east/board_9:Wood | resting | Slide | 40.346333 | 26.625666 | 0.029167 | (1.486667, 2.960000, 2.875000) |
| 2093 | north/gable:Brick | west/board_6:Wood | resting | Tip | 11.480474 | 6.901628 | 0.050000 | (-1.040000, 3.295000, 2.875000) |
| 2094 | north/gable:Brick | west/board_7:Wood | resting | Tip | 7.827625 | 4.486992 | 0.035417 | (-1.176667, 3.192500, 2.875000) |
| 2112 | north/gable:Brick | east/board_5:Wood | resting | Slide | 19.401924 | 7.967798 | 0.008333 | (0.946667, 3.365000, 2.875000) |
| 2113 | north/gable:Brick | east/board_6:Wood | resting | Slide | 86.645256 | 49.477997 | 0.050000 | (1.040000, 3.295000, 2.875000) |
| 2114 | north/gable:Brick | east/board_7:Wood | resting | Slide | 32.676888 | 12.404829 | 0.035417 | (1.176667, 3.192500, 2.875000) |
| 2117 | north/gable:Brick | west/board_3:Wood | resting | Tip | 0.035310 | 0.025124 | 0.002083 | (-0.636667, 3.597500, 2.875000) |
| 2118 | north/gable:Brick | west/board_4:Wood | resting | Tip | 14.322619 | 5.175585 | 0.050000 | (-0.720000, 3.535000, 2.875000) |
| 2129 | north/gable:Brick | east/board_3:Wood | resting | LiftForce | 0.805601 | 0.000000 | 0.002083 | (0.636667, 3.597500, 2.875000) |
| 2131 | north/gable:Brick | east/board_5:Wood | resting | Slide | 80.232613 | 39.283298 | 0.041667 | (0.866667, 3.425000, 2.875000) |
| 2135 | north/gable:Brick | west/board_3:Wood | resting | LiftForce | 46.049160 | 0.000000 | 0.047917 | (-0.556667, 3.657500, 2.875000) |
| 2139 | north/gable:Brick | east/board_2:Wood | resting | Slide | 44.128292 | 19.243223 | 0.045833 | (0.406667, 3.770000, 2.875000) |
| 2140 | north/gable:Brick | east/board_3:Wood | resting | Slide | 37.084625 | 16.312769 | 0.047917 | (0.556667, 3.657500, 2.875000) |
| 2141 | north/gable:Brick | east/board_0:Wood | resting | LiftForce | 1.443739 | 0.000000 | 0.004295 | (0.129792, 3.977656, 2.775279) |
| 2142 | north/gable:Brick | east/board_0:Wood | resting | Slide | 70.682915 | 38.598656 | 0.044404 | (0.077275, 4.017044, 2.887520) |
| 2145 | north/gable:Brick | east/board_2:Wood | resting | Slide | 2.812168 | 2.058100 | 0.004167 | (0.326667, 3.830000, 2.875000) |
| 2147 | north/gable:Brick | west/board_1:Wood | resting | Tip | 10.092798 | 5.710214 | 0.050000 | (-0.240000, 3.895000, 2.875000) |
| 2148 | north/gable:Brick | west/board_2:Wood | resting | LiftForce | 10.358729 | 0.000000 | 0.004167 | (-0.326667, 3.830000, 2.875000) |
| 2662 | south/gable:Brick | east/board_9:Wood | resting | LiftForce | 8.075438 | 0.000000 | 0.020833 | (1.566667, 2.900000, -2.875000) |
| 2663 | south/gable:Brick | east/board_10:Wood | resting | Slide | 55.969669 | 28.180437 | 0.046875 | (1.675000, 2.818750, -2.875000) |
| 2695 | south/gable:Brick | west/board_9:Wood | resting | Slide | 24.806387 | 23.581564 | 0.020833 | (-1.566667, 2.900000, -2.875000) |
| 2696 | south/gable:Brick | west/board_10:Wood | resting | LiftForce | 13.514573 | 0.000000 | 0.030331 | (-1.700776, 2.799418, -2.883633) |
| 2697 | south/gable:Brick | west/board_10:Wood | resting | Tip | 2.002010 | 1.618608 | 0.016544 | (-1.627744, 2.854192, -2.859172) |
| 2700 | south/gable:Brick | east/board_7:Wood | resting | Slide | 9.782886 | 4.574858 | 0.003333 | (1.262149, 3.128388, -2.966704) |
| 2701 | south/gable:Brick | east/board_7:Wood | resting | Slide | 26.337622 | 8.496353 | 0.011251 | (1.255043, 3.133718, -2.847836) |
| 2703 | south/gable:Brick | east/board_9:Wood | resting | Slide | 19.618879 | 8.587041 | 0.029167 | (1.486667, 2.960000, -2.875000) |
| 2728 | south/gable:Brick | west/board_7:Wood | resting | LiftForce | 13.921329 | 0.000000 | 0.014583 | (-1.256667, 3.132500, -2.875000) |
| 2729 | south/gable:Brick | west/board_8:Wood | resting | Tip | 19.055597 | 6.850499 | 0.050000 | (-1.360000, 3.055000, -2.875000) |
| 2730 | south/gable:Brick | west/board_9:Wood | resting | Tip | 4.088763 | 4.055513 | 0.029167 | (-1.486667, 2.960000, -2.875000) |
| 2733 | south/gable:Brick | east/board_5:Wood | resting | Slide | 22.325041 | 6.608334 | 0.008333 | (0.946667, 3.365000, -2.875000) |
| 2734 | south/gable:Brick | east/board_6:Wood | resting | Slide | 63.997772 | 10.502137 | 0.048167 | (1.039214, 3.295589, -2.879547) |
| 2735 | south/gable:Brick | east/board_6:Wood | resting | Slide | 4.370940 | 4.056961 | 0.001833 | (1.060646, 3.279515, -2.755497) |
| 2736 | south/gable:Brick | east/board_7:Wood | resting | Slide | 0.211778 | 0.092293 | 0.000103 | (1.230077, 3.152442, -2.994380) |
| 2737 | south/gable:Brick | east/board_7:Wood | resting | Slide | 64.305435 | 20.653439 | 0.035314 | (1.176511, 3.192617, -2.874652) |
| 2756 | south/gable:Brick | west/board_5:Wood | resting | Slide | 13.879538 | 11.284101 | 0.008333 | (-0.946667, 3.365000, -2.875000) |
| 2757 | south/gable:Brick | west/board_6:Wood | resting | Tip | 19.496244 | 4.236865 | 0.050000 | (-1.040000, 3.295000, -2.875000) |
| 2761 | south/gable:Brick | east/board_3:Wood | resting | Slide | 4.551390 | 3.973550 | 0.002083 | (0.636667, 3.597500, -2.875000) |
| 2763 | south/gable:Brick | east/board_5:Wood | resting | Slide | 101.419533 | 42.488422 | 0.041667 | (0.866667, 3.425000, -2.875000) |
| 2776 | south/gable:Brick | west/board_4:Wood | resting | Slide | 113.408493 | 77.201561 | 0.050000 | (-0.720000, 3.535000, -2.875000) |
| 2777 | south/gable:Brick | west/board_5:Wood | resting | Tip | 14.135464 | 4.613533 | 0.041667 | (-0.866667, 3.425000, -2.875000) |
| 2780 | south/gable:Brick | east/board_2:Wood | resting | Tip | 6.480957 | 6.469029 | 0.045833 | (0.406667, 3.770000, -2.875000) |
| 2781 | south/gable:Brick | east/board_3:Wood | resting | Slide | 78.038696 | 44.632324 | 0.047917 | (0.556667, 3.657500, -2.875000) |
| 2785 | south/gable:Brick | west/board_2:Wood | resting | Tip | 5.826746 | 2.829194 | 0.035767 | (-0.422761, 3.757929, -2.875992) |
| 2786 | south/gable:Brick | west/board_2:Wood | resting | Tip | 0.423306 | 0.262076 | 0.010066 | (-0.349477, 3.812892, -2.871474) |
| 2788 | south/gable:Brick | east/board_0:Wood | resting | Slide | 37.139389 | 34.132454 | 0.050000 | (0.080000, 4.015000, -2.875000) |
| 2789 | south/gable:Brick | east/board_1:Wood | resting | Tip | 13.717559 | 3.270352 | 0.050000 | (0.240000, 3.895000, -2.875000) |
| 2792 | south/gable:Brick | west/board_1:Wood | resting | Tip | 6.887103 | 1.557786 | 0.050000 | (-0.240000, 3.895000, -2.875000) |
| 2793 | south/gable:Brick | west/board_2:Wood | resting | LiftForce | 4.449155 | 0.000000 | 0.004167 | (-0.326667, 3.830000, -2.875000) |
| 3017 | east/board_7:Wood | east/board_8:Wood | nailed | Tension | 3485.929932 | 3240.000000 | 0.020543 | (1.295164, 3.135218, -3.094498) |
| 3065 | west/board_0:Wood | west/board_1:Wood | nailed | Tension | 10359.539062 | 3240.000000 | 0.110225 | (-0.175053, 3.975071, 0.809036) |
| 3070 | west/board_1:Wood | west/board_2:Wood | nailed | Tension | 6154.121582 | 3240.000000 | 0.051650 | (-0.335145, 3.855193, 1.410084) |
| 3072 | west/board_1:Wood | west/board_2:Wood | nailed | Shear | 3377.504639 | 3240.000000 | 0.062169 | (-0.334879, 3.854838, 0.271779) |
| 3084 | west/board_2:Wood | west/board_3:Wood | nailed | Shear | 4046.637695 | 3240.000000 | 0.067134 | (-0.494797, 3.734729, 1.050276) |
| 3085 | west/board_2:Wood | west/board_3:Wood | nailed | Tension | 4275.071777 | 3240.000000 | 0.078907 | (-0.495027, 3.735036, 2.510926) |
| 3091 | west/board_3:Wood | west/board_4:Wood | nailed | Tension | 7420.719238 | 3240.000000 | 0.063764 | (-0.655001, 3.615001, -1.723618) |
| 3093 | west/board_3:Wood | west/board_4:Wood | nailed | Tension | 9882.285156 | 3240.000000 | 0.024503 | (-0.655038, 3.615050, -0.840949) |
| 3094 | west/board_3:Wood | west/board_4:Wood | nailed | Tension | 3623.100830 | 3240.000000 | 0.044227 | (-0.654992, 3.614990, 1.268653) |
| 3103 | west/board_4:Wood | west/board_5:Wood | nailed | Tension | 12650.349609 | 3240.000000 | 0.030978 | (-0.815143, 3.495191, -1.894512) |
| 3104 | west/board_4:Wood | west/board_5:Wood | nailed | Tension | 10139.880859 | 3240.000000 | 0.019783 | (-0.814526, 3.494367, -1.386826) |
| 3105 | west/board_4:Wood | west/board_5:Wood | nailed | Tension | 13185.880859 | 3240.000000 | 0.033331 | (-0.815177, 3.495236, -0.855947) |
| 3119 | west/board_6:Wood | west/board_7:Wood | nailed | Tension | 4489.658203 | 3240.000000 | 0.048967 | (-1.135134, 3.255179, 0.644025) |
| 3121 | west/board_6:Wood | west/board_7:Wood | nailed | Tension | 3843.846924 | 3240.000000 | 0.059385 | (-1.135030, 3.255039, 1.727648) |
| 3123 | west/board_6:Wood | west/board_7:Wood | nailed | Tension | 3547.132080 | 3240.000000 | 0.039161 | (-1.134860, 3.254814, -0.237047) |
| 3129 | west/board_7:Wood | west/board_8:Wood | nailed | Tension | 7164.458984 | 3240.000000 | 0.081380 | (-1.295132, 3.135176, -1.435381) |
| 3134 | west/board_8:Wood | west/board_9:Wood | nailed | Tension | 9028.945312 | 3240.000000 | 0.027938 | (-1.454694, 3.014593, -2.269850) |
| 3138 | west/board_8:Wood | west/board_9:Wood | nailed | Tension | 3252.771240 | 3240.000000 | 0.034856 | (-1.455057, 3.015076, 0.032557) |
| 3140 | west/board_8:Wood | west/board_9:Wood | nailed | Tension | 3543.512695 | 3240.000000 | 0.052335 | (-1.454928, 3.014904, 2.776611) |
| 3142 | west/board_9:Wood | west/board_10:Wood | nailed | Shear | 3988.796631 | 3240.000000 | 0.004932 | (-1.615137, 2.895183, -3.250664) |
| 3143 | west/board_9:Wood | west/board_10:Wood | nailed | Tension | 4143.148926 | 3240.000000 | 0.028748 | (-1.614887, 2.894849, -2.913849) |
| 3148 | west/board_9:Wood | west/board_10:Wood | nailed | Tension | 6939.813477 | 3240.000000 | 0.047137 | (-1.615030, 2.895040, -2.155043) |
| 3149 | west/board_9:Wood | west/board_10:Wood | nailed | Tension | 3449.003174 | 3240.000000 | 0.057385 | (-1.614973, 2.894963, 1.004529) |
| 3156 | west/board_10:Wood | west/board_11:Wood | nailed | Tension | 5464.052246 | 3240.000000 | 0.051802 | (-1.775020, 2.775027, -1.551471) |
| 3160 | west/board_10:Wood | west/board_11:Wood | nailed | Tension | 7404.722168 | 3240.000000 | 0.111837 | (-1.775003, 2.775004, 0.329635) |

## Missing truths and decisions

- **Lift at resting gable faces:** a rest cannot supply tensile traction. The linear trial currently welds every rest before judging its reaction. A tensile trial reaction is evidence that the assumed active contact set is wrong; it does not prove the roof is unsound. A compression-only contact solve must redistribute the load before making that claim.
- **Tip at those faces:** the available lever is a property of the actual contact polygon. An equal-area circle invents both the lever and the rotational stiffness. Integrate the authored overlap and test its perimeter; do not change friction or strength.
- **Slide at those faces:** tangential tractions and their friction budget depend on the contact-pressure distribution. The trial currently sticks the whole patch, then removes it if its demand exceeds the budget. Frictional slip and partial opening remain unresolved contact states; changing a material coefficient would conceal that missing law.
- **Nailed-edge tension and shear:** long thin plank-edge contacts need their directional area moments. The circular substitute changes the load sharing as well as peak traction. Fixing compliance is also missing: bulk wood stiffness is not a measured nailing stiffness. Withdrawal strength remains 3,240 Pa, and the explicitly Unknown lateral analogue remains 3,240 Pa. Neither is raised.
- **Timber direction:** the material row explicitly states a transverse modulus and transverse tension strength; the structural solve treats them as scalar facts. The authored grain frame currently serves the view. A longitudinal timber response requires directional constitutive data and a law that reads it. This audit cannot certify physical unsoundness from a scalar trial alone.

The polygon correction changes geometry, not authored material numbers, and does not seed cracks or pre-settle a construction. Root-plate equivalent circular footprints keep their explicit whole-footprint contract.

## Polygon observation

MEASURED unit-scale house after integrating the actual face moments: 97 initial trial overloads
(6 lift, 17 tip, 32 slide at gable/board rests; 23 tension and 19 shear at nailed board edges).
The first five ticks crack 97, 97, 85, 8 and 1 joints and release 0, 52, 20, 22 and 4 chunks.
No further crack occurs; the standing owner equilibrates at tick 28. The tree again has no
initial overload and performs zero rounds on its first gravity-only tick. These are observations
of the current linear-contact approximation; the geometry fix does not resolve contact activity.

DERIVED isolated-board witness from the authored geometry, not a whole-roof certification:
ordinary boards are 0.2 m wide and 0.05 m thick, 6.6 m long, density 448 kg/m³. Gable reaction
centres at z = ±2.875 m give span 5.75 m with 0.425 m overhang at each end. Uniform gravity
load is `q = density * width * thickness * g`; maximum simply supported moment with equal
loaded overhangs is `q * (span² - 4 * overhang²) / 8`. Resolving gravity into the roof's 3:4
slope gives maximum combined corner tension about 2.03 MPa, below the row's scalar 2.4 MPa.
The board alone therefore supplies no proof of an unsound span. Its downslope force exceeds
unassisted friction because `tan(theta) = 0.75 > sqrt(0.5 * 0.65) = 0.5701`; edge contacts,
fixings and the plate must carry that remainder. Solving their unilateral contact and actual
fixing compliance is needed before declaring the authored roof physically unsound.

## Placed-scale census and gates

MEASURED using the admitted map library, all 709 construction placements, and up to 128 law-only
ticks per owner. All 635 tree instances (scales 0.50205–1.499289) keep every joint and chunk;
their initial equilibrium is idle. Every house instance starts cracking: the 74 house scales
0.549203–1.463569 have 68–122 initial trial overloads, with no first-tick departure. Ticks 1–3
crack 83–103, 43–85 and 3–48 joints and release 12–63, 15–58 and 0–26 chunks respectively.
Every standing owner reaches equilibrium within the observation's 128 ticks. This does not
include re-contact or landings after departure and makes no physical-soundness certification.

The polygon step passed 228 targeted construction/wall/caller/doc-citation tests; four ignored
instruments remain on demand. Both gravity census instruments ran separately. Formatting and
workspace/all-target CI clippy passed. No timing run, capture, visible window, material-strength
change or initial-topology exemption was made. Engine calls use the existing Bevy 0.19.1 and
Avian 0.7 pinned APIs; new tensor arithmetic was checked against glam 0.32.1 source.
