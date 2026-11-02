# Contact classes: what the contact walk does on real code

Subject: the game's damage model, how a round moves across armour; game state only. Instrument: the `#[ignore]`d test `contact_classes_on_the_shipped_hull` in `src/ballistics/contact_classes.rs`, real shots through the real march and resolve. The shipped hull is its fixture, not its spec; nothing here passes or fails on a tank's numbers. Numbers are MEASURED unless labelled. Two seeded runs gave identical tables apart from T9's microseconds.

**Bottom line.** The handoff fix solved rests at first contact, tick phase for nearly every aim, unrelated motion and moving targets. It did not solve the same rest one exchange later (25.2 % of edge shots), the marks (174 mm at 75°, 516 mm at 85°, exactly as the design says), or long chains at one contact. §8's condition for shrinking to lanes 4 and 7 is not met, and those are not the lanes the numbers ask for; the verdict is after the tables. Two march panics met here (24 shots) come first.

### Summary per class

All shipped arms, muzzle and a quarter of muzzle speed. First exchange shares are of `shots`.

| class | shots | skid | through | buried | Held | zero-turn at first exchange | zero-turn anywhere in the shot | mean speed lost on one, m/s | strike off axis, median / 95th, radii | contacts of 5+ exchanges | exchanges at zero clock | aim step 95th / worst, ° | aims whose outcome flips with phase |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 568 | 80.1 % | 7.6 % | 11.3 % | 0.0 % | 1.1 % | 7.6 % | 214 | 2.05 / 14.38 | 17 of 987 | 103 of 1279 | 0.00 / 23.49 | 2 of 226 |
| thin plate | 740 | 68.0 % | 21.9 % | 8.1 % | 0.0 % | 2.0 % | 11.8 % | 250 | 1.73 / 12.12 | 33 of 1412 | 206 of 1921 | 0.00 / 30.00 | 6 of 268 |
| edge | 960 | 52.0 % | 33.3 % | 13.0 % | 0.4 % | 1.0 % | 25.2 % | 243 | 1.23 / 7.61 | 69 of 1487 | 406 of 3750 | 2.15 / 67.60 | 11 of 334 |
| strip | 906 | 55.0 % | 27.8 % | 16.0 % | 0.1 % | 1.1 % | 17.0 % | 237 | 1.45 / 7.75 | 29 of 1464 | 353 of 2252 | 4.13 / 51.26 | 3 of 248 |
| inside face | 960 | 72.3 % | 13.6 % | 11.8 % | 0.3 % | 2.0 % | 9.8 % | 240 | 1.95 / 12.12 | 38 of 2003 | 307 of 2839 | 0.00 / 7.79 | not flown |
| narrow gap | 1050 | 73.1 % | 17.5 % | 7.3 % | 0.0 % | 2.0 % | 12.0 % | 207 | 2.05 / 12.12 | 47 of 1942 | 331 of 4918 | 0.00 / 46.01 | 8 of 322 |
| raised detail | 822 | 52.7 % | 22.7 % | 21.7 % | 1.1 % | 1.8 % | 24.0 % | 213 | 1.40 / 7.18 | 88 of 1354 | 506 of 2399 | 3.05 / 30.01 | 8 of 258 |
| seam | 248 | 62.5 % | 20.6 % | 14.1 % | 0.0 % | 2.8 % | 8.1 % | 255 | 1.69 / 12.12 | 8 of 417 | 49 of 599 | 0.00 / 10.30 | 2 of 76 |
| wedge | 849 | 57.2 % | 23.4 % | 17.0 % | 1.4 % | 0.9 % | 16.8 % | 202 | 1.40 / 8.26 | 61 of 1474 | 412 of 2455 | 4.92 / 55.82 | 6 of 268 |
| curved facets | 686 | 63.8 % | 18.7 % | 16.3 % | 0.0 % | 1.2 % | 14.4 % | 212 | 1.61 / 11.30 | 47 of 1146 | 309 of 1960 | 2.66 / 11.23 | 10 of 254 |
| moving target | 1158 | 51.0 % | 34.6 % | 14.3 % | 0.0 % | 0.0 % | 4.9 % | 395 | 1.77 / 14.38 | 42 of 2285 | 289 of 3622 | not flown | not flown |

Every zero-turn at a first exchange belongs to the HE arm (T1c); every kinetic arm reads 0.0 % there. DERIVED from `decide_playback`, not traced: an instant fuze takes a first touch that would bite through `clear_path`, which is not noted, so the HE arm's first noted exchange is often the piece flying on from the burst, born on the surface. 7.9 mm rounds, which fly one line and no disc, read 0.0 % zero-turn anywhere in the shot in every class.

<!-- contact_classes tables begin -->
All tables MEASURED on the shipped hull (the fixture), edit-profile test binary, seed `0x5eedc0de0260`, up to 24 aims per class, obliquity and calibre. Calibres fired: 88.0 mm, 35.7 mm, 7.9 mm. Rounds: kwk36_l56 88mm_AP, kwk36_l56 88mm_APHE_ring, kwk36_l56 88mm_HE, kwk36_l56 88mm_APCR, mg34 7.9mm_AP. Speeds: muzzle and 0.25 of it (fixture). Phase: uniform per aim.

### T0. What the armour offers: aims kept, and the class mix of uniform aims

Uniform-aim share is a rate over the hull's surface area; other columns are quotas, not rates.

| class | 88.0 mm kept | 88.0 mm uniform mix | 35.7 mm kept | 35.7 mm uniform mix | 7.9 mm kept | 7.9 mm uniform mix |
|---|---|---|---|---|---|---|
| full plate | 34 | 4.3 % | 86 | 27.4 % | 96 | 80.8 % |
| thin plate | 80 | 25.5 % | 96 | 22.9 % | 34 | 0.6 % |
| edge | 96 | 16.5 % | 96 | 11.5 % | 96 | 7.4 % |
| strip | 93 | 11.1 % | 96 | 5.0 % | 78 | 1.3 % |
| inside face | 96 | 0.0 % | 96 | 0.0 % | 96 | 0.0 % |
| narrow gap | 96 | 19.8 % | 96 | 16.5 % | 96 | 6.2 % |
| raised detail | 94 | 10.3 % | 96 | 3.3 % | 33 | 0.4 % |
| seam | 19 | 2.3 % | 53 | 3.5 % | 15 | 0.7 % |
| wedge | 79 | 5.3 % | 92 | 4.3 % | 96 | 1.7 % |
| curved facets | 69 | 4.9 % | 77 | 5.7 % | 59 | 1.0 % |
| moving target | 0 | 0.0 % | 0 | 0.0 % | 0 | 0.0 % |

### T1. Outcome of the first exchange, per class

| | n | no contact | skid | Held | zero-turn rest | buried | through | zero-turn anywhere in the shot |
|---|---|---|---|---|---|---|---|---|
| full plate | 568 | 0.0 % | 80.1 % | 0.0 % | 1.1 % | 11.3 % | 7.6 % | 7.6 % |
| thin plate | 740 | 0.0 % | 68.0 % | 0.0 % | 2.0 % | 8.1 % | 21.9 % | 11.8 % |
| edge | 960 | 0.2 % | 52.0 % | 0.4 % | 1.0 % | 13.0 % | 33.3 % | 25.2 % |
| strip | 906 | 0.0 % | 55.0 % | 0.1 % | 1.1 % | 16.0 % | 27.8 % | 17.0 % |
| inside face | 960 | 0.0 % | 72.3 % | 0.3 % | 2.0 % | 11.8 % | 13.6 % | 9.8 % |
| narrow gap | 1050 | 0.0 % | 73.1 % | 0.0 % | 2.0 % | 7.3 % | 17.5 % | 12.0 % |
| raised detail | 822 | 0.0 % | 52.7 % | 1.1 % | 1.8 % | 21.7 % | 22.7 % | 24.0 % |
| seam | 248 | 0.0 % | 62.5 % | 0.0 % | 2.8 % | 14.1 % | 20.6 % | 8.1 % |
| wedge | 849 | 0.0 % | 57.2 % | 1.4 % | 0.9 % | 17.0 % | 23.4 % | 16.8 % |
| curved facets | 686 | 0.0 % | 63.8 % | 0.0 % | 1.2 % | 16.3 % | 18.7 % | 14.4 % |
| moving target | 1158 | 0.0 % | 51.0 % | 0.0 % | 0.0 % | 14.3 % | 34.6 % | 4.9 % |

### T1a. Zero-turn rests anywhere in the shot, by reason

A zero-turn rest is a skid the march ended where it read it: the round stops and is charged its whole speed. `held material`: a sample's start was inside armour. `no approach`: nothing held and no distance to turn in. Position: first contact, after a skid, after a bite.

| class | shots | with one | share | held material | no approach | other | first contact | after skid | after bite | on the surface just skidded | median mm since the last exchange | mean speed lost (m/s) | coverage below 1 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 568 | 43 | 7.6 % | 38 | 5 | 0 | 6 | 37 | 0 | 31 | 0.0 | 214 | 33 |
| thin plate | 740 | 87 | 11.8 % | 72 | 15 | 0 | 15 | 72 | 0 | 52 | 0.0 | 250 | 64 |
| edge | 960 | 242 | 25.2 % | 235 | 7 | 0 | 10 | 232 | 0 | 231 | 0.0 | 243 | 222 |
| strip | 906 | 154 | 17.0 % | 146 | 8 | 0 | 10 | 144 | 0 | 143 | 0.0 | 237 | 147 |
| inside face | 960 | 94 | 9.8 % | 85 | 9 | 0 | 19 | 72 | 3 | 46 | 0.0 | 240 | 41 |
| narrow gap | 1050 | 126 | 12.0 % | 115 | 11 | 0 | 21 | 105 | 0 | 70 | 0.0 | 207 | 88 |
| raised detail | 822 | 197 | 24.0 % | 189 | 8 | 0 | 15 | 182 | 0 | 179 | 0.0 | 213 | 179 |
| seam | 248 | 20 | 8.1 % | 12 | 8 | 0 | 7 | 13 | 0 | 13 | 0.0 | 255 | 11 |
| wedge | 849 | 143 | 16.8 % | 141 | 2 | 0 | 8 | 135 | 0 | 133 | 0.0 | 202 | 132 |
| curved facets | 686 | 99 | 14.4 % | 92 | 7 | 0 | 8 | 91 | 0 | 86 | 0.0 | 212 | 84 |
| moving target | 1158 | 57 | 4.9 % | 57 | 0 | 0 | 0 | 39 | 18 | 38 | 3.7 | 395 | 40 |

