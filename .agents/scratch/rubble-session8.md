Subject: the video game's environment destruction model; game state only. Measurement only; landing gate remains unmet.
Merge: “construction: merge full tick attribution for rubble measurements (#305)”; all temporary probes removed.
MEASURED local timings/counts below; p99 and overruns show contact window / complete capture. Energies, bounds and shares below are DERIVED.

| Run | Worst ms | p99 ms, contact/all | Overruns, contact/all | Maximum sweep ms |
|---|---:|---:|---:|---:|
| Main A | 21.865 | 19.396 / 10.886 | 5 / 5 | 0.899 |
| Main B | 32.229 | 20.654 / 7.787 | 4 / 5 | 1.116 |
| Lane A | 57.230 | 25.370 / 13.068 | 11 / 11 | 11.280 |
| Lane B | 33.329 | 17.515 / 10.684 | 4 / 5 | 14.587 |

Sweep: MEASURED dominant A/B bodies: 1760/1592 casts, 5.489/5.919 ms, speed 1.113/0.777 m/s, spin 51.60/40.02 rad/s; DERIVED kinetic energy 320.47/191.71 J against weakest intact-bond work bounds 0.09509/0.01997 J. DERIVED below-bound cast-time share: 0% in both. These pulse-work bounds do not certify an energy-only skip under the force-controlled sweep law.
Outliers: MEASURED historical A/B overruns 185/4; the extra 181 are late, with zero casts; DERIVED period CPU means 1.40/1.07 cores. Their exact historical work site is unproven. DERIVED current driving share of late FixedUpdate: 95.5%. MEASURED main B's pre-contact spike: 23.398 ms wall / 5.032 ms CPU, hence DERIVED at least 18.366 ms off CPU; descheduling versus blocking, and the historical 58 ms tick, remain unclassified.
Decision: pursue rotating-body query cost and driving evaluation, not an energy cutoff; local observations do not settle realism or hosted capacity (#306). [Every over-budget tick's full stages, body motions/energies/work bounds and methods](rubble-session8-measurements.json). Formatting, CI clippy and MEASURED 68 targeted/guard tests pass; MEASURED full library suite 2495 passed, 41 ignored, zero failed; 30 reader cases passed.
