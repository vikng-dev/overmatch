# Preset tiers: what PC each graphics preset promises 120 fps on

Research lane, read 2026-10-30. No repo edits, builds, runs or cloud launches.

**Labels**
- **MEASURED**: a number a cited source measured (benchmark fps or score, Valve survey share).
- **DERIVED**: arithmetic on labelled inputs.
- **ASSUMED**: a choice or estimate made here.
- **CLAIMED**: a vendor or reviewer assertion, with no number behind it that this lane read.

**Access.** `store.steampowered.com` resets every connection from this machine. Steam pages were read as raw Internet Archive captures (`web.archive.org/web/<ts>id_/…`). TechPowerUp's GPU database returns a bot wall and is not archived, so Tom's Hardware is the benchmark aggregate. Notebookcheck GPU pages loaded; its comparison pages did not.

---

## Recommendation

| Preset | Reference GPU (raster index, RTX 4060 = 1.00) | Reference CPU (cores; PassMark single-thread) | RAM / VRAM | Output resolution | Windows Steam users with this GPU class or faster |
|---|---|---|---|---|---|
| **LOW** | **GeForce GTX 1650 4 GB** (0.32) | **Ryzen 5 3600** or Core i5-10400F (6C/12T; 2,557 / 2,540) | 16 GB / 4 GB | **1920×1080** | **79–81 %** |
| **MEDIUM** | **GeForce RTX 4060 8 GB** (1.00) | **Ryzen 5 5600** or Core i5-12400F (6C/12T; 3,254 / 3,484) | 16 GB / 8 GB | **1920×1080**, or 2560×1440 at 75 % render scale (the same pixel count) | **49–54 %** |
| **HIGH** | **GeForce RTX 5070 12 GB** (1.85) | **Ryzen 7 7700** (8C/16T; 4,050) | 32 GB / 12 GB | **2560×1440** | **14–18 %** |
| ULTRA | **No promised class.** ULTRA may exist as an unpromised preset. | — | — | — | (≥ RTX 5070 Ti class: 8 %) |

How to read the table:
- **Indices** are DERIVED (§2).
- **PassMark** scores are MEASURED (PassMark).
- **Coverage** is DERIVED from MEASURED Valve shares: the low figure is August 2026, the high one September 2026.
  - The 8–9 % of cards Valve lumps into "Other" are counted as below every tier.
  - Counting only named cards gives LOW 86–89 %, MEDIUM 54–59 %, HIGH 15–20 %.
- **RAM and VRAM** are ASSUMED pairings, checked against the survey in §1.

**Decisions taken**
1. **LOW is the GTX 1650, not the GTX 1660 Super / RTX 3050 class.**
   - It covers about 10 points more users (79–81 % against 67–71 %, DERIVED).
   - It is World of Tanks' *recommended* GPU (CLAIMED, prior note §2).
   - The owner's M4 GPU benchmarks at about the same level (§5), so LOW can be checked GPU-side on the dev machine.
   - The cost: LOW must cost about a third of MEDIUM on the GPU at the same 1080p (§2), and fit in 4 GB of VRAM.
2. **MEDIUM is the RTX 4060.**
   - It is the median Steam GPU class.
   - It is the PC the 2026-10-29 note already plans to buy (`.agents/scratch/perf-2026-10-29/performance-target-research.md` §4).
   - The RTX 3060 (0.86) sits 14 % lower. About 15 % headroom on the 4060 would extend the promise to 3060 owners, worth about 6 more points of coverage (DERIVED).
3. **HIGH is the RTX 5070, not the RTX 4070.**
   - At 1440p a 4070 renders only 1.09× as fast as a 4060 at 1080p, so HIGH on a 4070 could cost barely more than MEDIUM.
   - The 5070 gives HIGH 1.35× MEDIUM's per-frame budget (DERIVED from Tom's, §2).
   - It is also the most common single GPU on Steam: 3.77 % in August, 6.15 % in September (MEASURED).
4. **ULTRA is not a promised hardware class.**
   - The ≥ RTX 5070 Ti cohort is 7.9–8.0 % and steady month to month (MEASURED/DERIVED).
   - At 1440p it is only 1.21× the HIGH reference.
   - At 4K, a 5070 Ti renders a frame at 0.96× the speed of a 4060 at 1080p (DERIVED), so a 4K ULTRA would have to cost less than MEDIUM.
   - A fourth promise would add a fourth test rig for about 8 % of users. Those users already beat HIGH's reference by about 17 % (2.17 / 1.85).