### T1b. First exchange per class and obliquity

| | n | no contact | skid | Held | zero-turn rest | buried | through | zero-turn anywhere in the shot |
|---|---|---|---|---|---|---|---|---|
| full plate 45° | 192 | 0.0 % | 58.9 % | 0.0 % | 1.0 % | 25.5 % | 14.6 % | 10.4 % |
| full plate 60° | 186 | 0.0 % | 84.4 % | 0.0 % | 2.2 % | 7.5 % | 5.9 % | 8.6 % |
| full plate 75° | 114 | 0.0 % | 95.6 % | 0.0 % | 0.0 % | 0.9 % | 3.5 % | 5.3 % |
| full plate 85° | 76 | 0.0 % | 100.0 % | 0.0 % | 0.0 % | 0.0 % | 0.0 % | 1.3 % |
| thin plate 45° | 196 | 0.0 % | 46.9 % | 0.0 % | 2.6 % | 10.2 % | 40.3 % | 14.8 % |
| thin plate 60° | 210 | 0.0 % | 53.3 % | 0.0 % | 1.4 % | 11.0 % | 34.3 % | 11.0 % |
| thin plate 75° | 224 | 0.0 % | 87.1 % | 0.0 % | 1.8 % | 7.6 % | 3.6 % | 10.3 % |
| thin plate 85° | 110 | 0.0 % | 94.5 % | 0.0 % | 2.7 % | 0.0 % | 2.7 % | 10.9 % |
| edge 45° | 240 | 0.8 % | 35.0 % | 1.7 % | 0.8 % | 22.5 % | 39.2 % | 18.3 % |
| edge 60° | 240 | 0.0 % | 48.3 % | 0.0 % | 0.4 % | 10.4 % | 40.8 % | 21.2 % |
| edge 75° | 240 | 0.0 % | 56.2 % | 0.0 % | 1.2 % | 14.2 % | 28.3 % | 31.2 % |
| edge 85° | 240 | 0.0 % | 68.3 % | 0.0 % | 1.7 % | 5.0 % | 25.0 % | 30.0 % |
| strip 45° | 216 | 0.0 % | 41.2 % | 0.0 % | 1.4 % | 25.0 % | 32.4 % | 15.3 % |
| strip 60° | 240 | 0.0 % | 48.3 % | 0.4 % | 0.8 % | 14.6 % | 35.8 % | 16.7 % |
| strip 75° | 228 | 0.0 % | 56.6 % | 0.0 % | 0.4 % | 20.2 % | 22.8 % | 18.9 % |
| strip 85° | 222 | 0.0 % | 73.9 % | 0.0 % | 1.8 % | 4.5 % | 19.8 % | 17.1 % |
| inside face 45° | 240 | 0.0 % | 56.7 % | 1.2 % | 0.4 % | 17.1 % | 24.6 % | 11.2 % |
| inside face 60° | 240 | 0.0 % | 58.8 % | 0.0 % | 0.0 % | 17.1 % | 24.2 % | 10.4 % |
| inside face 75° | 240 | 0.0 % | 82.9 % | 0.0 % | 2.5 % | 10.0 % | 4.6 % | 9.6 % |
| inside face 85° | 240 | 0.0 % | 90.8 % | 0.0 % | 5.0 % | 2.9 % | 1.2 % | 7.9 % |
| narrow gap 45° | 291 | 0.0 % | 47.8 % | 0.0 % | 1.0 % | 15.8 % | 35.4 % | 14.8 % |
| narrow gap 60° | 279 | 0.0 % | 64.9 % | 0.0 % | 1.4 % | 5.7 % | 28.0 % | 11.1 % |
| narrow gap 75° | 240 | 0.0 % | 92.1 % | 0.0 % | 0.8 % | 6.2 % | 0.8 % | 2.9 % |
| narrow gap 85° | 240 | 0.0 % | 94.6 % | 0.0 % | 5.0 % | 0.0 % | 0.4 % | 18.8 % |
| raised detail 45° | 198 | 0.0 % | 32.8 % | 2.0 % | 1.0 % | 28.3 % | 35.9 % | 19.7 % |
| raised detail 60° | 198 | 0.0 % | 46.0 % | 0.0 % | 1.5 % | 23.2 % | 29.3 % | 28.8 % |
| raised detail 75° | 202 | 0.0 % | 59.4 % | 1.5 % | 2.0 % | 22.8 % | 14.4 % | 23.8 % |
| raised detail 85° | 224 | 0.0 % | 70.1 % | 0.9 % | 2.7 % | 13.4 % | 12.9 % | 23.7 % |
| seam 45° | 78 | 0.0 % | 43.6 % | 0.0 % | 2.6 % | 19.2 % | 34.6 % | 12.8 % |
| seam 60° | 86 | 0.0 % | 53.5 % | 0.0 % | 1.2 % | 19.8 % | 25.6 % | 2.3 % |
| seam 75° | 48 | 0.0 % | 87.5 % | 0.0 % | 2.1 % | 6.2 % | 4.2 % | 10.4 % |
| seam 85° | 36 | 0.0 % | 91.7 % | 0.0 % | 8.3 % | 0.0 % | 0.0 % | 8.3 % |
| wedge 45° | 240 | 0.0 % | 44.6 % | 3.3 % | 0.0 % | 18.3 % | 33.8 % | 18.3 % |
| wedge 60° | 239 | 0.0 % | 48.5 % | 1.3 % | 0.4 % | 17.6 % | 32.2 % | 20.5 % |
| wedge 75° | 240 | 0.0 % | 62.1 % | 0.4 % | 0.8 % | 21.2 % | 15.4 % | 12.1 % |
| wedge 85° | 130 | 0.0 % | 87.7 % | 0.0 % | 3.8 % | 5.4 % | 3.1 % | 16.2 % |
| curved facets 45° | 202 | 0.0 % | 42.6 % | 0.0 % | 0.5 % | 29.7 % | 27.2 % | 14.9 % |
| curved facets 60° | 240 | 0.0 % | 65.4 % | 0.0 % | 0.8 % | 12.5 % | 21.2 % | 17.1 % |
| curved facets 75° | 184 | 0.0 % | 76.1 % | 0.0 % | 1.1 % | 10.9 % | 12.0 % | 10.9 % |
| curved facets 85° | 60 | 0.0 % | 91.7 % | 0.0 % | 5.0 % | 3.3 % | 0.0 % | 13.3 % |
| moving target 45° | 288 | 0.0 % | 24.3 % | 0.0 % | 0.0 % | 22.9 % | 52.8 % | 4.5 % |
| moving target 60° | 296 | 0.0 % | 37.5 % | 0.0 % | 0.0 % | 12.2 % | 50.3 % | 4.1 % |
| moving target 75° | 290 | 0.0 % | 62.8 % | 0.0 % | 0.0 % | 16.6 % | 20.7 % | 2.8 % |
| moving target 85° | 284 | 0.0 % | 80.3 % | 0.0 % | 0.0 % | 5.6 % | 14.1 % | 8.5 % |

### T1c. First exchange per class and round

