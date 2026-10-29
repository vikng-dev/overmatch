# Performance target: what "120 fps on an average computer at low settings" means (issue #151)

Research lane, 2026-10-29. No code was built or run. Labels:
- **MEASURED**: timed by this repo's harnesses, or read directly by this lane (a file, a source line, a price list);
- **DERIVED**: arithmetic on labelled inputs;
- **ASSUMED**: a choice or estimate made here, with no source;
- **CLAIMED**: published by someone else (a vendor, Valve, a benchmark site), not checked here.

Access note: `store.steampowered.com` and `steamdeck.com` reset every connection from this machine. Every Steam page cited here was read as the raw original HTML of an Internet Archive capture (`web.archive.org/web/<timestamp>id_/…`). The capture dates are in Sources.

---

## Summary

**Reference machines (proposed)**

| Role | Machine | Why | Display | Target, at LOW |
|---|---|---|---|---|
| LOW | Steam Deck (Linux, SteamOS): Zen 2 4c/8t, 8 RDNA 2 CUs at 1.6 GHz, 16 GB LPDDR5 | Its GPU is the median Linux Steam GPU, about 20 % of Linux users. It sits near the 10th–20th percentile of all Steam GPUs (DERIVED, §1). | 1280×800, 60 Hz (LCD) or 90 Hz (OLED) | **60 fps**, p50 ≤ 16.7 ms (ASSUMED) |
| AVERAGE | Windows 11 desktop: 6-core Zen 3 (Ryzen 5 5600 class), RTX 4060 8 GB, 16 GB dual-channel RAM | The RTX 4060 class is the median Steam GPU (DERIVED, §1). The median is 8 physical cores and 16–32 GB of RAM. Named NVIDIA cards are 71.75 % of all Steam GPUs. | 1920×1080 (the median, 48 %), 144 Hz (ASSUMED: Steam publishes no refresh rate) | **120 fps**: p50 ≤ 8.33 ms, p99 ≤ 16.7 ms |
| DEV | MacBook Pro 14" M4 (Mac16,1): 4P+6E CPU, 10-core GPU, 16 GB, 120 Hz | The machine every number so far was taken on (MEASURED). It is also the median Mac GPU (DERIVED, §1). | The harness surface, 2560×1440 physical, at RENDER SCALE 75 %. That shades 1920×1080 worth of pixels, the AVERAGE count (DERIVED). | 120 fps as a proxy gate (§3) |

**Scene.** The **battle scene**, on the Kalinovo map:
- 30 tanks, moving and firing on a script, at mixed ranges;
- the third-person camera at the forest edge, with about 48 trunks within 150 m;
- track prints on and a few felled trees;
- the in-battle HUD, with the debug card off;
- one online client against a dedicated server on another machine.