5. **The Steam Deck stays outside the 120 fps ladder.** Its panel tops out at 60 or 90 Hz (CLAIMED, prior note). The prior note's "Deck at 60 fps" remains available as a separate handheld promise.

**The owner's Mac (MacBook Pro 14", M4 10-core GPU, 16 GB)**

| Part | Places at | Evidence (§5) | Confidence |
|---|---|---|---|
| GPU | **LOW**: about a GTX 1650 (0.7–1.1×); 0.24–0.40× an RTX 4060 | Notebookcheck game fps and 3DMark | Medium for the tier; low for any exact ratio (cross-API, cross-architecture) |
| CPU, single thread | **Above HIGH**: about a Ryzen 7 9800X3D (PassMark 4,491 against 4,420) | PassMark | Medium as a synthetic; low as a predictor of game frame times |
| CPU, multi-thread | **MEDIUM**: 23,567, between the Ryzen 5 5600 (21,488) and 7600 (26,963) | PassMark | Medium |

**Dev-machine proxy rules (DERIVED, weak):**
- **CPU.** Multiply each thread's milliseconds on the M4 by 1.76 to predict LOW, 1.38 for MEDIUM and 1.11 for HIGH (PassMark single-thread ratios).
  - At 120 fps the per-thread budget on the M4 is therefore 4.7 / 6.0 / 7.5 ms.
  - LOW binds every tier, because presets move GPU cost far more than CPU cost (ASSUMED). The prior note's 6.0 ms rule was set against MEDIUM's machine; 4.7 ms replaces it.
- **GPU.** The M4 rendering LOW at 1080p-equivalent pixels stands in for the LOW promise within a factor of 0.7–1.1. The conservative reading: the M4's GPU frame stays ≤ 7.6 ms (8.33 / 1.1).
  - Metal exposes no GPU timestamps (prior note §3), so only the whole frame is observable.
  - The M4 is useless as a MEDIUM or HIGH GPU proxy: those references are 2.5–4× faster.

---

## 1. Steam Hardware & Software Survey, August and September 2026

September 2026 is the latest month: October's results publish in early November.

**September is skewed.** Simplified Chinese rose from 23.97 % to 31.39 % (+7.42 points, MEASURED). In the same month:
- 2560×1440 jumped 5.18 points;
- the RTX 5070 jumped 2.38 points;
- AMD CPUs at 3.7 GHz or more jumped 3.74 points.

Valve's month-to-month sample swings look like this. **August is the baseline here, and September the upper bound.**

**GPUs.** Source: Windows PCs, from the video card page. Columns are May to September 2026, MEASURED.

| Card | May | Jun | Jul | Aug | Sep |
|---|---|---|---|---|---|
| RTX 5070 | 3.09 | 3.29 | 3.62 | 3.77 | 6.15 |
| RTX 5060 | 2.58 | 2.81 | 3.03 | 3.16 | 4.40 |
| RTX 5060 Ti | 2.06 | 2.26 | 2.40 | 2.55 | 4.06 |
| RTX 4060 | 3.74 | 3.65 | 3.63 | 3.60 | 3.89 |
| RTX 3060 | 4.02 | 3.88 | 3.88 | 3.92 | 3.66 |
| RTX 4060 Laptop | 3.99 | 4.02 | 3.64 | 3.84 | 3.50 |
| RTX 3050 | 3.28 | 3.23 | 3.22 | 3.21 | 2.90 |
| RTX 4060 Ti | 2.45 | 2.41 | 2.36 | 2.37 | 2.85 |
| RTX 5060 Laptop | 1.83 | 2.48 | 2.19 | 2.66 | 2.45 |
| GTX 1650 (desktop and laptop are one string) | 2.70 | 2.63 | 2.61 | 2.49 | 2.19 |
| RTX 4070 | 1.81 | 1.76 | 1.78 | 1.78 | 2.11 |
| RTX 5070 Ti | 1.73 | 1.76 | 1.78 | 1.82 | 1.87 |
| GTX 1660 Super | 1.52 | 1.47 | 1.46 | 1.47 | 1.26 |
| "Other" | 8.58 | 8.55 | 8.72 | 8.51 | 8.33 |