| | n | no contact | skid | Held | zero-turn rest | buried | through | zero-turn anywhere in the shot |
|---|---|---|---|---|---|---|---|---|
| full plate kwk36_l56 88mm_AP | 68 | 0.0 % | 73.5 % | 0.0 % | 0.0 % | 23.5 % | 2.9 % | 11.8 % |
| full plate kwk36_l56 88mm_APHE_ring | 68 | 0.0 % | 73.5 % | 0.0 % | 0.0 % | 23.5 % | 2.9 % | 11.8 % |
| full plate kwk36_l56 88mm_HE | 68 | 0.0 % | 61.8 % | 0.0 % | 8.8 % | 29.4 % | 0.0 % | 16.2 % |
| full plate kwk36_l56 88mm_APCR | 172 | 0.0 % | 75.6 % | 0.0 % | 0.0 % | 5.8 % | 18.6 % | 9.3 % |
| full plate mg34 7.9mm_AP | 192 | 0.0 % | 95.3 % | 0.0 % | 0.0 % | 1.0 % | 3.6 % | 0.0 % |
| thin plate kwk36_l56 88mm_AP | 160 | 0.0 % | 69.4 % | 0.0 % | 0.0 % | 7.5 % | 23.1 % | 10.0 % |
| thin plate kwk36_l56 88mm_APHE_ring | 160 | 0.0 % | 69.4 % | 0.0 % | 0.0 % | 7.5 % | 23.1 % | 10.0 % |
| thin plate kwk36_l56 88mm_HE | 160 | 0.0 % | 58.8 % | 0.0 % | 9.4 % | 20.6 % | 11.2 % | 21.2 % |
| thin plate kwk36_l56 88mm_APCR | 192 | 0.0 % | 75.0 % | 0.0 % | 0.0 % | 0.0 % | 25.0 % | 10.9 % |
| thin plate mg34 7.9mm_AP | 68 | 0.0 % | 63.2 % | 0.0 % | 0.0 % | 4.4 % | 32.4 % | 0.0 % |
| edge kwk36_l56 88mm_AP | 192 | 0.0 % | 53.1 % | 0.5 % | 0.0 % | 13.5 % | 32.8 % | 34.4 % |
| edge kwk36_l56 88mm_APHE_ring | 192 | 0.0 % | 53.1 % | 0.5 % | 0.0 % | 13.5 % | 32.8 % | 33.9 % |
| edge kwk36_l56 88mm_HE | 192 | 0.0 % | 44.3 % | 0.5 % | 5.2 % | 21.9 % | 28.1 % | 34.4 % |
| edge kwk36_l56 88mm_APCR | 192 | 0.0 % | 40.6 % | 0.5 % | 0.0 % | 11.5 % | 47.4 % | 23.4 % |
| edge mg34 7.9mm_AP | 192 | 1.0 % | 68.8 % | 0.0 % | 0.0 % | 4.7 % | 25.5 % | 0.0 % |
| strip kwk36_l56 88mm_AP | 186 | 0.0 % | 52.2 % | 0.0 % | 0.0 % | 15.1 % | 32.8 % | 23.7 % |
| strip kwk36_l56 88mm_APHE_ring | 186 | 0.0 % | 52.2 % | 0.0 % | 0.0 % | 15.1 % | 32.8 % | 23.7 % |
| strip kwk36_l56 88mm_HE | 186 | 0.0 % | 39.8 % | 0.0 % | 5.4 % | 29.0 % | 25.8 % | 19.9 % |
| strip kwk36_l56 88mm_APCR | 192 | 0.0 % | 45.8 % | 0.5 % | 0.0 % | 14.1 % | 39.6 % | 15.1 % |
| strip mg34 7.9mm_AP | 156 | 0.0 % | 91.0 % | 0.0 % | 0.0 % | 5.1 % | 3.8 % | 0.0 % |
| inside face kwk36_l56 88mm_AP | 192 | 0.0 % | 70.3 % | 0.5 % | 0.0 % | 13.0 % | 16.1 % | 6.8 % |
| inside face kwk36_l56 88mm_APHE_ring | 192 | 0.0 % | 70.3 % | 0.5 % | 0.0 % | 13.0 % | 16.1 % | 6.8 % |
| inside face kwk36_l56 88mm_HE | 192 | 0.0 % | 57.3 % | 0.5 % | 9.9 % | 21.4 % | 10.9 % | 25.5 % |
| inside face kwk36_l56 88mm_APCR | 192 | 0.0 % | 67.7 % | 0.0 % | 0.0 % | 7.3 % | 25.0 % | 9.9 % |
| inside face mg34 7.9mm_AP | 192 | 0.0 % | 95.8 % | 0.0 % | 0.0 % | 4.2 % | 0.0 % | 0.0 % |
| narrow gap kwk36_l56 88mm_AP | 216 | 0.0 % | 71.3 % | 0.0 % | 0.0 % | 6.5 % | 22.2 % | 13.4 % |
| narrow gap kwk36_l56 88mm_APHE_ring | 216 | 0.0 % | 71.3 % | 0.0 % | 0.0 % | 6.5 % | 22.2 % | 14.4 % |
| narrow gap kwk36_l56 88mm_HE | 202 | 0.0 % | 62.4 % | 0.0 % | 10.4 % | 17.8 % | 9.4 % | 22.3 % |
| narrow gap kwk36_l56 88mm_APCR | 222 | 0.0 % | 66.2 % | 0.0 % | 0.0 % | 5.9 % | 27.9 % | 9.5 % |
| narrow gap mg34 7.9mm_AP | 194 | 0.0 % | 96.4 % | 0.0 % | 0.0 % | 0.0 % | 3.6 % | 0.0 % |
| raised detail kwk36_l56 88mm_AP | 188 | 0.0 % | 55.3 % | 1.1 % | 0.0 % | 18.6 % | 25.0 % | 27.1 % |
| raised detail kwk36_l56 88mm_APHE_ring | 188 | 0.0 % | 55.3 % | 1.1 % | 0.0 % | 18.6 % | 25.0 % | 27.1 % |
| raised detail kwk36_l56 88mm_HE | 188 | 0.0 % | 43.6 % | 1.1 % | 8.0 % | 33.5 % | 13.8 % | 30.3 % |
| raised detail kwk36_l56 88mm_APCR | 192 | 0.0 % | 49.5 % | 1.0 % | 0.0 % | 19.8 % | 29.7 % | 19.8 % |
| raised detail mg34 7.9mm_AP | 66 | 0.0 % | 72.7 % | 1.5 % | 0.0 % | 10.6 % | 15.2 % | 0.0 % |
| seam kwk36_l56 88mm_AP | 37 | 0.0 % | 83.8 % | 0.0 % | 0.0 % | 5.4 % | 10.8 % | 8.1 % |
| seam kwk36_l56 88mm_APHE_ring | 37 | 0.0 % | 83.8 % | 0.0 % | 0.0 % | 5.4 % | 10.8 % | 8.1 % |
| seam kwk36_l56 88mm_HE | 38 | 0.0 % | 57.9 % | 0.0 % | 18.4 % | 18.4 % | 5.3 % | 26.3 % |
| seam kwk36_l56 88mm_APCR | 106 | 0.0 % | 42.5 % | 0.0 % | 0.0 % | 21.7 % | 35.8 % | 3.8 % |
| seam mg34 7.9mm_AP | 30 | 0.0 % | 86.7 % | 0.0 % | 0.0 % | 3.3 % | 10.0 % | 0.0 % |
| wedge kwk36_l56 88mm_AP | 158 | 0.0 % | 50.6 % | 1.3 % | 0.0 % | 16.5 % | 31.6 % | 24.1 % |
| wedge kwk36_l56 88mm_APHE_ring | 158 | 0.0 % | 50.6 % | 1.3 % | 0.0 % | 16.5 % | 31.6 % | 24.1 % |
| wedge kwk36_l56 88mm_HE | 158 | 0.0 % | 39.9 % | 1.3 % | 5.1 % | 32.9 % | 20.9 % | 23.4 % |
| wedge kwk36_l56 88mm_APCR | 184 | 0.0 % | 52.2 % | 2.7 % | 0.0 % | 15.2 % | 29.9 % | 16.3 % |
| wedge mg34 7.9mm_AP | 191 | 0.0 % | 87.4 % | 0.5 % | 0.0 % | 6.3 % | 5.8 % | 0.0 % |
| curved facets kwk36_l56 88mm_AP | 138 | 0.0 % | 64.5 % | 0.0 % | 0.0 % | 14.5 % | 21.0 % | 16.7 % |
| curved facets kwk36_l56 88mm_APHE_ring | 138 | 0.0 % | 64.5 % | 0.0 % | 0.0 % | 14.5 % | 21.0 % | 15.9 % |
| curved facets kwk36_l56 88mm_HE | 138 | 0.0 % | 56.5 % | 0.0 % | 5.8 % | 28.3 % | 9.4 % | 26.1 % |
| curved facets kwk36_l56 88mm_APCR | 154 | 0.0 % | 50.6 % | 0.0 % | 0.0 % | 18.8 % | 30.5 % | 11.7 % |
| curved facets mg34 7.9mm_AP | 118 | 0.0 % | 88.1 % | 0.0 % | 0.0 % | 3.4 % | 8.5 % | 0.0 % |
| moving target kwk36_l56 88mm_AP | 382 | 0.0 % | 45.5 % | 0.0 % | 0.0 % | 17.3 % | 37.2 % | 9.4 % |
| moving target kwk36_l56 88mm_APCR | 400 | 0.0 % | 36.2 % | 0.0 % | 0.0 % | 15.0 % | 48.8 % | 5.2 % |
| moving target mg34 7.9mm_AP | 376 | 0.0 % | 72.3 % | 0.0 % | 0.0 % | 10.6 % | 17.0 % | 0.0 % |

### T1d. First exchange per class and speed