60 s are measured after a warmup. The pen cam corner (the game's damage-model feedback inset) gets its own transient budget. This scene does not exist yet: today's 30-tank lever spawns idle probes (§5).

**Target.** On AVERAGE at 1920×1080 LOW, the battle scene runs at p50 ≤ 8.33 ms (120 fps) and p99 ≤ 16.7 ms. Bevy pipelines rendering, so the frame is roughly the slowest of three stages: the main thread, the render thread and the GPU. **The budget is therefore three 8.33 ms columns, not one sum** (§5).

**Rig.**
- **Buy:** one Windows 11 desktop matching AVERAGE, with a 1080p 144 Hz monitor and a Linux dual-boot. Also a Steam Deck: a refurbished LCD model if Valve still lists one, otherwise the OLED at $789 (CLAIMED).
- **Do not rent yet.** AWS g6.xlarge (NVIDIA L4) costs $0.99/h on Windows (CLAIMED, AWS price list). It has 4 vCPUs and a virtual display, so it can produce A/B throughput deltas, never absolute frame times or pacing. Owner sequencing also parks instance launches (§4).

**Headlines**
1. **120 fps at 1080p low on an RTX 4060 is more than any comparable game publishes.**
   - Battlefield 6 promises "1080p at 80FPS+ (Low Settings)" on an RTX 3060 Ti with a Ryzen 7 3700X (CLAIMED, EA). That GPU is slightly faster than ours (DERIVED index 121 against 116, §1).
   - War Thunder, World of Tanks and CS2 publish no frame rate at all.
   - Valorant's 144 fps tier runs on a GTX 1050 Ti, but its scenes are small arenas, not 1–4 km² maps with 30 tanks (§2).
2. **Nothing has been measured on x86, Windows or Linux, and nothing at 30 tanks on current main.**
   - The last 30-tank frame on the M4 was 14.3–14.6 ms p50 at 1440p (2026-08-13, MEASURED): about 69 fps on the dev machine.
   - Since then the 2-tank duel's frame fell from 11.83 ms to 4.64 ms. No run isolates why (§5).
3. **The M4 translates weakly, and in opposite directions.**
   - GPU work: the AVERAGE machine at 1080p should do it in about 0.2–0.3× the M4's 1440p time.
   - CPU work: AVERAGE should take about 1.3–1.4× the M4's time (DERIVED, §3).
   - So the limiting stage probably flips from the GPU on the M4 to the CPU on the PC. This is untested.
   - Proxy rule for the dev machine: each CPU thread stays ≤ 6.0 ms p50 in the battle scene on the M4 (DERIVED, weak).
4. **Measured items already over their draft allocation on AVERAGE:**
   - the track wrap: 30 × 56 µs = 1.7 ms per frame on the M4, which DERIVES to 2.2–2.3 ms;
   - the segmented forest: +1.2 ms per sim tick;
   - the pen cam corner: +4.3 ms p50 on the M4 while it shows.
5. **LOW may only cut cost that nobody reads as gameplay.**
   - It may cut: cascades 3→2 (the one large lever measured so far, −1.16 ms each on the M4), shadow map size, MSAA, track-print relief, the LOD pixel budget, and effect resolution.
   - It may never cut: shadows within 350 m, vegetation, smoke and dust, tracers, or tank draw distance.
   - Overmatch has no spotting mechanic, so the rendered picture *is* the concealment model (§6).

---

## 1. What "average computer" means

Source: the Steam Hardware & Software Survey, September 2026, combined platforms (CLAIMED by Valve). The Linux-only and Mac-only views are from August 2026, the latest captured.

**Platform split (Sept 2026, CLAIMED):** Windows 95.03 %, Linux 3.05 %, macOS 1.92 %.

**CPU, RAM and display (Sept 2026, combined)**

| Item | Distribution (CLAIMED) | Median and mode (DERIVED) |
|---|---|---|
| Physical CPUs (cores) | 4: 10.79 %; 6: 27.23 %; 8: 30.06 %; 10: 8.19 %; 12: 4.78 %; 14: 5.23 %; 16: 5.78 % | Median **8** (6 or fewer: 40.72 %; 8 or fewer: 70.78 %). Mode 8. |
| CPU vendor (Windows) | AMD 47.41 % | No model data is published, only core counts and Intel clock buckets |
| System RAM | 8 GB: 6.47 %; 16 GB: 37.82 %; 32 GB: 42.22 %; 64 GB: 3.71 % | Median at the 16/32 boundary (16 GB or less: 47.58 %). Mode 32 GB. |
| VRAM | 8 GB: 26.71 %; 12 GB: 13.06 %; 16 GB: 27.21 % | Median **12 GB** (8 GB or less: 45.89 %) |
| Primary display | 1920×1080: 47.91 %; 2560×1440: 27.04 %; 2560×1600: 5.22 %; 3840×2160: 4.79 % | Median **1920×1080** (below 1080p: 5.04 %) |
| Refresh rate | **not surveyed.** The item list has no refresh entry (MEASURED read of the page). | UNKNOWN. The AVERAGE rig's 144 Hz is ASSUMED. |

**GPU tier (DERIVED).** Valve publishes cards, not tiers. To rank them, each named card gets a raster index with RTX 3060 = 100:
- Tom's Hardware 2026 hierarchy, 1080p Ultra geomean of 11 games (CLAIMED), divided by the RTX 3060's 30.2 %;
- older cards bridged from Tom's 2020–21 legacy suite with a factor of 0.54. That is the low end of the 0.52–0.59 ratio across 10 cards present in both suites (DERIVED);
- laptop GPUs, iGPUs and handhelds, 25 % of the named share, given ASSUMED indices: RTX 4060 Laptop 100, Iris Xe 20, Radeon 780M 40, Steam Deck 30.

| Percentile of Steam GPUs (combined, Sept 2026) | Named cards only | "Other" (8.82 %) counted at the bottom |
|---|---|---|
| p10 | GTX 1050 Ti (index 29) | Intel UHD 620 |
| p25 | GTX 1660 Super (68) | GTX 1060 (47) |
| **p50** | **RTX 4070 Laptop / RTX 3060–4060 band (115)** | **RTX 2070 Super (107)** |
| p75 | RTX 5060 Ti (167) | RTX 5060 Ti |

- **The median sits between the RTX 3060 (100) and the RTX 4060 (116)** (DERIVED).
  - At or above RTX 3060: 58–64 % of GPUs.
  - At or above RTX 4060: 45–49 %.
  - At or above the Steam Deck: 81–89 %.
- The most common band is RTX 3060–4060 (index 95–130): 20.8 % of all GPUs. Next is RTX 4060 Ti–4070 (130–200): 20.3 % (DERIVED).
- The most common single card is the RTX 5070 at 5.86 %. It jumped 2.30 points in one month, so read it with Valve's month-to-month noise in mind (CLAIMED).
- Vendors, as shares of all Steam GPUs, from the named cards: NVIDIA 71.75 %, AMD 12.38 %, Intel 4.80 %, Apple 1.74 %; "Other" (8.82 %) is unattributed. Named laptop parts are at least 13.21 % (DERIVED).

**Linux (Aug 2026, CLAIMED):**
- Steam Deck GPU 10.48 % plus "AMD Custom GPU 0405" (the Deck LCD's APU) 9.28 %: **19.76 % of Linux users on a Steam Deck**.
- Displays: 1920×1080 35.61 %, Deck panels 17.74 % (800×1280 12.49 %, 1280×800 5.25 %).
- Most common core count: 4 cores, 32.29 %.
- Median Linux GPU: the **Steam Deck** (DERIVED, with ASSUMED indices for the many iGPU strings, such as "AMD Raphael", that may hide a discrete card).

**macOS (Aug 2026, CLAIMED):** M4 18.24 %, M1 14.99 %, M2 10.72 %, M5 9.85 %, M3 5.94 %, A18 Pro 4.03 %. RAM is 16 GB for 43.41 % and 8 GB for 29.44 %. Ordered by Geekbench 7 Metal score, the median Apple Silicon GPU is the **M4** (DERIVED): M1 + A18 Pro + M2 + M3 reach 35.7 % of 88.6 % Apple Silicon. Intel Macs are not a shipping target.

**Chosen reference machines.** The summary's table holds the choices. The reasoning:
- **AVERAGE** takes the median GPU class. The RTX 4060 itself is chosen because it is current, has a $299.99 MSRP (CLAIMED, Tom's) and is Geekbench 7's calibration card.
- Its CPU is a 6-core Zen 3 rather than the median 8 cores. This is deliberate: the CPU is probably the limiting stage (§3), so the reference errs low. Steam publishes no CPU models, so this is the least-evidenced choice in this note (ASSUMED).
- Valve's Steam Machine (Zen 4 6c/12t, 28 RDNA 3 CUs, 16 GB DDR5 + 8 GB GDDR6, $1,049, CLAIMED by press coverage) is the same tier. Its Geekbench 7 Vulkan score of 33,371 is close to the RX 7600's 34,636 (CLAIMED), and the RX 7600 rasters like an RTX 4060 (CLAIMED, Tom's). It is a credible *Linux* AVERAGE; Windows NVIDIA stays primary.

---

## 2. How comparable games set targets

All from first-party pages (CLAIMED).

| Game | Minimum | Recommended | Frame rate promised |
|---|---|---|---|
| War Thunder (Windows) | Win 10, dual-core 2.2 GHz, 4 GB, GTX 660 / Radeon 77XX; "minimum supported resolution … 720p" | Core i5 or Ryzen 5 3600, 16 GB, GTX 1060 / RX 570 | **None** |
| World of Tanks (from 2.3.1) | i5-3330 / FX-6300, GTX 660 / R9 270 "(1280×768 screen resolution)", 8 GB | i3-9100F / Ryzen 3 3100, GTX 1650 / RX 580, 16 GB | **None** |
| Counter-Strike 2 | Win 10, "4 hardware CPU threads – Intel Core i5 750 or higher", 8 GB, 1 GB DX11 SM 5.0 | none listed | **None** |
| Valorant | i3-540 / Athlon 200GE, HD 4000 / R5 220, 4 GB: "(30 FPS)" | i3-4150 / Ryzen 3 1200, GT 730 / R7 240: "(60 FPS)" | High-end: i5-9400F / R5 2600X, GTX 1050 Ti / R7 370 / Arc A310: **"(144 FPS)"** |
| Battlefield 6 | i5-8400 / R5 2600, RTX 2060 / RX 5600 XT / Arc A380 (6 GB), 16 GB: **"1080p at 30FPS Low Settings"** | i7-10700 / R7 3700X, RTX 3060 Ti / RX 6700 XT / Arc B580 (8 GB), 16 GB: **"1080p at 80FPS+ (Low Settings)"** or "1440p at 60FPS (High Settings)" | as listed |

**Reading (DERIVED).**
- The two tank games publish floors, not frame rates.
- The large-scale shooter that does publish numbers (BF6) asks a slightly faster GPU than our AVERAGE for 80+ fps at 1080p low:
  - GPU index 121 against 116;
  - CPU single-thread 2,654 against 3,254 (CLAIMED, PassMark), so a slower CPU than ours.
- 120 fps on AVERAGE is therefore an ambitious target, about 1.5× what the closest published comparable promises. Valorant's 144 fps on a GTX 1050 Ti shows that tiny scenes allow it. It says nothing about 30 tanks on a km² map.

**What their low settings turn down**
- **War Thunder**, as read in `.agents/scratch/war-thunder-research-2026-10-26/public-sources.md` (Gaijin primary sources):
  - "Objects … such as trees or houses, now have a similar rendering distance for all graphics presets" (2021).
  - Ultra-low shadows are kept, but only "in a certain radius around you".
  - A separate "Old Videocard Support (Ultra Low)" render path drops whole techniques.
  - There are texture tiers by resolution and an upscaler menu.
- **World of Tanks** (Update 1.0 graphics guide):
  - Settings, ordered by cost: antialiasing, texture quality, object quality (the LOD distance), and draw distance, which "affects only objects not vital to gameplay".
  - Also lighting and post-processing, terrain tessellation, vegetation (grass amount and detail, wind, and grass transparency in sniper mode), and effects.
  - "On Medium and lower settings, the tracks of the tank are drawn in a simplified form."
  - "The camouflage mechanics work the same regardless of what settings you choose."
  - A less demanding preset can raise performance by 30–40 %.
- **Valorant, CS2 and BF6** publish no per-setting breakdown on these pages.

**The difference that matters here.** WoT can let settings vary because spotting is decided by the server: what a client draws does not decide who is spotted. Overmatch has "no abstract spotting mechanic" (`.agents/PRODUCT.md`), so what the client draws *is* the concealment model. WoT's own grass-transparency-in-sniper-mode setting is exactly the kind of option this game cannot ship (§6).

---

## 3. How the dev machine's numbers translate

| | DEV: M4 (MBP 14") | AVERAGE: RTX 4060 + Ryzen 5 5600 | LOW: Steam Deck | Label |
|---|---|---|---|---|
| GPU, Geekbench 7 (RTX 4060 = 100,000) | 51,944 (Metal) | 100,000 baseline (100,851 on the Vulkan chart) | 16,037–19,458 (Vulkan, RADV VANGOGH) | CLAIMED |
| GPU, 3DMark Steel Nomad Light | 3,839 median (range 3,583–3,989) | RTX 4060 *Laptop*: 9,584 median. No desktop figure found. | not found | CLAIMED (Notebookcheck, read via search excerpts: the pages return 403) |
| GPU FP32 | not published by Apple | 15 TFLOPS | 1.6 TFLOPS | CLAIMED |
| Memory bandwidth | 120 GB/s, shared | 272 GB/s: 128-bit × 17 Gbps. The 128-bit bus is CLAIMED; the 17 Gbps is ASSUMED. | 102.4 GB/s shared (OLED: 6400 MT/s, quad 32-bit) | CLAIMED / DERIVED |
| CPU single-thread, PassMark | 4,491 | 3,254 (Ryzen 5 5600); 3,484 (i5-12400F) | 2,201 (OLED APU 0932) | CLAIMED |
| CPU multithread, PassMark | 23,567 | 19,527 (i5-12400F); 24,871 (i5-13400F) | 9,342 | CLAIMED |
| Pixels at the reference surface | 3.69 MP (2560×1440) | 2.07 MP (1080p) | 1.02 MP (800p) | DERIVED |
| Indirect draws | Metal *loops* each indirect draw on the CPU, and indirect-count is an empty TODO | DX12 and Vulkan native | Vulkan native | MEASURED read of `wgpu-hal-29.0.4/src/metal/command.rs:1574–1590, 1627–1636` |
| GPU timestamps | none: `gpu_ms` is null (`src/render_cost.rs`) | real | real | MEASURED read |

**Multipliers (DERIVED, weak), applied to M4 milliseconds at 1440p:**
- **Pixel-bound GPU work:**
  - AVERAGE at 1080p ≈ ×0.20–0.30, from 0.5625 of the pixels divided by 1.9–2.8× the GPU (Geekbench 7, and Steel Nomad Light against the 4060 laptop).
  - LOW at 800p ≈ ×0.75–0.90, from 0.278 of the pixels divided by 0.31–0.37× the GPU (Geekbench 7).
- **CPU work:** AVERAGE ≈ ×1.29–1.38; LOW ≈ ×2.04 (PassMark single-thread).
- **Dev-machine proxy rule:**
  - Keep each CPU thread (main and render) ≤ **6.0 ms p50** on the M4 in the battle scene: 8.33 / 1.38 for AVERAGE at 120 fps, and 16.7 / 2.04 = 8.2 ms for LOW at 60 fps.
  - On the GPU side, M4 at 1440p is the *harder* machine for both references, so anything the M4's GPU finishes in 8.33 ms fits both (DERIVED).

**Why the translation is weak** (each point is checkable):
1. **Compute benchmarks are not raster benchmarks.**
   - Geekbench 7 Vulkan puts the RX 7600 at 0.35× an RTX 4060 (34,636), while Tom's raster puts it at 0.98× (34.3 % against 35.1 %).
   - Geekbench 7 also ranks the GTX 1060 6 GB (17,471) below the GTX 1650 (33,786); raster ranks them the other way (Tom's legacy: 26.5 against 20.9).
   - A compute ratio can be off by about 3× in either direction (CLAIMED inputs, DERIVED reading).
2. **We do not know which stage limits the frame on the M4.** Metal gives no GPU timestamps, and no tracy capture splits the main thread, the render thread and the GPU for the 30-tank scene. The evidence points both ways:
   - Pixel-bound: in August a 4× surface took 2.8× the time (m350-2: 4.19 ms at 1280×720 against 11.83 ms at 2560×1440, MEASURED, `run-frame-sweep.sh` header).
   - Pass- or draw-bound: shadows cost about 1.16 ms per cascade but only 0.27 ms for the whole 1024→4096 resolution ladder (MEASURED, `src/settings.rs` `ShadowCascades`), and LOD cut 35 % of the 30-tank frame (MEASURED, `lod-perf-verdict-2026-08-13.md`).
3. **API and architecture differ.**
   - Metal CPU-encodes every indirect draw (above); DX12 and Vulkan do not.
   - The M4 is a tile-based (TBDR) GPU, and the repo's shadow note calls per-cascade cost "Apple-specific": each cascade is a render pass, and pass boundaries are expensive on TBDR.
   - Both effects may make the PC's render thread and shadows *cheaper* than the CPU multiplier predicts. UNMEASURED.
4. **macOS presentation beats.** On this rig p90 is about 1.8× p50 in every run, shadows off included (owner note `profiling-phase01`). Tails measured on macOS do not transfer.
5. **Single-thread scores are not Bevy's schedule.** Bevy's task pool spreads systems across cores: 4P+6E on the M4, 6C/12T on a Ryzen 5 5600. Cache, memory latency and thread count change the ratio per system.
6. **Thermals and power.** The M4 laptop on AC, a desktop at 115 W graphics power, and a 4–15 W APU each throttle differently over 60 s runs (CLAIMED power figures).

---

## 4. Test rigs

| Option | Cost (label) | Fidelity | Agent-operable |
|---|---|---|---|
| **Physical AVERAGE PC** (Windows 11 plus a Linux dual-boot, RTX 4060, Ryzen 5 5600 class, 16 GB, 1080p 144 Hz monitor) | $900–1,200 with monitor (ASSUMED; 2026 RAM prices are volatile). Reference points: RTX 4060 MSRP $299.99 (CLAIMED, Tom's); Steam Machine $1,049, sold at cost by Valve's account (CLAIMED, press). | High: a real display, consumer drivers, real CPU, real present path, and GPU timestamps on DX12 and Vulkan (`src/render_cost.rs`) | **Yes.** A machine of its own can run *visible-window* sweeps over SSH without taking the owner's screen. Today every visible run waits for Yan to hand his screen over (`RUNBOOK.md`). |
| **Steam Deck** (the LOW reference) | OLED 512 GB $789; LCD discontinued; refurbished LCD from $319 (all CLAIMED, secondary press, single source for refurbished) | High for the Linux low end. Run in Desktop Mode, or with gamescope's frame limiter off. | Yes (SSH in Desktop Mode) |
| **AWS g6.xlarge** (NVIDIA L4, 4 vCPU, 16 GiB) | Windows $0.9888/h; Linux $0.8048/h, on demand in us-east-1 (CLAIMED, AWS price list read 2026-10-29). One sweep session ≈ $1 (DERIVED). Alternatives: g4dn.xlarge (T4) $0.71 / $0.526; g5.xlarge (A10G) $1.19 / $1.006. | **Throughput A/B only.** See the list below this table. | Yes, but parked: quotas were filed and an earlier request was denied; launches wait for the seeded-replay instrument; the CLI still runs on root credentials (owner memory `aws-benchmarking-account`) |

Why AWS only gives A/B throughput deltas:
- The CPU is 4 vCPUs against 6–8 desktop cores.
- The L4 is a 72 W, 300 GB/s data-center part. On paper it has 30.3 TFLOPS, twice the RTX 4060's FP32, which is exactly why paper specs mislead (CLAIMED, NVIDIA).
- It runs the data-center (GRID) driver.
- It has no physical display. The virtual display's refresh rate is UNVERIFIED.
- Virtualisation adds jitter to the tails.
- What it does give: the client's own frame loop (`frame_ms` from an `Instant` in `Update`, `src/frame_cost.rs`) measures throughput on any machine, and DX12 adds GPU timestamps.

**How the validity gates carry over** (`scripts/perf/run-frame-sweep.sh` header, `frame-run.zsh`, `analyze.py`):

| Gate | On the M4 today | On Windows (AVERAGE, AWS) | On the Steam Deck |
|---|---|---|---|
| Occlusion (no occluded interval in the window) | `WindowOccluded` from winit | **Blind.** winit 0.30.13 marks `Occluded` "Unsupported" on Windows and Wayland (`event.rs:418–421`), and wgpu-hal 29.0.4 raises `SurfaceError::Occluded` only on Metal (`metal/surface.rs:151`). Needs a new signal, such as a Win32 foreground/minimised poll written as stream rows, plus a dedicated, untouched machine. | Works under X11 (winit's X11 backend emits it). Blind under Wayland or gamescope. |
| Monitor ≥ 100,000 mHz, primary (`analyze.py:72`) | built-in 120 Hz panel | Passes on a 144 Hz monitor. Fails on AWS's virtual display. | Fails by design (60 or 90 Hz). Replace with "uncapped present confirmed, and frame times not quantised to multiples of the refresh period". |
| Surface exactly 2560×1440 (`analyze.py:79`); client pins 1280×720 logical (`src/frame_cost.rs:148`) | correct | **Must be parameterised.** At scale 1.0 the pin gives a 1280×720 surface, 0.92 MP. AVERAGE needs 1920×1080. | Needs 1280×800 |
| Present mode: the literal "Immediate, frame cap off" (`frame-run.zsh:161`) | Metal offers Immediate | DX12 and Vulkan on X11 offer Immediate | Wayland refuses Immediate and offers Mailbox (`src/settings.rs` `VsyncMode`). Accept Mailbox there. |
| Runner | zsh, `caffeinate`, `pgrep` | Port it, to PowerShell or to Python (`analyze.py` is Python already) | zsh works in Desktop Mode |
| Shader warm-up | discard the first run (Metal stalls more than 10 s) | Same rule (DX12 and Vulkan pipeline caches) | Same rule |
| Quiet machine | no builds, AC power, display lit | Also: Windows Update, Defender scans and Steam/NVIDIA overlays off; a fixed power plan | AC power; performance overlay off |

**Recommendation.**
- Buy the AVERAGE PC and a Steam Deck: about $1.7–2.0k in all (ASSUMED, with the Deck at $789 CLAIMED, or about $1.2–1.5k with a refurbished LCD).
- Parameterise the surface gate. Add a Windows occlusion signal and a quantisation gate.
- Keep AWS as an optional second-opinion A/B instrument once the owner lifts its sequencing.

---

## 5. A first budget

**What the repo has measured.** All on the M4, release build, 2560×1440 physical, Immediate present, built-in 120 Hz panel, gates passed, unless noted.

| Quantity | Value | When | Label | Source |
|---|---|---|---|---|
| Duel (2 tanks), cones baseline: p50 / p95 / p99 | 4.64 / 7.29 / 8.56 ms | 2026-10-29, `49f0982` | MEASURED, but WITHOUT RENDERING (the window never drew; `focus-gap-diagnosis.md`). The rendered duel is 14.59 ms p50 (`sweep-30-2026-10-29.md`) | `destruction-2026-10-26/forest-frame-cost-2026-10-29.md` |
| Duel with 635 segmented trees | 5.72 / 8.78 / 9.88 ms | same | MEASURED | same |
| Forest view, cones / trees (p50) | 4.51 / 5.24 ms | same | MEASURED | same |
| Trees − cones, p50 / p95 | forest +0.73 / +1.31; duel +1.09 / +1.49 ms | same | MEASURED | same |
| Whole sim tick, headless, test profile, 2 idle tanks: 243 / 878 / 9,768 colliders | 1,056 / 1,174 / 2,255 µs p50 | 2026-10-26 | MEASURED | `codex-lane-b-stage-3-5/verdict.md`; `src/headless_test.rs:1686` |
| Track-print relief, SUN SHADOW against LIGHTING | INVALID: the window never drew (`focus-gap-diagnosis.md`) | 2026-10-29 | — | `track-prints-relief-cost.md` |
| 30 tanks (2 + 28 idle probes), near placement, shipped shadows | 14.30–14.57 ms p50; p99 28.9–30.5 | 2026-08-13, `df080ff` | MEASURED | `shadow-caster-share-2026-08-13.md` |
| Same, shadows off | 11.19–11.32 ms p50 | same | MEASURED | same |
| Sun shadow cost at 30 tanks: total / tank casters / envelope + terrain | 3.22 ± 0.10 / 1.14 ± 0.25 / 2.08 ± 0.35 ms | same | DERIVED from MEASURED | same |
| LOD at 30 tanks: near (on / absent); far 600 m (on) | 13.9–14.5 / 21.4–22.0; 10.3–10.6 ms | 2026-08-13, v0.3.1 | MEASURED | `lod-perf-verdict-2026-08-13.md` |
| One shadow cascade; whole 1024→4096 resolution ladder | ~1.16 ms; 0.27 ms | 2026-07 | MEASURED (as the code doc states it) | `src/settings.rs` `ShadowCascades`, `ShadowResolution` |
| Track wrap (view), per tank per frame | ~56 µs, so 1.7 ms at 30 tanks | 2026-07-26 | MEASURED / DERIVED | `docs/design/track-model/architecture.md` §1a, §6 |
| Track contact field (authority sim), per tank per tick | 0.056 ms, so 1.7 ms per tick at 30 tanks | 2026-07 | MEASURED bench / DERIVED | `docs/design/track-model/HQ.md:1262–1265` |
| Pen cam corner while it shows (damage-model feedback inset) | +4.3 ms p50 offline; +6.2–6.8 ms online | 2026-10-20 | DERIVED from MEASURED | `damage-cam-2026-10-08/slice9b-frame-cost-2026-10-20.md` |
| Repeatability | 1–2 % within a session; up to ≈1 ms p50 drift across runs | — | MEASURED | `RUNBOOK.md`; slice 9b note |

**A baseline that moved, unexplained.**
- The duel read 11.83 ms in August (m350-2, `run-frame-sweep.sh` header).
- In the pen cam runs of 2026-10-19/20, the frames without the corner read 11.3–12.9 ms (a scripted-fire scene).
- On 2026-10-29 the duel read 4.64 ms.

The scenes differ. Bevy 0.19.1, lightyear 0.30, the trees and the terrain packs all landed in between. **Frame totals cannot be carried across sessions; only within-run deltas can.** The August 30-tank numbers say what 30 tanks cost *then*, not now.

**Structure of the budget (MEASURED read of source).** Bevy's `DefaultPlugins` includes `PipelinedRenderingPlugin` on native multi-threaded builds (`bevy_internal-0.19.1/src/default_plugins.rs:55–57`). The main world (sim, view, UI) runs in parallel with the render world (extract, prepare, queue, encode), and the GPU runs behind both. Frame time ≈ the slowest of the three, so each stage gets 8.33 ms.

The fixed tick is Bevy's default 64 Hz (`bevy_time-0.19.1/src/fixed.rs:24,76`). At 120 fps it runs on 64/120 ≈ 53 % of frames (DERIVED). Those frames carry the whole tick, so the tick shows up in p95/p99 more than in p50.

**Draft per-system budget: AVERAGE, 1920×1080 LOW, battle scene, ms at p50.** The allocations are ASSUMED. They are a starting split to be replaced by the first measurements on the rig. The basis column says what constrains each.

| System | Main thread | Render thread | GPU | Basis |
|---|---|---|---|---|
| Sim tick (FixedUpdate: client physics, interpolation, applying replication) | 1.5 on tick frames | — | — | Bare 2-tank tick on the M4 is 1.06 ms (MEASURED, test profile, not release) ×1.29–1.38 = 1.4–1.5 (DERIVED). The online client's tick is UNMEASURED; it mounts Avian too (`src/net/client.rs:504`). |
| Trees (sim share inside the tick; rendering) | 0.3 per tick | 0.3 | 0.5 | Segmented forest +1.2 ms per tick on the M4 (MEASURED) ×1.29–1.38 = **1.5–1.7, over** (DERIVED). Rendering ≈ 0 on the M4 at ~48 trunks within 150 m (MEASURED within noise). The lever is the collider count (forest note). |
| Tanks, view: track wrap, LOD and visibility, transforms | 2.0 | — | — | Wrap 1.7 ms on the M4 ×1.29–1.38 = **2.2–2.3, over** (DERIVED; whether the wrap runs in parallel is UNVERIFIED). LOD walk over 36,240 ranged entities ≈ 0–0.25 ms (MEASURED). |
| Tanks, terrain and buildings: depth prepass and opaque pass | — | 2.5 | 2.5 | No stage split exists. 30 tanks with shadows off read 11.3 ms on the M4 in August, total frame (MEASURED). |
| Shadows: 2 cascades, 2048, 350 m | — | 1.0 | 1.2 | 3.22 ms at 3 cascades on the M4, minus ~1.16 for one cascade = ~2.1 ms on the M4 (DERIVED). The Windows per-pass cost is UNMEASURED. |
| Terrain marks (track prints): stamping; relief at LIGHTING | 0.2 | 0.1 | 0.2 | Relief ≤ 0.05 ms on the M4 at 1440p (MEASURED). A stamping CPU bench exists (`src/track_prints/store.rs:1511`), but no figure is recorded in the notes. |
| Effects (muzzle, tracers, impacts, dust, smoke) | 0.5 | 0.5 | 1.0 | UNMEASURED. The `run-fire-capture.sh` harness exists, for the controlled tank only. |
| UI and post-processing (HUD; bloom, tonemapping, sky environment light) | 0.5 | 0.4 | 0.6 | UNMEASURED. Bloom and tonemapping run at full resolution whatever the render scale (`src/render_scale.rs:71–73`). |
| Netcode, audio, input | 0.5 | — | — | UNMEASURED |
| Engine floor (schedule, extract, prepare, submit) | 1.0 | 1.0 | — | ASSUMED |
| **Reserve** | 1.8 | 2.5 | 2.3 | ASSUMED |
| **Column total** | 8.3 | 8.3 | 8.3 | |

**Transient events** sit outside the steady-state columns and get their own ceiling. The proposal is ≤ 1.5 ms on AVERAGE while shown (ASSUMED) for the pen cam corner and the KILL CAM sequence. The corner measures +4.3 ms p50 on the M4 at 1440p, and its half-resolution option is already queued in the slice 9b note.

**LOW, the Steam Deck at 60 fps:** 16.7 ms per stage. CPU ×2.04 against the M4 (DERIVED) gives:
- bare tick ≈ 2.2 ms;
- forest +2.4 ms per tick;
- track wrap ≈ 3.5 ms per frame.

That is about 8 ms of main thread before effects, UI and netcode: it fits, but it is the stage to watch. GPU work ≈ ×0.75–0.90 of the M4 at 1440p (DERIVED).

**Missing measurements, in order of what they unblock:**
1. **30 tanks on current main, on the M4**, including a LOW candidate condition: 2 cascades, 2048, MSAA off, prints at LIGHTING. The runner exists; it needs about 10–15 min of the owner's screen. Without it no 30-tank number is current.
2. **A three-stage split** (main thread, render thread, GPU): tracy on the M4 (the only shadow-pass instrument), and `SPIKE_RENDER_COST` `gpu_ms` on the PC.
3. **The battle scene itself.** Probes are idle today (`src/tank/scenario.rs:491`), and scripted fire drives only the controlled tank, offline (`run-fire-capture.sh`). The scene needs 30 tanks moving and firing: track wrap under motion, print stamping, dust, muzzle, tracers, impacts and audio.
4. **The online client against a separate server.** All frame numbers so far come from the offline path, which runs the whole authority in-process: the 30-tank contact field alone is 1.7 ms per tick there (DERIVED).
5. The cost of effects, UI, bloom and the sky environment light. None is measured.
6. **Any x86, Windows or Linux frame at all.** None exists.
7. The Steam Deck.
8. Tails on Windows; first-launch pipeline stalls on DX12 and Vulkan.
9. VRAM footprint against 8 GB (AVERAGE) and 16 GB shared (Deck).
10. Hull marks (#124), felled trees as dynamic bodies, and destruction debris at scale.
11. The duel baseline's drop from August to October, as one A/B of v0.3.1 against current main on the same scene.

---

## 6. What "low settings" should turn down

**Today's rows** (`src/settings/ui.rs:140`, `Row::ORDER`). Defaults are in bold.

| Row | Rungs |
|---|---|
| RENDER SCALE | 50 / 67 / 75 / 85 / **100 %** |
| DETAIL BUDGET (LOD screen-space error) | 0.5–8 px, **1.0** |
| WINDOW MODE | **Windowed** / Fullscreen |
| DISPLAY | which monitor |
| SHADOW DISTANCE | Off / **350** / 700 / 1000 m |
| SHADOW QUALITY | 1024 / 2048 / **4096** |
| SHADOW CASCADES | 2 / **3** / 4 |
| TRACK PRINTS (relief) | Lighting / Parallax / **Sun Shadow** |
| TRACK PRINT RANGE | Near / Middle / **Far** / Farthest |
| MSAA | Off / 2× / **4×** |
| VSYNC | Off / Fast / **On** |
| FRAME CAP | armed only with VSYNC Off |
| UI SCALE | percent |
| DEBUG PANEL | **On** (beta) / Off |
| PEN CAM CORNER | **All** / Through only / Off |
| KILL CAM | **On** / Off |
| REDUCE MOTION | **Off** / On |

There is no preset: "LOW" would be a new bundle of row values. `src/render_policy.rs` holds no player setting. It owns per-object shadow roles (`ShadowRole`, `VisualScope`), the levers by which a mesh can stop casting.

**The rule** (`.agents/PRODUCT.md`): optimisations "must not silently become spotting or concealment rules". No abstract spotting exists, so the rendered picture is the visibility model. A setting that removes or thins anything that hides a tank gives the player who lowers it an advantage. A setting that culls a tank by distance hides it. Both are excluded. Lowering resolution or crispness uniformly costs the chooser some detail and hides nothing, so it is allowed.

**Proposed LOW**

| Row | LOW | Gameplay-visible? | What it saves |
|---|---|---|---|
| SHADOW CASCADES | **2** | No: crispness falls off sooner, but the shadows remain | ~1.16 ms on the M4 (MEASURED). The largest measured lever. |
| SHADOW QUALITY | **2048** | No: far shadows detach by ~1.07 m instead of 0.53 m at 350 m (`src/settings.rs` table) | ≤ 0.27 ms on the M4 for the whole ladder (MEASURED) |
| SHADOW DISTANCE | **350 m (unchanged)** | Yes at Off. `Off` exists but is "SUSPENDED rather than withdrawn" pending exactly this measurement (`docs/design/display/shadow-presets.md`). LOW must not use it. Whether Off survives is Yan's open decision. | 3.22 ms at 30 tanks on the M4, if Off were allowed (MEASURED) |
| MSAA | **Off** | No: "MSAA smooths edges, it does not reveal or conceal anything" (`src/settings.rs`). Tree cards fall back from alpha-to-coverage to `Mask(0.5)`. | UNMEASURED. The resolve stays at full resolution whatever the render scale. |
| TRACK PRINTS / RANGE | **Lighting / Near** | No: every level darkens the trodden ground the same; only the relief differs (`TrackPrintRelief` doc) | ≤ 0.05 ms on the M4 (MEASURED). Little to gain, but free to take. |
| DETAIL BUDGET | **2 px** (ASSUMED cap 4 px) | No, provided the coarsest tank rung covers every map distance. That invariant is not verified here; "no spotting by LOD" needs a test for it. | UNMEASURED |
| RENDER SCALE | **100 %** at 1080p; 75 % on Retina surfaces | Allowed: a uniform loss of resolution. It also coarsens LOD selection, because the LOD system's view height is window height × render scale (`src/view.rs:176`). | Pixel-bound passes go with scale² (DERIVED) |
| PEN CAM CORNER | unchanged; the corner itself must get cheaper (half resolution) | Not world visibility | +4.3 ms p50 while shown (MEASURED) |
| DEBUG PANEL | **Off** | No | UNMEASURED |

**Not on the page, but candidates for LOW:** these lower cost without lowering visibility.
- Half-resolution bloom. Not bloom off: "Tracer emissive values require HDR and bloom" (`src/camera.rs:323`), and tracers are gameplay information.
- A static sky environment map instead of the generated one (`src/world.rs:229`).
- Half-resolution particles at the same density and opacity.
- Tree LOD rungs moved nearer, keeping canopy coverage and casting from the trunk. A trunk shadow is a gameplay read (`destruction-2026-10-26/tree-rendering.md`).

**Never in any preset:**
- tank draw distance or culling by range;
- vegetation presence, density, draw distance or opacity, including grass if it is ever added (WoT's sniper-mode grass transparency is the counter-example);
- smoke, dust and fire density or opacity;
- the visibility of tracers and muzzle flashes;
- shadow existence within the envelope;
- fog or atmosphere, if added;
- the terrain LOD budget. `TERRAIN_LOD_BUDGET_PX` is a fixed 1.0 px (`src/terrain_lod.rs:151`) and must stay off the settings page, because a coarser ridge can hide or expose a hull-down tank.

War Thunder states the same intent publicly: equal draw distance for trees and houses across all presets (§2).

---

## Sources

**Repo**
- Product rules: `.agents/PRODUCT.md`
- Perf notes:
  - `.agents/scratch/perf-2026-10-29/track-prints-relief-cost.md`
  - `.agents/scratch/perf-2026-10-29/RUNBOOK.md`
  - `.agents/scratch/perf-2026-10-29/runs/forest/manifest.txt`
  - `.agents/scratch/destruction-2026-10-26/forest-frame-cost-2026-10-29.md`
  - `.agents/scratch/destruction-2026-10-26/codex-lane-b-stage-3-5/verdict.md`
  - `.agents/scratch/destruction-2026-10-26/tree-rendering.md`
  - `.agents/scratch/lod-perf-verdict-2026-08-13.md`
  - `.agents/scratch/shadow-caster-share-2026-08-13.md`
  - `.agents/scratch/damage-cam-2026-10-08/slice9b-frame-cost-2026-10-20.md`
  - `.agents/scratch/war-thunder-research-2026-10-26/public-sources.md`
- Design docs:
  - `.agents/docs/design/display/shadow-presets.md`
  - `.agents/docs/design/track-model/architecture.md` (§1a, §6)
  - `.agents/docs/design/track-model/HQ.md:1262–1265`
  - `.agents/docs/adr/0033-geometry-mipmapping-declarative-lod.md`
- Settings and rendering code:
  - `src/settings.rs`: `ShadowCascades`, `ShadowDistance`, `ShadowResolution`, `MsaaLevel`, `RenderScaleLevel`, `VsyncMode`, `TrackPrintRelief`, `TrackPrintRange`, `PixelBudget`
  - `src/settings/ui.rs:140`
  - `src/render_policy.rs`
  - `src/render_scale.rs:66–78`
  - `src/camera.rs:320–330`
  - `src/world.rs:200–235`
  - `src/view.rs:176`
  - `src/terrain_lod.rs:151`
- Measurement instruments:
  - `src/frame_cost.rs` (`SURFACE_LOGICAL` at :148)
  - `src/render_cost.rs`
  - `src/headless_test.rs:1686`
  - `src/track_prints/store.rs:1511`
  - `scripts/perf/run-frame-sweep.sh`, `frame-run.zsh:161`, `analyze.py:72,79`, `run-fire-capture.sh`
- Scenario and runtime:
  - `src/tank/scenario.rs:491`
  - `src/net/client.rs:504`
  - `src/lib.rs:550–560`
- Vendored crates:
  - `wgpu-hal-29.0.4`: `src/metal/command.rs:1574–1590, 1627–1636`; `src/metal/surface.rs:151`
  - `winit-0.30.13/src/event.rs:405–421`
  - `bevy_internal-0.19.1/src/default_plugins.rs:55–57`
  - `bevy_time-0.19.1/src/fixed.rs:24,76`
- Dev machine: `system_profiler` (Mac16,1, Apple M4, 10 cores (4P+6E), 16 GB, 3024×1964, 120 Hz).
- Owner memory notes, outside the repo: `profiling-phase01`, `shadow-perf-2026-07`, `aws-benchmarking-account`.

**Web**
- Steam Hardware & Software Survey, read via the Internet Archive's raw (`id_`) captures:
  - September 2026, combined: https://store.steampowered.com/hwsurvey/Steam-Hardware-Software-Survey-Welcome-to-Steam (capture 20261022045548)
  - Video cards, Windows, September 2026: https://store.steampowered.com/hwsurvey/videocard/ (20261026155202)
  - Linux, August 2026: https://store.steampowered.com/hwsurvey/?platform=linux (20261001085148)
  - Mac, August 2026: https://store.steampowered.com/hwsurvey/?platform=mac (20261001065314)
  - Processor vendor, September 2026: https://store.steampowered.com/hwsurvey/processormfg/ (20261005081709)
- Tom's Hardware GPU Benchmarks Hierarchy 2026, updated 24 June 2026: https://www.tomshardware.com/reviews/gpu-hierarchy,4388.html ; the legacy 2020–21 suite: https://www.tomshardware.com/reviews/gpu-hierarchy,4388-3.html
- Geekbench 7 charts, calibrated to RTX 4060 = 100,000:
  - https://browser.geekbench.com/metal-benchmarks (capture 20260910070109)
  - https://browser.geekbench.com/vulkan-benchmarks (capture 20260920085418)
- PassMark, updated 28 October 2026:
  - https://www.cpubenchmark.net/singleThread.html
  - https://www.cpubenchmark.net/cpu_list.php
  - https://www.cpubenchmark.net/cpu.php?cpu=Apple+M4+10+Core
  - https://www.cpubenchmark.net/cpu.php?cpu=AMD+Custom+APU+0932
- Notebookcheck, from search excerpts because the pages return 403:
  - M4 10-core GPU: https://www.notebookcheck.net/Apple-M4-10-core-GPU-Benchmarks-and-Specs.835807.0.html
  - RTX 4060 Laptop: https://www.notebookcheck.com/NVIDIA-GeForce-RTX-4060-Laptop-GPU-Benchmarks-und-Spezifikationen.675691.0.html
- UL, RTX 4060 Steel Nomad median 2,309: https://benchmarks.ul.com/hardware/gpu/NVIDIA+GeForce+RTX+4060+review
- Hardware specs:
  - Apple MacBook Pro 14" (M4, 2024): https://support.apple.com/en-us/121552
  - NVIDIA RTX 4060: https://www.nvidia.com/en-us/geforce/graphics-cards/40-series/rtx-4060-4060ti/
  - NVIDIA L4: https://www.nvidia.com/en-us/data-center/l4/
- AWS:
  - G6 instances: https://aws.amazon.com/ec2/instance-types/g6/
  - On-demand price lists, us-east-1: https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/USD/current/ec2-ondemand-without-sec-sel/US%20East%20(N.%20Virginia)/Linux/index.json and `…/Windows/index.json`
- Valve hardware:
  - Steam Deck store page (capture 20261027201032): https://store.steampowered.com/steamdeck
  - Price rise, secondary press: https://www.gematsu.com/2026/05/steam-deck-oled-price-increase-announced
  - Steam Machine price and specs, secondary press: https://www.engadget.com/2199144/valve--starts-at-1049-with-512gb-storage-and-no-controller/ , https://www.thesixthaxis.com/2026/06/22/steam-machine-price/
- Game requirements:
  - War Thunder: https://warthunder.com/en/game/about/ ; https://warthunder.com/en/news/7330-it-s-fixed-21-en (via the repo's War Thunder note)
  - World of Tanks: https://worldoftanks.eu/content/guide/general/system-requirements/ ; https://worldoftanks.eu/en/news/general-news/update-1-0-graphics-settings/
  - Valorant: https://support.riotgames.com/en-us/valorant/support-tools/minimum-recommended-pc-specs
  - Counter-Strike 2 (capture 20261028134130): https://store.steampowered.com/app/730/CounterStrike_2/
  - Battlefield 6 (capture 20251214175310): https://help.ea.com/en/articles/battlefield/battlefield-6/pc-requirements/