- The page lists 96 named cards. The full list is in the capture.
- Named laptop dGPUs total about 14 % in September (DERIVED).
- The integrated parts (Iris Xe, "AMD Radeon Graphics", UHD) total about 10 % (DERIVED).

**CPUs** (Windows; MEASURED; Aug → Sep).
- **Vendor:** Intel 54.47 → 52.59 %; AMD 45.52 → 47.41 %. Valve publishes no CPU models.
- **Physical cores:**

| Cores | Aug | Sep |
|---|---|---|
| 2 | 2.79 | 2.36 |
| 4 | 11.83 | 10.37 |
| 6 | 28.23 | 27.84 |
| 8 | 27.95 | 30.05 |
| 10 | 7.09 | 7.81 |
| 12 | 5.08 | 4.73 |
| 14 | 5.04 | 5.36 |
| 16 | 6.10 | 5.88 |
| 20 | 2.25 | 2.17 |
| 24 | 3.08 | 2.88 |

- 6 cores or more: 85.1 → 87.0 %. 8 or more: 56.9 → 59.2 % (DERIVED).
- Counts of 10, 14, 20 and 24 are Intel hybrid parts (P+E cores), so a core count is not a performance tier.

**Primary display resolution** (combined platforms; MEASURED; Aug → Sep).

| Resolution | Aug | Sep |
|---|---|---|
| 1920×1080 | 50.52 | 47.91 |
| 2560×1440 | 21.86 | 27.04 |
| 2560×1600 | 5.71 | 5.22 |
| 3840×2160 | 4.98 | 4.79 |
| 3440×1440 | 3.14 | 2.91 |
| 1920×1200 | 2.76 | 2.56 |
| 1366×768 | 2.23 | 1.92 |

- 1440p class or larger: **36.6 → 40.8 %** (DERIVED: 2560×1440, 2560×1600, 3440×1440, 2880×1800, 3840×2160, 5120×1440).
- 4K or larger: 5.4 → 5.2 % (DERIVED).
- Refresh rate is not surveyed.

**RAM and VRAM** (combined; MEASURED, sums DERIVED; Aug → Sep).
- RAM ≥ 16 GB: 88.1 → 89.2 %. RAM ≥ 32 GB: 44.2 → 48.8 %.
- VRAM ≥ 8 GB: 77.7 → 79.6 %. VRAM of exactly 4 GB: 5.69 → 5.13 %.
- So LOW's 4 GB VRAM budget excludes almost no discrete card. MEDIUM's 8 GB matches about four in five users.

---

## 2. GPU tiers

**Index (DERIVED).** Each card gets one raster scale, RTX 4060 desktop = 1.00:
- **Current cards.** Tom's Hardware 2026 hierarchy: 1080p Ultra, geometric mean of 11 games on a Ryzen 7 9800X3D. Index = card fps / the 4060's 71.5 fps (MEASURED).
- **Older cards.** Tom's 2022–2024 suite: 8 games on a Core i9-12900K. Index = card fps / the 4060's 84.9 fps (MEASURED).
  - The two suites agree where they overlap: RTX 3060 at 0.86 against 0.85, RTX 3050 at 0.62 against 0.61 (DERIVED).
- **Laptop GPUs.** Notebookcheck's Cyberpunk 2077 1080p Ultra averages divided by the desktop 4060's 77 fps (MEASURED). Where no game data was read, Time Spy Graphics / 10,427 × 0.95 (DERIVED).
- **iGPUs and a few laptop parts:** ASSUMED.

| Tier | Reference (index) | Neighbours in the same class (index) | Reference share, Aug / Sep (MEASURED) | Share at or above, Aug / Sep (DERIVED) |
|---|---|---|---|---|
| LOW | GTX 1650 (0.32) | GTX 1060 6 GB 0.38, RX 570 0.33, GTX 1650 Super 0.40, RX 580 8 GB 0.42 | 2.49 / 2.19 % | 78.9 / 81.3 % |
| (alternative LOW) | GTX 1660 Super (0.55) | RTX 3050 0.62, RTX 2060 0.65, RX 6600 0.73 | 1.47 / 1.26 % | 67–71 % (at ≥ 0.60) |
| MEDIUM | RTX 4060 (1.00) | RTX 4060 Laptop 0.94, RTX 5050 0.97, RX 7600 0.98, RTX 3060 Ti 1.04, RTX 5060 1.24 | 3.60 / 3.89 % | 49.2 / 54.4 % (at ≥ 0.94) |
| HIGH | RTX 5070 (1.85) | RTX 4070 Super 1.77, RTX 4070 Ti 1.89, RX 9070 1.97 | 3.77 / 6.15 % | 13.8 / 16.2 % (15.3 / 18.0 % at ≥ 1.75) |
| enthusiast | RTX 5070 Ti (2.17) | RX 9070 XT 2.19, RTX 4080 2.20, RTX 5080 2.33, RTX 4090 2.57, RTX 5090 2.85 | 1.82 / 1.87 % | 7.9 / 8.0 % |