| | n | no contact | skid | Held | zero-turn rest | buried | through | zero-turn anywhere in the shot |
|---|---|---|---|---|---|---|---|---|
| full plate muzzle | 284 | 0.0 % | 63.4 % | 0.0 % | 1.1 % | 21.5 % | 14.1 % | 3.2 % |
| full plate spent | 284 | 0.0 % | 96.8 % | 0.0 % | 1.1 % | 1.1 % | 1.1 % | 12.0 % |
| thin plate muzzle | 370 | 0.0 % | 40.8 % | 0.0 % | 1.4 % | 15.4 % | 42.4 % | 5.7 % |
| thin plate spent | 370 | 0.0 % | 95.1 % | 0.0 % | 2.7 % | 0.8 % | 1.4 % | 17.8 % |
| edge muzzle | 480 | 0.2 % | 30.0 % | 0.0 % | 0.4 % | 15.6 % | 53.8 % | 8.8 % |
| edge spent | 480 | 0.2 % | 74.0 % | 0.8 % | 1.7 % | 10.4 % | 12.9 % | 41.7 % |
| strip muzzle | 453 | 0.0 % | 32.9 % | 0.0 % | 0.7 % | 15.9 % | 50.6 % | 6.6 % |
| strip spent | 453 | 0.0 % | 77.0 % | 0.2 % | 1.5 % | 16.1 % | 5.1 % | 27.4 % |
| inside face muzzle | 480 | 0.0 % | 50.0 % | 0.0 % | 1.9 % | 22.1 % | 26.0 % | 4.2 % |
| inside face spent | 480 | 0.0 % | 94.6 % | 0.6 % | 2.1 % | 1.5 % | 1.2 % | 15.4 % |
| narrow gap muzzle | 480 | 0.0 % | 62.9 % | 0.0 % | 1.9 % | 15.8 % | 19.4 % | 5.6 % |
| narrow gap spent | 480 | 0.0 % | 97.1 % | 0.0 % | 2.5 % | 0.0 % | 0.4 % | 13.8 % |
| narrow gap just through | 90 | 0.0 % | 0.0 % | 0.0 % | 0.0 % | 1.1 % | 98.9 % | 36.7 % |
| raised detail muzzle | 411 | 0.0 % | 22.6 % | 0.0 % | 1.2 % | 34.1 % | 42.1 % | 8.3 % |
| raised detail spent | 411 | 0.0 % | 82.7 % | 2.2 % | 2.4 % | 9.2 % | 3.4 % | 39.7 % |
| seam muzzle | 125 | 0.0 % | 45.6 % | 0.0 % | 2.4 % | 16.8 % | 35.2 % | 4.0 % |
| seam spent | 123 | 0.0 % | 79.7 % | 0.0 % | 3.3 % | 11.4 % | 5.7 % | 12.2 % |
| wedge muzzle | 425 | 0.0 % | 32.9 % | 0.0 % | 0.9 % | 26.6 % | 39.5 % | 6.1 % |
| wedge spent | 424 | 0.0 % | 81.6 % | 2.8 % | 0.9 % | 7.3 % | 7.3 % | 27.6 % |
| curved facets muzzle | 343 | 0.0 % | 38.2 % | 0.0 % | 1.5 % | 25.4 % | 35.0 % | 5.2 % |
| curved facets spent | 343 | 0.0 % | 89.5 % | 0.0 % | 0.9 % | 7.3 % | 2.3 % | 23.6 % |
| moving target muzzle | 1158 | 0.0 % | 51.0 % | 0.0 % | 0.0 % | 14.3 % | 34.6 % | 4.9 % |

### T1f. Exit speed share of a first-exchange skid, per obliquity, against sin θ

| class | θ label | sin θ | skids | median | 95th | worst |
|---|---|---|---|---|---|---|
| full plate | 45° | 0.707 | 113 | 0.707 | 0.709 | 0.751 |
| full plate | 60° | 0.866 | 157 | 0.866 | 0.866 | 0.889 |
| full plate | 75° | 0.966 | 109 | 0.966 | 0.966 | 0.970 |
| full plate | 85° | 0.996 | 76 | 0.996 | 0.996 | 0.996 |
| thin plate | 45° | 0.707 | 92 | 0.707 | 0.707 | 0.739 |
| thin plate | 60° | 0.866 | 112 | 0.866 | 0.866 | 0.883 |
| thin plate | 75° | 0.966 | 195 | 0.966 | 0.966 | 0.970 |
| thin plate | 85° | 0.996 | 104 | 0.996 | 0.996 | 0.996 |
| edge | 45° | 0.707 | 84 | 0.707 | 0.909 | 0.949 |
| edge | 60° | 0.866 | 116 | 0.866 | 0.959 | 0.994 |
| edge | 75° | 0.966 | 135 | 0.949 | 0.982 | 0.992 |
| edge | 85° | 0.996 | 164 | 0.995 | 0.998 | 0.999 |
| strip | 45° | 0.707 | 89 | 0.707 | 0.826 | 0.892 |
| strip | 60° | 0.866 | 116 | 0.866 | 0.922 | 0.939 |
| strip | 75° | 0.966 | 129 | 0.916 | 0.979 | 0.999 |
| strip | 85° | 0.996 | 164 | 0.987 | 0.998 | 1.000 |
| inside face | 45° | 0.707 | 136 | 0.707 | 0.707 | 0.777 |
| inside face | 60° | 0.866 | 141 | 0.866 | 0.866 | 0.897 |
| inside face | 75° | 0.966 | 199 | 0.966 | 0.972 | 0.974 |
| inside face | 85° | 0.996 | 218 | 0.993 | 0.996 | 0.997 |
| narrow gap | 45° | 0.707 | 139 | 0.707 | 0.724 | 0.780 |
| narrow gap | 60° | 0.866 | 181 | 0.866 | 0.866 | 0.932 |
| narrow gap | 75° | 0.966 | 221 | 0.966 | 0.967 | 0.976 |
| narrow gap | 85° | 0.996 | 227 | 0.996 | 0.996 | 0.996 |
| raised detail | 45° | 0.707 | 65 | 0.683 | 0.763 | 0.872 |
| raised detail | 60° | 0.866 | 91 | 0.851 | 0.894 | 0.949 |
| raised detail | 75° | 0.966 | 120 | 0.957 | 0.982 | 0.984 |
| raised detail | 85° | 0.996 | 157 | 0.949 | 0.996 | 0.999 |
| seam | 45° | 0.707 | 34 | 0.707 | 0.721 | 0.753 |
| seam | 60° | 0.866 | 46 | 0.866 | 0.886 | 0.896 |
| seam | 75° | 0.966 | 42 | 0.966 | 0.966 | 0.972 |
| seam | 85° | 0.996 | 33 | 0.996 | 0.996 | 0.996 |
| wedge | 45° | 0.707 | 107 | 0.707 | 0.810 | 0.931 |
| wedge | 60° | 0.866 | 116 | 0.841 | 0.918 | 0.987 |
| wedge | 75° | 0.966 | 149 | 0.922 | 0.978 | 0.995 |
| wedge | 85° | 0.996 | 114 | 0.992 | 0.998 | 0.999 |
| curved facets | 45° | 0.707 | 86 | 0.707 | 0.795 | 0.822 |
| curved facets | 60° | 0.866 | 157 | 0.866 | 0.924 | 0.937 |
| curved facets | 75° | 0.966 | 140 | 0.966 | 0.978 | 0.982 |
| curved facets | 85° | 0.996 | 55 | 0.996 | 0.997 | 0.997 |
| moving target | 45° | 0.707 | 70 | 0.707 | 0.931 | 0.947 |
| moving target | 60° | 0.866 | 111 | 0.866 | 0.881 | 0.975 |
| moving target | 75° | 0.966 | 182 | 0.966 | 0.974 | 0.982 |
| moving target | 85° | 0.996 | 228 | 0.996 | 0.998 | 0.999 |

### T1e. How the shot ended

| class | n | flying on | rest | buried | a burst mark |
|---|---|---|---|---|---|
| full plate | 568 | 50.9 % | 14.3 % | 34.9 % | 14.4 % |
| thin plate | 740 | 44.1 % | 16.8 % | 39.2 % | 27.0 % |
| edge | 960 | 37.4 % | 29.6 % | 33.0 % | 24.8 % |
| strip | 906 | 34.7 % | 21.0 % | 44.4 % | 25.8 % |
| inside face | 960 | 15.4 % | 29.6 % | 55.0 % | 26.5 % |
| narrow gap | 1050 | 43.9 % | 19.9 % | 36.2 % | 25.6 % |
| raised detail | 822 | 26.4 % | 27.3 % | 46.4 % | 30.2 % |
| seam | 248 | 52.0 % | 12.1 % | 35.9 % | 19.0 % |
| wedge | 849 | 27.8 % | 24.3 % | 47.9 % | 24.6 % |
| curved facets | 686 | 40.7 % | 19.1 % | 40.2 % | 25.2 % |
| moving target | 1158 | 47.5 % | 7.3 % | 45.3 % | 0.0 % |

### T2. Strike offset: the mark from the axis at the exchange, in the round's radii

First exchange. `off axis` is the mark's distance from the axis point at the exchange; `off line` its distance from the axis line. Radii of calibre / 2, a 7.9 mm round included.

| class | n | off axis median | 95th | worst | within 1 r | off line median | 95th | worst |
|---|---|---|---|---|---|---|---|---|
| full plate | 568 | 2.05 | 14.38 | 14.38 | 18.5 % | 0.00 | 1.00 | 1.00 |
| thin plate | 740 | 1.73 | 12.12 | 14.83 | 29.6 % | 0.00 | 1.00 | 1.00 |
| edge | 958 | 1.23 | 7.61 | 35.55 | 34.0 % | 0.27 | 1.00 | 1.00 |
| strip | 906 | 1.45 | 7.75 | 44.94 | 32.1 % | 0.08 | 1.00 | 1.00 |
| inside face | 960 | 1.95 | 12.12 | 14.38 | 21.9 % | 0.00 | 1.00 | 1.00 |
| narrow gap | 1050 | 2.05 | 12.12 | 14.44 | 25.8 % | 0.00 | 1.00 | 1.00 |
| raised detail | 822 | 1.40 | 7.18 | 14.38 | 34.5 % | 0.00 | 1.00 | 1.00 |
| seam | 248 | 1.69 | 12.12 | 14.38 | 37.1 % | 0.00 | 1.00 | 1.00 |
| wedge | 849 | 1.40 | 8.26 | 25.95 | 34.4 % | 0.00 | 1.00 | 1.00 |
| curved facets | 686 | 1.61 | 11.30 | 15.67 | 32.8 % | 0.00 | 1.00 | 1.00 |
| moving target | 1158 | 1.77 | 14.38 | 35.55 | 44.3 % | 0.00 | 0.47 | 0.97 |

### T2a. The same in millimetres per obliquity, skids only, at the largest calibre