**Bands that hold the users** (Sep, DERIVED):
- Index 0.94–1.5 (the MEDIUM band) is the largest, at 31.3 %. Its biggest cards are the RTX 5060, 5060 Ti, 4060, 4060 Laptop, 4060 Ti and 5060 Laptop.
- Below the LOW reference, at index under 0.3, sit 10.3 % of users, mostly iGPUs. No preset promises them anything.

**What the references imply for preset cost** (DERIVED from Tom's MEASURED fps; GPU only):
- **LOW against MEDIUM, at 1080p:** the GTX 1650 runs at 0.32× the 4060 (ultra) and 0.36× (medium) in Tom's suite. Notebookcheck's Cyberpunk gives 0.34–0.35×. **LOW must cost about 1/3 of MEDIUM.**
- **HIGH at 1440p against MEDIUM at 1080p:** the RTX 5070 at 1440p (96.4 fps) against the 4060 at 1080p (71.5 fps) is **1.35×**, about 0.76× per pixel. With an RTX 4070 as the reference it would be 1.09×.
- **A hypothetical ULTRA on an RTX 5070 Ti:** 1.63× MEDIUM at 1440p, only 1.21× HIGH. At 4K it is 0.96×.
- **How far below AAA ultra each preset must sit:** the 4060 averages 71.5 fps at 1080p Ultra in Tom's 2026 suite, so MEDIUM must be about 1.7× cheaper than a 2026 AAA ultra frame. The 5070 averages 96.4 fps at 1440p Ultra, so HIGH must be about 1.25× cheaper.

---

## 3. Resolution per tier

Valve does not cross-tabulate GPU with display, so this is a **quantile match (DERIVED)**: rank both, align the percentiles.
- Perfect rank correlation is the upper bound on how far high-end GPUs sit on high-resolution displays.
- Zero correlation is the lower bound: every tier then shows the overall mix, about 37–41 % at 1440p or above.

| GPU band (Aug percentiles from the top) | Displays at those percentiles | Choice |
|---|---|---|
| Enthusiast, top 8 % | 4K and super-ultrawide (5.4 %), then 3440×1440 | No promise (§Recommendation) |
| HIGH, 8–14 % (to 16–18 % with the 4070 Super) | 3440×1440, 2560×1600, 2560×1440 | **2560×1440** |
| MEDIUM, 14–49 % | 1440p class down to the 36.6 % mark, then 1920×1080 | **1920×1080**; 1440p owners at 75 % render scale |
| LOW, 49–79 % | 1920×1080, plus 1366×768 laptops | **1920×1080** |

**The MEDIUM band is split.**
- Between 37 % (zero correlation) and 64 % (perfect correlation) of it sits at 1440p or above (DERIVED).
- 1080p stays the reference because it is the overall mode and the 4060 class's design point. Tom's calls the RTX 5060 and up enough for high-refresh 1080p, and the 5070 or RX 9070 enough for 1440p (CLAIMED).
- 2560×1440 at 75 % render scale shades exactly 1920×1080's pixel count. The game already has that rung, so one promise covers both groups for pixel-bound work (DERIVED). Bloom and tonemapping still run at full output resolution (prior note §5).

---

## 4. CPU pairing

Valve publishes core counts and vendors, not models. Pairings are ASSUMED as the typical CPU bought alongside each reference GPU, and checked against the core-count distribution (§1).

| Tier | CPU | Cores | PassMark ST / MT (MEASURED) | Survey fit |
|---|---|---|---|---|
| LOW | Ryzen 5 3600; or Core i5-10400F | 6C/12T | 2,557 / 17,646; 2,540 / 12,090 | 6 cores is the 28 % bucket; ≥ 6 cores covers 85–87 %. A 4-core i3-10100F has the same single-thread score (2,578). |
| MEDIUM | Ryzen 5 5600; or Core i5-12400F | 6C/12T | 3,254 / 21,488; 3,484 / 19,526 | Same 6-core bucket. Matches the prior note's AVERAGE PC. |
| HIGH | Ryzen 7 7700 | 8C/16T | 4,050 / 34,312 | 8 cores is the modal bucket (28–30 %); ≥ 8 covers 57–59 %. A non-X3D part is chosen as the conservative reference (the 7800X3D scores 3,759 ST yet games faster). |
| (enthusiast) | Ryzen 7 9800X3D | 8C/16T | 4,420 / 39,912 | Tom's GPU testbed |

**Why the CPU matters most at LOW.** Presets mainly scale GPU work: pixels, shadow resolution, MSAA. Sim, netcode and most of the main thread do not scale with them (ASSUMED; shadow cascades and LOD do touch the render thread). So the LOW CPU sets the CPU budget for every promise. Its single-thread score is 0.79× MEDIUM's (DERIVED).

---

## 5. The owner's M4 against the tiers

**GPU (10-core, Metal).** Notebookcheck figures are MEASURED; ratios are DERIVED.

| Test | M4 | Comparison | M4 ratio |
|---|---|---|---|
| Cyberpunk 2077 1080p low / med / high / ultra (native macOS port on the M4; Windows 2.x on the PCs) | 34 / 27 / 23 / 20 fps (MBP 14: 34.8 / 27.9 / 23.7 / 20.4) | GTX 1650 desktop: 50.4 / 38.9 / 31.8 / 26.3. RTX 4060: 144 / 109 / 91 / 77 | 0.67–0.76× GTX 1650; 0.24–0.26× RTX 4060 |
| Baldur's Gate 3, 1080p low → ultra | 44 / 36 / 32 / 30 | RTX 4060: 141 / 112 / 99 / 97 | 0.31–0.32× RTX 4060 |
| Total War Pharaoh, 1080p high / ultra | 45 / 39 | RTX 4060: 158 / 123 | 0.28–0.32× |
| 3DMark Steel Nomad Light | 3,787 avg (MBP 14: 3,932) | RTX 4060 Laptop 9,584. GTX 1650 Ti Mobile 3,477, which is ≈ a desktop GTX 1650 by Time Spy Graphics (3,689 against 3,541) | 0.40× 4060 Laptop, ≈ 0.38× 4060; ≈ 1.13× GTX 1650 |
| 3DMark Wild Life Extreme Unlimited | 9,140 avg (MBP 14: 9,628) | RTX 4060 Laptop 17,749; RTX 3050 6 GB Laptop 8,916 | 0.51× 4060 Laptop |
| Notebookcheck's own text | "almost as good as the GeForce GTX 1650 Laptop" | | CLAIMED |

**Reading.** Games put the M4 at 0.25–0.32× an RTX 4060, synthetics at 0.38–0.51×. A GTX 1650 is 0.32–0.36×. So **the M4 GPU sits at the LOW reference: 0.7–1.1× a GTX 1650.**
- Mobile-born tests (Wild Life) flatter the tile-based GPU.
- Ported games handicap it: API translation, and Metal encodes indirect draws on the CPU (prior note §3).
- Both pull in opposite directions. Confidence is medium for the tier, low for the factor.

**CPU (4P + 6E).** PassMark figures are MEASURED; ratios are DERIVED.
- **Single thread: 4,491.** About the Ryzen 7 9800X3D (4,420) and Core i7-14700K (4,453).
  - Ratios: 1.76× the LOW reference (Ryzen 5 3600), 1.38× MEDIUM (Ryzen 5 5600), 1.11× HIGH (Ryzen 7 7700).
  - A secondary aggregate (CPU-Monkey, read only as a search excerpt) gives Cinebench 2024 single-core at 175 for the M4 against 115 for the Ryzen 5 7600X (CLAIMED). That points the same way.
- **Multi-thread: 23,567.** 1.10× the Ryzen 5 5600, 0.87× the Ryzen 5 7600, 0.69× the Ryzen 7 7700.
- **Caveat.** Single-thread synthetics miss what games reward: cache size and memory latency. The X3D parts show it, with a lower PassMark single-thread score yet higher game fps. Bevy also spreads systems over all cores, 4P + 6E on the M4 against 6C/12T on the references. Confidence is low for game-frame prediction.

**Net placement:** GPU **LOW**; CPU **HIGH-plus** per thread and **MEDIUM** in aggregate. The machine flips the limiting stage relative to every PC tier, which the prior note also found.

---

## 6. Cloud stand-ins (AWS; nothing launched)

**What AWS documents (CLAIMED, AWS docs):**
- The `*.xlarge` sizes have **2 physical cores** (4 vCPUs). That is below every tier's core count, so a run needs `*.4xlarge` (8 cores) to avoid starving Bevy's task pool.
- NVIDIA gaming drivers (WDDM, DirectX, Vulkan, one virtual 4K display) are supported on G4dn, G5, G6, G6e, G7 and G7e, not on G6f or Gr6.

| Tier | Instance | GPU facts (CLAIMED: NVIDIA, AWS) | Raster equivalence (DERIVED unless noted) | CPU single-thread against the tier's CPU (PassMark, DERIVED) |
|---|---|---|---|---|
| LOW | g4dn.4xlarge, T4 | TU104, 2,560 CUDA cores, 70 W, 320+ GB/s, 8.1 TFLOPS FP32 | ≈ GTX 1650 Super–1660, index 0.4–0.5: slightly *above* the LOW reference. PassMark G3D 9,730 against GTX 1650 7,865 and GTX 1060 10,015 (MEASURED) | Xeon P-8259L, read as PassMark's 8259CL (ASSUMED same part), 1,948: **0.76×** |
| MEDIUM | g6.4xlarge, L4 | AD104, 72 W, 300 GB/s, 30.3 TFLOPS FP32 | 0.55–1.2× an RTX 4060, the widest spread here. The bandwidth matches the 4060 class, but the 72 W cap throttles a 58-SM die. G3D 10,821, measured in a VM (MEASURED, low confidence) | EPYC 7R13, 2,606: **0.80×** |
| HIGH | g5.4xlarge, A10G | GA102, "80 ray tracing cores" (so 80 SMs, DERIVED), 24 GB. "Up to 3x higher graphics performance" than G4dn (CLAIMED) | ≈ RTX 3070 Ti–3080, index 1.3–1.56: *below* the 5070 reference. G3D 13,598–17,660 (MEASURED in VMs, low confidence) | EPYC 7R32, 2,004: **0.49×** |
| enthusiast | g6e.4xlarge, L40S | 18,176 CUDA cores, 350 W, 864 GB/s, 91.6 TFLOPS | ≈ RTX 4080 Super–4090, index 2.2–2.5 | EPYC 7R13: **0.59×** against the 9800X3D |
| (newer, unverified) | g7, RTX PRO 4500 Blackwell; Granite Rapids CPU | AWS lists the part, 32 GB | The desktop part scores G3D 33,318 (≈ RTX 4080), the server edition in a VM 17,580 (MEASURED). Specs not verified. | not measured |

**Caveats that apply to every row:**
- **The CPUs are server parts.** Each is 0.5–0.8× its tier's single-thread score, so frames go CPU-bound earlier than on the reference PC.
- **The GPUs are power-capped data-center parts.** Paper TFLOPS mislead: the L4 has twice the 4060's FP32.
- **Presentation is virtual.** The display, its refresh rate and the frame pacing are not the reference PC's. Tails do not transfer.
- **Use: same-instance A/B throughput deltas only.** Calibrate any absolute equivalence by running one build on the instance and on a real reference PC.
- The fractional `g6f` slices (⅛–½ L4, GRID only) are time-sliced. They do not model a slower GPU.
- **Owner sequencing:** launches stay parked until the seeded-replay instrument exists (owner memory `aws-benchmarking-account`).
- GCP G2 (L4) and Azure NVads A10 v5 (A10) carry the same chips (ASSUMED; not verified this pass).

---

## 7. Against the 2026-10-29 note

`.agents/scratch/perf-2026-10-29/performance-target-research.md` defined three machines, each with its own target:
- **LOW**: the Steam Deck at 60 fps;
- **AVERAGE**: RTX 4060 + Ryzen 5 5600 running the LOW preset at 1080p and 120 fps;
- **DEV**: the M4, with a 6.0 ms per-thread proxy rule.

**What changes under a preset ladder:**
1. **The AVERAGE machine becomes the MEDIUM reference.** It is the same PC, so the planned purchase still holds. The preset it promises moves from LOW to MEDIUM.
2. **A new LOW reference** (GTX 1650 + Ryzen 5 3600) carries the promise for the preset now called LOW. It tightens the M4 CPU proxy from 6.0 ms to 4.7 ms per thread (DERIVED).
3. **The Deck leaves the 120 fps ladder** (Decision 5).
4. **Coverage figures agree within about 5 points.** That note used the combined page with RTX 3060 = 100; this one uses the Windows video card page with RTX 4060 = 1.00 and different laptop indices. Its "≥ RTX 4060: 45–49 %" corresponds to 44–50 % here at ≥ 1.00, and 49–54 % at ≥ 0.94.

---

## Sources (all read 2026-10-30)

**Steam Hardware & Software Survey** (Valve), via Internet Archive raw captures:
- September 2026, main page: https://web.archive.org/web/20261022045548id_/https://store.steampowered.com/hwsurvey/Steam-Hardware-Software-Survey-Welcome-to-Steam
- August 2026, main page: https://web.archive.org/web/20260920112146id_/https://store.steampowered.com/hwsurvey/Steam-Hardware-Software-Survey-Welcome-to-Steam
- Video cards, May–Sep 2026: https://web.archive.org/web/20261026155202id_/https://store.steampowered.com/hwsurvey/videocard/
- Physical CPUs, May–Sep 2026: https://web.archive.org/web/20261022145203id_/https://store.steampowered.com/hwsurvey/cpus/
- Processor vendor, May–Sep 2026: https://web.archive.org/web/20261005081709id_/https://store.steampowered.com/hwsurvey/processormfg/

**Tom's Hardware GPU Benchmarks Hierarchy**, updated 24 June 2026:
- 2026 suite: https://www.tomshardware.com/reviews/gpu-hierarchy,4388.html
- 2022–2024 suite: https://www.tomshardware.com/reviews/gpu-hierarchy,4388-2.html

**Notebookcheck**:
- M4 10-core GPU: https://www.notebookcheck.net/Apple-M4-10-core-GPU-Benchmarks-and-Specs.835807.0.html
- RTX 4060: https://www.notebookcheck.net/NVIDIA-GeForce-RTX-4060-GPU-Benchmarks-and-Specs.741899.0.html
- RTX 4060 Laptop: https://www.notebookcheck.net/NVIDIA-GeForce-RTX-4060-Laptop-GPU-Benchmarks-and-Specs.675692.0.html
- RTX 5060 Laptop: https://www.notebookcheck.net/Nvidia-GeForce-RTX-5060-Laptop-Benchmarks-and-Specs.934941.0.html
- RTX 4050 Laptop: https://www.notebookcheck.net/NVIDIA-GeForce-RTX-4050-Laptop-GPU-Benchmarks-and-Specs.675695.0.html
- GTX 1650 desktop: https://www.notebookcheck.net/NVIDIA-GeForce-GTX-1650-Desktop-GPU-Benchmarks-and-Specs.421420.0.html

**PassMark**:
- CPU single-thread: https://www.cpubenchmark.net/singleThread.html
- CPU multi-thread: https://www.cpubenchmark.net/high_end_cpus.html , https://www.cpubenchmark.net/mid_range_cpus.html
- GPU G3D: https://www.videocardbenchmark.net/high_end_gpus.html , https://www.videocardbenchmark.net/mid_range_gpus.html , https://www.videocardbenchmark.net/low_end_gpus.html

**CPU-Monkey** (secondary, search excerpt only, not opened): https://www.cpu-monkey.com/en/compare_cpu-amd_ryzen_5_7600x-vs-apple_m4_10_cpu

**AWS**:
- Accelerated computing instance specs: https://docs.aws.amazon.com/ec2/latest/instancetypes/ac.html
- NVIDIA drivers by instance type: https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/install-nvidia-driver.html
- G5 product page: https://aws.amazon.com/ec2/instance-types/g5/

**NVIDIA**:
- T4: https://www.nvidia.com/en-us/data-center/tesla-t4/
- L4: https://www.nvidia.com/en-us/data-center/l4/
- L40S: https://www.nvidia.com/en-us/data-center/l40s/

**Repo:** `.agents/scratch/perf-2026-10-29/performance-target-research.md` (WoT recommended specs, Deck refresh rates, the Metal indirect-draw and GPU-timestamp facts, the rig plan).