| class | θ | n | off axis median mm | 95th | worst | DERIVED r/cos θ mm |
|---|---|---|---|---|---|---|
| full plate | 45° | 50 | 64 | 64 | 64 | 62 |
| full plate | 60° | 74 | 90 | 90 | 90 | 88 |
| full plate | 75° | 18 | 174 | 174 | 174 | 170 |
| thin plate | 45° | 67 | 64 | 64 | 64 | 62 |
| thin plate | 60° | 78 | 90 | 90 | 90 | 88 |
| thin plate | 75° | 126 | 174 | 174 | 174 | 170 |
| thin plate | 85° | 45 | 516 | 516 | 516 | 505 |
| edge | 45° | 39 | 49 | 62 | 64 | 62 |
| edge | 60° | 69 | 62 | 273 | 273 | 88 |
| edge | 75° | 83 | 61 | 136 | 141 | 170 |
| edge | 85° | 98 | 136 | 379 | 449 | 505 |
| strip | 45° | 50 | 64 | 69 | 69 | 62 |
| strip | 60° | 51 | 71 | 93 | 93 | 88 |
| strip | 75° | 73 | 81 | 112 | 118 | 170 |
| strip | 85° | 94 | 148 | 313 | 354 | 505 |
| inside face | 45° | 71 | 64 | 64 | 71 | 62 |
| inside face | 60° | 68 | 76 | 90 | 90 | 88 |
| inside face | 75° | 116 | 174 | 174 | 193 | 170 |
| inside face | 85° | 125 | 131 | 516 | 516 | 505 |
| narrow gap | 45° | 69 | 64 | 64 | 65 | 62 |
| narrow gap | 60° | 104 | 90 | 90 | 90 | 88 |
| narrow gap | 75° | 129 | 174 | 174 | 177 | 170 |
| narrow gap | 85° | 132 | 516 | 516 | 516 | 505 |
| raised detail | 45° | 48 | 60 | 68 | 70 | 62 |
| raised detail | 60° | 69 | 82 | 99 | 100 | 88 |
| raised detail | 75° | 77 | 120 | 195 | 196 | 170 |
| raised detail | 85° | 96 | 93 | 316 | 336 | 505 |
| seam | 45° | 20 | 64 | 64 | 64 | 62 |
| seam | 60° | 22 | 90 | 90 | 90 | 88 |
| seam | 75° | 27 | 174 | 174 | 174 | 170 |
| seam | 85° | 15 | 516 | 516 | 516 | 505 |
| wedge | 45° | 52 | 58 | 71 | 71 | 62 |
| wedge | 60° | 56 | 63 | 102 | 111 | 88 |
| wedge | 75° | 78 | 94 | 124 | 150 | 170 |
| wedge | 85° | 37 | 109 | 286 | 286 | 505 |
| curved facets | 45° | 61 | 61 | 74 | 74 | 62 |
| curved facets | 60° | 91 | 81 | 115 | 118 | 88 |
| curved facets | 75° | 77 | 154 | 214 | 217 | 170 |
| curved facets | 85° | 27 | 497 | 539 | 539 | 505 |
| moving target | 45° | 10 | 68 | 72 | 72 | 62 |
| moving target | 60° | 26 | 90 | 90 | 90 | 88 |
| moving target | 75° | 64 | 174 | 204 | 217 | 170 |
| moving target | 85° | 74 | 497 | 516 | 539 | 505 |

### T3. Exchanges at one contact

One contact is a run of consecutive exchanges each sharing a shell with, or coplanar with, the one before. `zero clock`: exchanges with no axis travel; the 5+ column is the design's over-4 gate.

| class | contacts | 1 | 2 | 3 | 4 | 5+ | worst | exchanges | at zero clock | shots with one |
|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 987 | 854 | 65 | 39 | 12 | 17 | 18 | 1279 | 103 | 100 |
| thin plate | 1412 | 1244 | 78 | 41 | 16 | 33 | 31 | 1921 | 206 | 192 |
| edge | 1487 | 1019 | 259 | 99 | 41 | 69 | 850 | 3750 | 406 | 384 |
| strip | 1464 | 1029 | 277 | 100 | 29 | 29 | 34 | 2252 | 353 | 333 |
| inside face | 2003 | 1714 | 136 | 92 | 23 | 38 | 27 | 2839 | 307 | 285 |
| narrow gap | 1942 | 1667 | 142 | 59 | 27 | 47 | 585 | 4918 | 331 | 309 |
| raised detail | 1354 | 917 | 221 | 73 | 55 | 88 | 15 | 2399 | 506 | 402 |
| seam | 417 | 355 | 33 | 18 | 3 | 8 | 38 | 599 | 49 | 45 |
| wedge | 1474 | 999 | 257 | 116 | 41 | 61 | 23 | 2455 | 412 | 339 |
| curved facets | 1146 | 810 | 179 | 73 | 37 | 47 | 30 | 1960 | 309 | 283 |
| moving target | 2285 | 1936 | 175 | 89 | 43 | 42 | 96 | 3622 | 289 | 228 |

### T4. Aim continuity: two aims 0.5 mm apart

First exchange of each. Exit angle over pairs that both leave; speed step (of speed in) over all pairs. `flips`: the two read different outcomes. `past §7.2`: angle step beyond the design's accepted step at that obliquity (9.2°, 18.5°, 27.7° at 75°, 60°, 45°).

| class | pairs | flips | angle n | median ° | 95th | worst | past §7.2 | speed median % | 95th | worst |
|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 568 | 6 | 495 | 0.00 | 0.00 | 23.49 | 0 | 0.00 | 0.34 | 86.60 |
| thin plate | 740 | 9 | 664 | 0.00 | 0.00 | 30.00 | 3 | 0.00 | 0.24 | 99.62 |
| edge | 958 | 46 | 804 | 0.00 | 2.15 | 67.60 | 8 | 0.00 | 11.20 | 99.62 |
| strip | 906 | 40 | 730 | 0.00 | 4.13 | 51.26 | 3 | 0.00 | 6.14 | 99.80 |
| inside face | 960 | 14 | 815 | 0.00 | 0.00 | 7.79 | 0 | 0.00 | 0.19 | 99.62 |
| narrow gap | 960 | 12 | 859 | 0.00 | 0.00 | 46.01 | 2 | 0.00 | 0.00 | 99.62 |
| raised detail | 822 | 27 | 611 | 0.00 | 3.05 | 30.01 | 1 | 0.00 | 6.70 | 99.62 |
| seam | 246 | 1 | 204 | 0.00 | 0.00 | 10.30 | 0 | 0.00 | 0.90 | 99.62 |
| wedge | 847 | 25 | 674 | 0.00 | 4.92 | 55.82 | 12 | 0.00 | 5.52 | 99.77 |
| curved facets | 686 | 19 | 556 | 0.00 | 2.66 | 11.23 | 0 | 0.00 | 3.69 | 99.65 |

### T5. Tick phase: one aim at 7 phases

Phases [0.001, 0.01, 0.05, 0.25, 0.5, 0.75, 0.95] of the first tick flown before first touch. `flips` is aims whose first outcome changed with phase; `flips ≥ 0.05` drops the two smallest phases, which are deliberately small. Spread is the widest distance between the axis points of the first exchange, and between exit speeds as a share of speed in.

| class | aims | flips | flips ≥ 0.05 | turn spread median mm | 95th | worst | speed spread median % | 95th | worst |
|---|---|---|---|---|---|---|---|---|---|
| full plate | 226 | 2 | 2 | 7.3 | 56.8 | 56.8 | 0.00 | 0.00 | 70.71 |
| thin plate | 268 | 6 | 6 | 0.0 | 19.7 | 56.8 | 0.00 | 2.17 | 99.62 |
| edge | 334 | 11 | 10 | 0.0 | 25.5 | 165.5 | 0.00 | 3.23 | 99.71 |
| strip | 248 | 3 | 3 | 0.0 | 43.8 | 313.3 | 0.00 | 1.32 | 98.96 |
| narrow gap | 322 | 8 | 7 | 9.6 | 56.8 | 67.4 | 0.00 | 0.00 | 99.62 |
| raised detail | 258 | 8 | 7 | 0.0 | 56.8 | 331.8 | 0.00 | 3.12 | 95.93 |
| seam | 76 | 2 | 2 | 0.0 | 19.8 | 56.8 | 0.00 | 0.00 | 96.59 |
| wedge | 268 | 6 | 5 | 0.0 | 49.6 | 392.6 | 0.00 | 0.02 | 87.62 |
| curved facets | 254 | 10 | 10 | 0.0 | 22.4 | 56.8 | 0.00 | 0.47 | 99.65 |

### T6. Unrelated motion, and a moving target

Same shot with a kinematic body 40 m off the tank standing still, then moving 20 m/s. Compared bit for bit on every noted exchange's kind, speeds, strike and axis point. `repeat` is the same still shot flown twice, the noise floor.

| shots | repeat differs | motion differs | of which outcome sequence differs | widest first-exchange axis move mm |
|---|---|---|---|---|
| 579 | 0 | 0 | 0 | 0.000 |

A target moving 15 m/s (across the line, and closing on it) against the same relative shot at a still target, first exchange in the body's frame, one round per calibre of 3 calibres.

| aim class | shots | contact flips | outcome flips | strike move median mm | 95th | worst | axis move median mm | 95th | worst | exit angle median ° | 95th | worst | speed median % | 95th | worst |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 106 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.00 | 0.01 | 0.01 | 0.000 | 0.000 | 0.028 | 0.000 | 0.000 | 0.147 |
| thin plate | 114 | 0 | 0 | 0.00 | 0.01 | 0.02 | 0.00 | 0.01 | 0.82 | 0.000 | 0.000 | 0.020 | 0.000 | 0.001 | 0.016 |
| edge | 120 | 0 | 0 | 0.00 | 0.16 | 11.56 | 0.00 | 0.02 | 0.73 | 0.000 | 0.000 | 0.000 | 0.000 | 0.026 | 0.297 |
| strip | 120 | 0 | 0 | 0.00 | 0.08 | 2.89 | 0.00 | 0.02 | 0.06 | 0.000 | 0.000 | 0.020 | 0.000 | 0.022 | 0.199 |
| inside face | 120 | 0 | 0 | 0.00 | 0.02 | 0.09 | 0.00 | 0.01 | 0.03 | 0.000 | 0.000 | 0.028 | 0.000 | 0.001 | 0.337 |
| narrow gap | 120 | 0 | 0 | 0.00 | 0.01 | 0.02 | 0.00 | 0.01 | 0.02 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.042 |
| raised detail | 112 | 0 | 0 | 0.00 | 0.04 | 0.21 | 0.00 | 0.01 | 1.52 | 0.000 | 0.000 | 0.510 | 0.000 | 0.013 | 0.287 |
| seam | 106 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.00 | 0.01 | 0.01 | 0.000 | 0.000 | 0.028 | 0.000 | 0.002 | 6.239 |
| wedge | 120 | 0 | 1 | 0.00 | 0.08 | 0.62 | 0.00 | 0.01 | 86.20 | 0.000 | 0.000 | 12.901 | 0.000 | 0.018 | 77.515 |
| curved facets | 120 | 0 | 0 | 0.00 | 0.02 | 0.18 | 0.00 | 0.01 | 0.05 | 0.000 | 0.000 | 0.028 | 0.000 | 0.017 | 11.254 |

### T7. Crossed-plate re-reads

After a through, a later read in the same shot whose faces include a primitive and shell the crossing read. The same closed shell has a far wall, so `near` keeps reads within two calibres of the hole: the plate actually just crossed. `gap, just through` flies each gap aim at the speed that only just gets through its first plate.

| class | shots through | shots re-reading | re-reads | near | near re-bites | near zero-turn | all re-bites | farthest from the hole mm |
|---|---|---|---|---|---|---|---|---|
| full plate | 114 | 22 | 42 | 4 | 1 | 1 | 8 | 894 |
| thin plate | 258 | 44 | 112 | 10 | 6 | 0 | 25 | 2322 |
| edge | 427 | 100 | 1024 | 72 | 37 | 9 | 73 | 1957 |
| strip | 398 | 134 | 192 | 65 | 37 | 7 | 104 | 2059 |
| inside face | 330 | 110 | 225 | 28 | 4 | 5 | 48 | 3067 |
| narrow gap | 217 | 40 | 98 | 33 | 19 | 3 | 46 | 1721 |
| raised detail | 389 | 170 | 481 | 147 | 44 | 27 | 134 | 1373 |
| seam | 75 | 12 | 30 | 3 | 2 | 0 | 11 | 2309 |
| wedge | 376 | 173 | 381 | 100 | 32 | 15 | 105 | 2673 |
| curved facets | 292 | 64 | 170 | 51 | 11 | 9 | 34 | 2436 |
| moving target | 598 | 141 | 274 | 80 | 57 | 11 | 150 | 2788 |
| gap, just through | 89 | 18 | 34 | 17 | 1 | 9 | 1 | 673 |

### T8. Ownership on real meshes

The mesh: 21526 armour triangles, 82 primitives, 131 closed shells, 249.9 m² of faces. Flat patches (normals within 2.6°, 2 mm of one plane, gaps at most 88 mm): 7104 of more than one triangle; 19 span more than one primitive (13.4 % of the face area), 19 span more than one shell (13.4 % of the face area). A hit names its volume, primitive, closed shell, triangle and welded feature (triangle interior, welded edge or vertex); no finite patch identity exists in the certificate.

At classification, per aim's disc (uniform aims only are a hull-wide rate): the welded coplanar patch under the disc spanning more than one primitive or shell, a sample's first run spanning more than one primitive, and the features the hits name.

| calibre | uniform aims | patch over >1 primitive | patch over >1 shell | first run over >1 primitive | hits on triangle interior | on welded edge | on welded vertex |
|---|---|---|---|---|---|---|---|
| 88.0 mm | 738 | 8.7 % | 8.7 % | 45.1 % | 100.0 % | 0.0 % | 0.0 % |
| 35.7 mm | 887 | 5.5 % | 5.5 % | 33.0 % | 100.0 % | 0.0 % | 0.0 % |
| 7.9 mm | 1016 | 1.2 % | 1.2 % | 22.2 % | 100.0 % | 0.0 % | 0.0 % |

"Own the whole run": after a first contact, the later reads in the same shot an owner of that contact's scope would skip, landing beyond the first contact's footprint (`r / cos θ + r`), so on material the round had not reached. Scopes: the primitive, the shell, and the coplanar plane. `of which bite` and `of which dose` are what skipping would remove. `welded A+B` counts first exchanges whose run spans more than one primitive and that ended in a bite: owning the run takes its unreached primitives with it.

| class | later reads | beyond footprint | in primitive | of which bite | of which dose | in shell | of which bite | of which dose | in plane | of which bite | of which dose | welded A+B bites / bites |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| full plate | 711 | 661 | 157 | 21 | 157 | 148 | 17 | 148 | 53 | 5 | 53 | 58 / 107 |
| thin plate | 1181 | 1071 | 120 | 23 | 105 | 109 | 18 | 94 | 78 | 9 | 63 | 0 / 222 |
| edge | 2792 | 1637 | 199 | 60 | 199 | 160 | 46 | 160 | 25 | 7 | 25 | 115 / 445 |
| strip | 1346 | 858 | 233 | 106 | 233 | 186 | 81 | 186 | 55 | 30 | 55 | 157 / 397 |
| inside face | 1879 | 1327 | 291 | 42 | 291 | 286 | 42 | 286 | 151 | 32 | 151 | 51 / 244 |
| narrow gap | 3868 | 3631 | 2542 | 43 | 845 | 1974 | 19 | 277 | 1797 | 17 | 98 | 46 / 261 |
| raised detail | 1577 | 1037 | 526 | 110 | 526 | 488 | 86 | 488 | 181 | 23 | 181 | 180 / 365 |
| seam | 351 | 324 | 129 | 6 | 62 | 126 | 4 | 59 | 97 | 1 | 30 | 80 / 86 |
| wedge | 1606 | 1029 | 450 | 81 | 450 | 422 | 66 | 422 | 223 | 29 | 223 | 123 / 343 |
| curved facets | 1274 | 595 | 165 | 40 | 165 | 132 | 21 | 132 | 18 | 0 | 18 | 173 / 240 |
| moving target | 2464 | 2067 | 767 | 106 | 767 | 374 | 65 | 374 | 24 | 15 | 24 | 211 / 567 |

### T9. One contact read

The closing walk plus the law's verdict on it, per exchange, µs, in the edit profile: a dev build, not a shipping one, on a machine other agents share (no compile runs during it; other work may). Not frame or tick time.

| | n | median µs | 95th | worst |
|---|---|---|---|---|
| 88.0 mm | 14116 | 67 | 388 | 3315 |
| 35.7 mm | 8226 | 64 | 326 | 1360 |
| 7.9 mm | 5652 | 5 | 16 | 111 |

### Sanity counts

Exchanges that should not exist if the march holds its own laws. `gained speed`: a leaving exchange left faster than it arrived. `into the face`: a full-coverage skid whose exit heads into the face it read. `backward`: a leaving exchange whose exit opposes the axis it arrived on.

| class | exchanges | gained speed | into the face | backward |
|---|---|---|---|---|
| full plate | 1279 | 0 | 0 | 0 |
| thin plate | 1921 | 0 | 0 | 0 |
| edge | 3750 | 0 | 0 | 0 |
| strip | 2252 | 0 | 0 | 0 |
| inside face | 2839 | 0 | 0 | 0 |
| narrow gap | 4918 | 0 | 0 | 0 |
| raised detail | 2399 | 0 | 0 | 0 |
| seam | 599 | 0 | 0 | 0 |
| wedge | 2455 | 0 | 0 | 0 |
| curved facets | 1960 | 0 | 0 | 0 |
| moving target | 3622 | 0 | 0 | 0 |

### Reproductions

Per class, its longest chain and its first zero-turn rest, as the seeded instrument flew them. World metres; the tank stands at its spawn pose unless a line says it moves.

- full plate longest chain: kwk36_l56 88mm_APCR at muzzle (20 exchanges), birth Vec3(8.13949, 5.209312, -6.1122766), post-step velocity Vec3(838.8892, 41.642204, -399.28802)
- full plate first zero-turn: kwk36_l56 88mm_AP at spent (7 exchanges), birth Vec3(13.722044, 5.0741754, -9.9128895), post-step velocity Vec3(-134.6896, -48.528957, 129.80453)
- thin plate longest chain: kwk36_l56 88mm_APCR at muzzle (37 exchanges), birth Vec3(22.049187, 11.593292, -5.4444304), post-step velocity Vec3(-804.1979, -464.95267, -44.550327)
- thin plate first zero-turn: kwk36_l56 88mm_AP at muzzle (5 exchanges), birth Vec3(9.558358, 8.339088, -5.126517), post-step velocity Vec3(538.44635, -546.59357, 94.02138)
- edge longest chain: kwk36_l56 88mm_APCR at spent (851 exchanges), birth Vec3(9.582715, 7.050139, -10.259464), post-step velocity Vec3(136.28441, -138.93864, 127.19618)
- edge first zero-turn: kwk36_l56 88mm_AP at muzzle (5 exchanges), birth Vec3(4.9817486, 0.5446081, -13.172495), post-step velocity Vec3(518.10205, 263.33224, 509.6621)
- strip longest chain: mg34 7.9mm_AP at spent (38 exchanges), birth Vec3(10.152986, 7.1120834, -6.5128107), post-step velocity Vec3(130.45758, -133.76167, -26.743206)
- strip first zero-turn: kwk36_l56 88mm_AP at muzzle (6 exchanges), birth Vec3(14.369194, 9.923744, -19.053654), post-step velocity Vec3(-243.79204, -396.2943, 617.28864)
- inside face longest chain: kwk36_l56 88mm_APCR at spent (28 exchanges), birth Vec3(11.087961, 4.980712, -6.6149206), post-step velocity Vec3(81.468506, -60.993103, 209.04301)
- inside face first zero-turn: kwk36_l56 88mm_AP at muzzle (4 exchanges), birth Vec3(11.732101, 4.3205953, -7.7961326), post-step velocity Vec3(-546.5935, -234.10826, 493.92087)
- narrow gap longest chain: kwk36_l56 88mm_APCR at spent (586 exchanges), birth Vec3(16.426207, 5.0934763, -6.494362), post-step velocity Vec3(-217.53993, -20.263647, 79.511116)
- narrow gap first zero-turn: kwk36_l56 88mm_AP at muzzle (6 exchanges), birth Vec3(7.62436, 6.1634655, -7.2766438), post-step velocity Vec3(524.24927, -546.5441, 154.85875)
- raised detail longest chain: kwk36_l56 88mm_APHE_ring at spent (23 exchanges), birth Vec3(9.696342, 2.1348114, -5.8388863), post-step velocity Vec3(55.580597, 112.45267, -147.00598)
- raised detail first zero-turn: kwk36_l56 88mm_AP at muzzle (6 exchanges), birth Vec3(5.315954, 7.6448927, -4.803032), post-step velocity Vec3(554.92615, -410.9732, -347.40045)
- seam longest chain: kwk36_l56 88mm_APHE_ring at spent (39 exchanges), birth Vec3(10.570225, 5.4949126, -10.398979), post-step velocity Vec3(-5.4192257, -50.01674, 186.5865)
- seam first zero-turn: kwk36_l56 88mm_AP at spent (3 exchanges), birth Vec3(11.684327, 2.583852, -10.570558), post-step velocity Vec3(147.06148, 45.04086, 117.00346)
- wedge longest chain: mg34 7.9mm_AP at spent (26 exchanges), birth Vec3(12.660804, 7.745212, -6.4579477), post-step velocity Vec3(-104.921745, -155.71931, 19.222202)
- wedge first zero-turn: kwk36_l56 88mm_AP at muzzle (5 exchanges), birth Vec3(13.525264, 0.44876647, -7.170403), post-step velocity Vec3(-546.5935, 540.239, 83.10444)
- curved facets longest chain: kwk36_l56 88mm_APHE_ring at muzzle (32 exchanges), birth Vec3(18.393211, 1.4165838, -0.3976822), post-step velocity Vec3(-383.04364, 222.88293, -633.3481)
- curved facets first zero-turn: kwk36_l56 88mm_AP at muzzle (8 exchanges), birth Vec3(14.410293, 3.1849966, -8.6979885), post-step velocity Vec3(-588.8636, 478.21426, 148.59282)
- moving target longest chain: mg34 7.9mm_AP at muzzle (98 exchanges), birth Vec3(13.829657, 6.8481746, -9.426392), post-step velocity Vec3(-492.6046, -191.52612, 517.9367)
- moving target first zero-turn: kwk36_l56 88mm_AP at muzzle (5 exchanges), birth Vec3(9.661281, 8.468985, -4.9607887), post-step velocity Vec3(545.0334, -538.2803, 104.62798)

### Panics met in the march

- seam aim, `kwk36_l56 88mm_AP`: born at Vec3(13.448209, 5.9313054, -3.65905), post-step velocity Vec3(-114.379295, -136.64839, -74.76734), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_AP`: born at Vec3(13.447991, 5.931244, -3.6586041), post-step velocity Vec3(-114.379295, -136.64839, -74.76734), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_AP`: born at Vec3(9.337844, 5.4328265, -8.553225), post-step velocity Vec3(92.55517, -50.016808, 162.10312), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_AP`: born at Vec3(9.04861, 5.589129, -9.059797), post-step velocity Vec3(92.55517, -50.016808, 162.10312), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_AP`: born at Vec3(10.570463, 5.4944863, -10.399087), post-step velocity Vec3(-5.4192257, -50.01674, 186.5865), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_APHE_ring`: born at Vec3(13.448209, 5.9313054, -3.65905), post-step velocity Vec3(-114.379295, -136.64839, -74.76734), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_APHE_ring`: born at Vec3(13.447991, 5.931244, -3.6586041), post-step velocity Vec3(-114.379295, -136.64839, -74.76734), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_APHE_ring`: born at Vec3(9.337844, 5.4328265, -8.553225), post-step velocity Vec3(92.55517, -50.016808, 162.10312), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_APHE_ring`: born at Vec3(9.04861, 5.589129, -9.059797), post-step velocity Vec3(92.55517, -50.016808, 162.10312), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- seam aim, `kwk36_l56 88mm_APHE_ring`: born at Vec3(10.570463, 5.4944863, -10.399087), post-step velocity Vec3(-5.4192257, -50.01674, 186.5865), tank at Vec3(12.0, 3.0, -7.0): game's damage model: a finite step fits its crossing limit; drain_exhausted: DrainExhausted { crossings: 4097 }
- wedge aim, `mg34 7.9mm_AP`: born at Vec3(11.882101, -3.3190238, -4.647451), post-step velocity Vec3(153.88496, 666.9333, -318.66043), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 304v0, primitive: 304v0, shell: 0 }, t: 0.5691003793721444, triangles: [56] }
- edge aim, `mg34 7.9mm_AP`: born at Vec3(13.480293, 3.381734, -6.838281), post-step velocity Vec3(-300.5629, 434.80978, 539.0987), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 308v0, primitive: 308v0, shell: 0 }, t: 0.38461666669378414, triangles: [159] }
- narrow gap aim, `mg34 7.9mm_AP`: born at Vec3(10.371233, 4.795458, -6.657592), post-step velocity Vec3(195.4084, -728.5137, -33.291897), tank at Vec3(12.0, 3.0, -7.0): game's damage model: the closing corridor must contain every opened shell's exit; incomplete_corridor: IncompleteCorridor { sample: 0, open: [ShellKey { volume: 209v0, primitive: 262v0, shell: 0 }], length: 0.022350319 }
- full plate aim, `mg34 7.9mm_AP`: born at Vec3(13.626741, 4.3076735, -4.3707595), post-step velocity Vec3(-65.802635, -546.7091, 516.531), tank at Vec3(12.0, 3.0, -7.0): game's damage model: the closing corridor must contain every opened shell's exit; incomplete_corridor: IncompleteCorridor { sample: 0, open: [ShellKey { volume: 208v0, primitive: 263v0, shell: 0 }], length: 0.6971152 }
- full plate aim, `mg34 7.9mm_AP`: born at Vec3(13.627799, 4.562245, -5.3784), post-step velocity Vec3(-133.4664, 113.61563, -70.034065), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 208v0, primitive: 263v0, shell: 0 }, t: 0.28324704489944896, triangles: [2] }
- edge aim, `mg34 7.9mm_AP`: born at Vec3(10.199447, 3.6056461, -7.847377), post-step velocity Vec3(94.37499, -163.12282, -10.529476), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 288v0, primitive: 288v0, shell: 1 }, t: 0.3019140970476435, triangles: [250] }
- wedge aim, `mg34 7.9mm_AP`: born at Vec3(13.0494995, 1.7382418, -7.063931), post-step velocity Vec3(38.47124, 166.73332, -79.66511), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 304v0, primitive: 304v0, shell: 0 }, t: 0.5691004158383528, triangles: [56] }
- thin plate aim, `mg34 7.9mm_AP`: born at Vec3(13.627188, 4.792919, -6.4707155), post-step velocity Vec3(-94.37499, 163.26141, -8.101518), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 208v0, primitive: 263v0, shell: 0 }, t: 0.009748966712674325, triangles: [2] }
- narrow gap aim, `mg34 7.9mm_AP`: born at Vec3(10.372812, 4.7832394, -7.2671714), post-step velocity Vec3(94.37499, -115.535774, -115.63481), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 209v0, primitive: 262v0, shell: 0 }, t: 0.7666039954659083, triangles: [2] }
- edge aim, `mg34 7.9mm_AP`: born at Vec3(13.476771, 3.3868294, -6.8319635), post-step velocity Vec3(-75.140724, 108.702446, 134.77467), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 308v0, primitive: 308v0, shell: 0 }, t: 0.3846166060660492, triangles: [159] }
- narrow gap aim, `mg34 7.9mm_AP`: born at Vec3(10.373523, 4.7869205, -6.657982), post-step velocity Vec3(48.8521, -182.12842, -8.322974), tank at Vec3(12.0, 3.0, -7.0): game's damage model: the closing corridor must contain every opened shell's exit; incomplete_corridor: IncompleteCorridor { sample: 0, open: [ShellKey { volume: 209v0, primitive: 262v0, shell: 0 }], length: 0.02235098 }
- full plate aim, `mg34 7.9mm_AP`: born at Vec3(13.625971, 4.3012667, -4.3647065), post-step velocity Vec3(-16.450659, -136.67728, 129.13275), tank at Vec3(12.0, 3.0, -7.0): game's damage model: the closing corridor must contain every opened shell's exit; incomplete_corridor: IncompleteCorridor { sample: 0, open: [ShellKey { volume: 208v0, primitive: 263v0, shell: 0 }], length: 0.69712174 }
- full plate aim, `mg34 7.9mm_AP`: born at Vec3(13.628284, 4.320487, -4.3828654), post-step velocity Vec3(-16.450659, -136.67728, 129.13275), tank at Vec3(12.0, 3.0, -7.0): game's damage model: the closing corridor must contain every opened shell's exit; incomplete_corridor: IncompleteCorridor { sample: 0, open: [ShellKey { volume: 208v0, primitive: 263v0, shell: 0 }], length: 0.6971237 }
- wedge aim, `mg34 7.9mm_AP`: born at Vec3(10.076007, 1.3322017, -5.4783688), post-step velocity Vec3(36.176228, 163.79178, -86.54535), tank at Vec3(12.0, 3.0, -7.0): game's damage model: certified closed shells alternate from established membership; unexpected_exit: UnexpectedExit { sample: 0, key: ShellKey { volume: 292v0, primitive: 292v0, shell: 0 }, t: 0.4851020517145097, triangles: [166] }


<!-- contact_classes tables end -->

## The design against real code

The design's numbers came from its fixture model. Here they are beside the real march at HEAD, handoff fix included.

| Design's claim | Real code | Reading |
|---|---|---|
| 51 % of edge aims at 75° rest at zero turn | first exchange 1.2 % at 75° (HE arm only; kinetic 0 %); anywhere in the shot 31.2 % (n 240) | the handoff fix moved it one exchange later; it did not remove it |
| all 12 strip shots rest or buried | first exchange 55.0 % skid, 27.8 % through, 16.0 % buried, 1.1 % zero-turn (n 906); anywhere 17.0 % | the claim is wrong on real code |
| marks 174 mm at 75°, 516 mm at 85° | 174 and 516 on full-coverage classes, DERIVED `r/cos θ` 170 and 505 | confirmed to the millimetre |
| up to 10 % of phases on a full plate become a long bite | 2 of 226 aims change outcome at any of 7 phases; turn spread median 7.3, worst 56.8 mm | the handoff fix nearly solved this |
| a spent round in a gap re-bites the crossed plate | just-through speed: 1 near re-bite and 9 near zero-turn rests in 89 shots through; other speeds 19 near re-bites in 217 | real, and rare; the rests are the larger part |
| unrelated motion can get in | 0 of 579 shots differ, bit for bit; repeat noise floor 0 | the handoff fix already solved this |
| a moving target may differ from its static equivalent | 0 contact flips, 1 outcome flip in 1158 (a wedge aim); strike move 95th at most 0.16 mm, worst 11.56 mm | the handoff fix already solved this |
| today's aim step is a 97–100 % speed change | that is the worst case (99.62 %); edge median 0, 95th 11.20 %; 46 of 958 edge pairs flip outcome | true at the extreme, not the typical step |
| full plate exit share equals sin θ | medians 0.707, 0.866, 0.966, 0.996 at 45°, 60°, 75°, 85°, each equal to sin θ | confirmed: the law is already right |

## §8 verdict

**Does the premise hold? Yes, moved.** §8 says it fails when edge zero-turn rests fall under 1 % and strikes lie within `r`. Neither holds. At the first exchange edge rests are 1.0 %, all of it the HE arm; but 25.2 % of edge shots end on the zero-turn terminal somewhere in the shot, at a mean 243 m/s lost, 232 of 242 straight after a skid and 231 of those on the surface just skidded, with no distance flown between (median 0.0 mm). Its cause reads as the same one: the rim is still in the plate, so the next read holds material with no way to turn. Only 34.0 % of edge strikes lie within `r` (95th 7.61 r). The rate climbs as speed falls: edge 8.8 % at muzzle speed, 41.7 % spent. So a shallow glance on a disc round still often stops dead on its second read. Discs only: 7.9 mm reads zero.

**Strike offset: stands, with one caveat.** The mark is 64, 90, 174, 516 mm from the axis at 45°, 60°, 75°, 85°, as the design says. Its distance from the line of fire is 0.00 r at median on every flat class and at most 1.00 r: the mark stays on the line and the turn falls short along it. Whether that reads wrong to a player is a matter of stills, not measured here.

**Does the rebuild shrink? Yes, but not to lanes 4 and 7.** The numbers ask for lanes 2 and 3 (turn at first reach, ownership, hold, exit) and the mark half of lane 5. Lane 4 has no wrong state left to fix: moving equals static and unrelated motion is zero; keep its witnesses, not its rewrite. Lane 7 and lanes 6, 8, 9, 10 concern readers and the clock, which this lane did not measure, so these numbers neither support nor refute them; their case rests on the audit's trace alone. That choice is the owner's.

**Which classes carry the case.** Edge, raised detail, strip and wedge: zero-turn after a skid 17–25 %, and 24–34 % for the 88 mm AP arm on edge, strip and raised detail. Then narrow gap and inside face for chains and re-reads. Seam carries the crossing-limit panic. Full and thin plate carry only the marks: their exit law is already right, and 854 of 987 full-plate contacts are one exchange. Moving carries nothing.

**Chains are the finding the design undersold.** §8 allows 4 exchanges at one contact and none at zero clock. Today 1.7–6.5 % of contacts run to 5 or more, 7–21 % of exchanges are at zero clock, the longest run is 850 exchanges at one edge contact and 585 in a gap, and past 4096 in one step the march panics (defect 1).

**Ownership, the second read's question.** Can the design's surface identity exist? Not as written. Available at a hit: volume, primitive, closed shell, triangle, welded feature (uniform aims: 100 % triangle interior). There is no finite patch identity in the certificate. 19 flat patches span more than one primitive and shell, 13.4 % of face area, under 8.7 %, 5.5 % and 1.2 % of uniform discs at 88, 35.7 and 7.9 mm; but one sample's first welded run spans more than one primitive on 45.1 %, 33.0 % and 22.2 %. Owning the whole run takes unreached primitives with it on first-exchange bites: full plate 58 of 107, strip 157 of 397, raised detail 180 of 365, seam 80 of 86, curved facets 173 of 240; thin plate 0 of 222. Beyond the first footprint, an owner kept for the whole shot would skip, over the ten static classes (DERIVED sums of T8, an upper bound since a real contact ends): by primitive 4812 of 12170 later reads (39.5 %), 532 of them bites and 3033 carrying dose; by coplanar plane 2678 (22.0 %), 153 bites and 897 with dose. This is measured, not designed: whichever identity lane 2 picks has to answer these counts first.

## Defects, not fixed here

Each reproduces by flying the named arm from the stated birth at the stated post-step velocity, tank at the spawn pose of the range, fixed tick 1/64 s; every line is under "Panics met in the march" and "Reproductions".

- **P1. The march panics on the crossing limit.** 10 shots, 3 seam aims, 88 mm AP and APHE_ring, spent speed. `kwk36_l56 88mm_AP` born at (13.448209, 5.9313054, -3.65905), post-step velocity (-114.379295, -136.64839, -74.76734): `a finite step fits its crossing limit; DrainExhausted { crossings: 4097 }`. A panic is process-fatal in every build (ADR-0011), the server's per-round path included. Live-match reach not traced.
- **P1. The march panics on shell membership.** 14 shots, 7.9 mm only, aims across full plate, thin plate, edge, gap and wedge; about 0.2 % of 7.9 mm flights (DERIVED from about 5900 flights). `mg34 7.9mm_AP` born at (13.480293, 3.381734, -6.838281), post-step velocity (-300.5629, 434.80978, 539.0987): `certified closed shells alternate from established membership; UnexpectedExit`. Also `IncompleteCorridor`, born at (10.371233, 4.795458, -6.657592), post-step velocity (195.4084, -728.5137, -33.291897). Some aims come from under the hull, which the absent ground would stop in play; others do not. Live-match reach not traced; a machine gun fires many of these.
- **P2. A disc round stops dead on its second read.** The premise above. `kwk36_l56 88mm_AP` at muzzle speed born at (4.9817486, 0.5446081, -13.172495), post-step velocity (518.10205, 263.33224, 509.6621). It loses all its speed, charged whole as the skid's shock.
- **P2. Chains of hundreds of exchanges on one shot.** `kwk36_l56 88mm_APCR` at spent speed born at (9.582715, 7.050139, -10.259464), post-step velocity (136.28441, -138.93864, 127.19618): 851 exchanges. DERIVED cost at T9's read: 851 × 65–381 µs is 55–324 ms for one shot on a dev build.
- **P3. HE's piece rests at once.** 5–18 % of the HE arm's first noted exchanges are a zero-turn rest on the surface it was born on (T1c). DERIVED cause above, not traced.

## What was not measured, and why

- **Both walkers.** Only today's exists. §8's missing hits, fresh discs in unowned material, dose moved, and the 0.1° comparison on full and thin plate need a rebuilt walker, the design's own lane 1 shadow.
- **A panic's lead-up.** A panic poisons the range, so its exchanges die with it. A fix lane should trace each.
- **Live-match reach.** Shots are placed one tick before their aim, not fired from a muzzle at range; ground is absent, so aims from under the hull are geometry only.
- **Rendered pose and marks on screen.** No captures in this lane.
- **Recorder, replicas, timeline, fuze clock.** Lanes 6–10's subject.
- **Concave primitive crossed twice.** Not isolated as a class. Curved faces are faceted convex surfaces only, read as their own class.
- **Pairs and phases on some classes.** Moving target has no aim pairs or phase series; inside face has no phase series, since it starts on its surface.
- **Short cells.** Up to 24 aims per cell; the armour offers fewer of some (T0): no full plate at 85° for an 88 mm disc, and seam 15–53 per calibre. The moving and unrelated arms fly one round per calibre.

A class needing an inline fixture: none. The hull offers all ten. Its mix among uniform aims is in T0.
