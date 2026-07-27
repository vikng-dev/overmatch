# Display / resolution / scaling — research brief (2026-07-27)

Source-verified research brief for the display-settings track. Produced by a research agent from
vendored-source reading (bevy 0.19 / winit 0.30.13 / wgpu-hal 29.0.4) + Apple docs + shipped-game
survey; commissioned after Yan's observations "fullscreen resolution changes nothing" and "very
few games handle macOS display properly".

**The doctrine in one line (proposed ADR-0031): we never enumerate display modes and never
mode-set a display; the drawable is always the window's real size and only the 3D render target
scales.**

## 0. The two observations, answered

**"In fullscreen the resolution setting changes nothing."** Mechanism fact, not a knob bug. The
F7 dev knob sets `WindowResolution::set_scale_factor_override`; `bevy_winit` turns that into a
window resize request (`changed_windows`, bevy_winit-0.19.0/src/system.rs:386-430); a window in a
fullscreen Space has its frame owned by macOS, so the request is ignored; only `react_to_resize`
writes `physical_*`, and the wgpu surface (→ `CAMetalLayer.setDrawableSize`) is configured from
`physical_*`. Drawable never moves; GPU cost identical; only the logical canvas (UI size)
changes. F7 is a window-size knob wearing a render-scale costume. Upstream agrees the path is
unreliable: bevy #24724 (0.19), #8921, #17188.

**"Very few games handle macOS display properly."** Named cause: `CGDisplayCopyAllDisplayModes`
returns modes for TWO regions in one unfilterable list (full panel vs below-menu-bar area).
Games take entry [0], render 3456×2234 on a 16" MBP where fullscreen apps get 3456×2160, and
Core Animation squashes 74 px. Measured wrong: Shadow of the Tomb Raider, No Man's Sky, Stray,
Riven. (Colin Cornaby, "Your Mac Game Is Probably Rendering Blurry", FB13375033, open since
Sept 2023.) We never enumerate modes → immune by construction.

## 1. macOS display reality, 2026

- **No exclusive fullscreen worth having.** Apple prescribes `toggleFullScreen:` +
  `.fullScreenPrimary` ("Avoid customizing any aspects of what toggleFullScreen does").
  `CGDisplayCapture` de-facto abandoned (SDL3: zero calls; SDL2 use fails on Sonoma/M2,
  SDL #12696). CGDisplaySetDisplayMode costs: no ⌘-Tab, no debugger (SDL #7776), desktop
  mis-scaled on exit, and DISQUALIFIES from macOS Game Mode (requires native fullscreen).
  Unity falls back to FullScreenWindow on non-Windows; Godot's "exclusive" is borderless+Dock
  suppression.
- **Compositor always runs at desktop mode.** `CAMetalLayer.drawableSize` is a plain settable
  property decoupled from window size — THE render-scaling mechanism. Apple's prescribed
  architecture: "Render 2D UI that matches the view's backing size and render 3D in a different
  render target… Then upscale it to the final drawable using a custom render pass or MetalFX."
  Upscale filter: `CALayer.magnificationFilter` (default .linear).
- **Backing scale** is binary 1.0/2.0 (winit #2048). "Looks like 1440p" on 4K = macOS composites
  5120×2880 then downsamples — user desktop scaling inflates pixels AND softens output unless we
  size the drawable ourselves. Free lunch (Apple forums 82929): rendering at EXACTLY half panel
  resolution gets integer pixel quadrupling by WindowServer, no interpolation blur → 50% and
  100% are special render-scale snap points.
- **ProMotion/vsync:** adaptive refresh needs adaptive display AND fullscreen (WWDC21-10147).
  No macOS opt-in for >60 needed (120 not gated). Vsync-off = `displaySyncEnabled=false`,
  documented to tear, but ADVISORY — compositor can take control (SDL #12088; bevy #12097:
  Immediate capped at 120 only while an unrelated app was fullscreen elsewhere).
- **⚠️ macOS 26 Tahoe regression (Yan is on 26.5):** multiple reports of fullscreen frame rate
  hard-capped at refresh regardless of vsync (Apple forums 799541, bevy #12097 on 26.2;
  Psychtoolbox: "framerates cut in half… 26.0–26.2"). Also ~1 px translucent border on
  borderless-fullscreen and a transparent menu bar that EATS INPUT in the top strip. Budget for
  this before blaming our renderer for fps ceilings.
- **Notch:** Spaces fullscreen auto-positions content inside safe area; fullscreen area starts
  below the MENU BAR, safe area below the NOTCH. `NSPrefersDisplaySafeAreaCompatibilityMode=true`
  scales ~0.96× and blurs (OpenRA #20568). Non-issue for us; leave the plist key absent.

## 2. winit 0.30.13 macOS facts

- `Fullscreen::Borderless` → `toggleFullScreen:` (idiomatic per winit docs);
  `with_borderless_game(true)` adds HideDock|HideMenuBar — **Bevy's `borderless_game` defaults
  to true** (window.rs:446), so dock/menu hiding is already correct.
- `Fullscreen::Exclusive` → real CGDisplayCapture + mode-set with THREE asserts + an unwrap —
  monitor unplug/contended capture = process abort. Open bugs: #2050 (desktop mis-scaled after
  exit), #1992 (NO KEYBOARD if launched into exclusive — never boot into it), #4162 (notch),
  #2068 (mode switching), #4440 (resize uses current monitor's scale).
- Fullscreen transitions are deferred/coalesced; a toggle during a Space animation silently
  fails (winit retries only for creation-time fullscreen). → "apply then verify": re-read
  `Window` next frame and reconcile UI to reality.
- `video_modes()` reports pixel sizes with the CURRENT refresh substituted into every mode
  (fiction on Apple Silicon). Monitor names are "Monitor #<model>" — duplicates
  indistinguishable.
- ≥0.30.12 mandatory for macOS 26 (objc2 crash); we're on .13.

## 3. Bevy 0.19 capabilities

- `WindowMode::{Windowed, BorderlessFullscreen(MonitorSelection), Fullscreen(..)}`;
  `Fullscreen` PANICS on unresolvable monitor (bevy_winit/src/system.rs:343).
- **PresentMode on Metal: only `Fifo` and `Immediate` are real; `Mailbox`/`FifoRelaxed`
  hit `unreachable!()` — they PANIC, no fallback** (wgpu-hal metal adapter.rs:416-418,
  surface.rs:73-77). Safe set: Fifo / Immediate / AutoVsync / AutoNoVsync.
- `desired_maximum_frame_latency` → `maximumDrawableCount(n+1)`; the only buffering knob.
  NonZero(1) = double buffering = optional "Low latency" toggle later.
- **Render scale routes:**
  - (a) scale_factor_override (today's F7): retire — see §0.
  - (b) Camera `viewport`: does NOT upscale — scissor on a 1:1 blit → letterbox, not scale.
  - (c) **`MainPassResolutionOverride(UVec2)`** (bevy_camera/src/camera.rs:144): THE primitive.
    Honored by prepass/main opaque/main transparent/deferred/OIT; fed to shaders as
    `View::main_pass_viewport`. Exists as DLSS scaffolding (PR #18381); only
    dlss/prepare.rs:111 inserts it today; DLSS node sits in `Core3dSystems::EarlyPostProcess`.
    **Missing piece = exactly one upscale pass** (fork bevy blit; its sampler is NonFiltering —
    need a filtering variant for bilinear).
- **Pipeline ordering that makes Route A cheap:** Prepass → MainPass → EarlyPostProcess →
  PostProcess (schedule.rs:66); bloom in PostProcess before tonemapping; **ui_pass runs
  .after(PostProcess).before(upscaling)** → upscale node in EarlyPostProcess ⇒ 3D reduced,
  bloom/tonemap/UI native. Exactly Apple's architecture.
- **UiScale** is complete: drives Val::Px layout AND text rasterization (higher-res glyph
  atlas). All 19 FontSize::Px sites in repo scale correctly.
- **MSAA under resolution override:** sampled/depth/ViewTarget textures still FULL size —
  render scale saves shading, not memory, and not MSAA resolve. At scale<1 prefer MSAA 2×/off.
- Bevy 0.19 has NO first-class render scale (#20859, #21530 open "Needs Design"), NO FSR/MetalFX,
  NO frame limiter (hand-roll sleep in `Last`). `ContrastAdaptiveSharpening` exists in
  bevy_anti_alias — pairs well with bilinear upscale as free polish.

## 4. Shipped-game patterns

- Window mode on macOS: TWO entries (Windowed / Fullscreen-borderless). OpenLoco disables the
  mode dropdown on macOS entirely (PR #3671) — precedent for per-platform hidden rows.
- **Render scale, not resolution.** WoW's Render Scale is the gold standard ("5K iMac defaults
  to 5120×2880 with full-scale crisp UI, renders the game to a 2560×1440 backbuffer"; Retina
  defaults 50%). Modern AAA Mac ports keep an output-resolution control + MetalFX as a separate
  axis; for an indie, WoW's model (native output + render-scale slider, no resolution list) is
  the right simplification. NOTE (honesty): no shipped Mac title was found that fully REPLACED
  the resolution dropdown with render scale — we'd be adopting the cleaner model, not copying one.
- UI scale always separate. Known-bad: UI scaling with render scale; resolution lists from
  CGDisplayCopyAllDisplayModes; desktop mode-setting (raylib #5038); wrong monitor crashes
  (osu #18983); **settings screen writing config on OPEN (MTG Arena) — only actual changes write.**
- Steam Deck: gamescope owns resolution/FSR/fps; "text should be legible" at 1280×800 →
  UI scale must be independent + per-platform presettable.

## 5. Recommended settings model

| Setting | Values | Live? |
|---|---|---|
| Window mode | Windowed / Fullscreen (=BorderlessFullscreen(Current)) | Live (apply-then-verify on macOS) |
| Windowed size | 1280×720 / 1600×900 / 1920×1080 / custom (logical) | Live |
| Display | Primary / monitor N (Monitor entities) | Re-enter fullscreen |
| Render scale | 50/67/75/85/100% (50+100 = integer-scaling snap points) | Live |
| UI scale | 75/90/100/115/130% → UiScale | Live |
| VSync | On (Fifo) / Off (AutoNoVsync) — advisory on macOS; Tahoe may cap fullscreen anyway | Live |
| Frame cap | Off/72/90/120/144 — **floored ≥72 or disabled in MP** (WinitSettings::continuous() exists to keep 64 Hz tick; a 60 cap reintroduces the lightyear #1113 jitter class) | Live |
| MSAA | Off/2×/4× | Live |
| Shadows | distance presets, count = compile constant | Live (distance only) |

**Deliberately absent: resolution dropdown** (macOS blur trap; Wayland can't mode-set; Windows
only meaningful with exclusive). **Exclusive fullscreen: don't ship in alpha line**; if ever,
Windows-only at menu level, never boot into it.

- Boot-time: mode/windowed size/present mode must be read BEFORE DefaultPlugins (window fields
  at creation; bevy #17208/#17188). Today lib.rs:626 + net/client.rs:165 set only `title`.
- Relaunch-only: shadow cascade COUNT (bevy Local<Parallel> crash, documented on
  `settings::SHADOW_CASCADES`), exclusive video mode (if ever).
- Persistence: RON; per-user config dir (macOS ~/Library/Application Support/dev.vikng.overmatch/
  settings.ron; Windows %APPDATA%\Overmatch; Linux XDG) — NOT next to exe (signed .app bundle is
  read-only). Pure `settings_path_from(...)` mirroring assets::asset_root_from + OVERMATCH_SETTINGS
  env override. Debounced atomic write-on-change; malformed file → warn + defaults, never panic;
  `--reset-display` escape hatch; opening settings never writes.

## 5.4 Render scale implementation — Route A (recommended)

New `src/render_scale.rs`, mounted in both windowed roots (as written, beside the then-extant
`perf::plugin`; that dev panel and its F4–F8 keys were deleted 2026-07-27 — everything below about
"measure with F4" now means `cargo tracy`):
1. Main-world `RenderScale(f32)` resource, ExtractResource.
2. Render-world system in `RenderSystems::Prepare` `.after(prepare_view_targets)` (DLSS's slot):
   insert `MainPassResolutionOverride((size * s).max(UVec2::ONE))` on the 3D view; remove at 1.0.
3. Node `render_scale_upscale` in `Core3dSystems::EarlyPostProcess`: post_process_write(),
   fullscreen triangle, sample `uv * vec2(w/W, h/H)`; fork bevy blit with a FILTERING sampler.

Why: camera still targets the window → aim.rs:285,387, sight.rs:627,679, hud.rs:132,192 need
ZERO changes (world_to_viewport stays window-logical); vfx shaders don't read view.viewport;
overlay.rs untouched. Blast radius = render app only. Limits (honest): no VRAM saving; bloom/
tonemapping/UI don't scale (camera.rs:192-196 runs Hdr+Bloom::NATURAL — measure bloom before
caring); MSAA resolve full-res. ~250-350 lines + ~20-line WGSL. 

Route B (offscreen RenderTarget::Image + ImageRenderTarget.scale_factor trick) only if bloom
dominates — real blast radius on UI foundation. Route D (MetalFX via raw MTLTexture) = the
eventual answer, weeks, park.

## 5.6 Info.plist (scripts/package-macos.sh:89-105)

Add: `LSApplicationCategoryType = public.app-category.games`, `LSSupportsGameMode = true`
(macOS 26+, Game Mode = sustained CPU/GPU priority + 2× BT sampling; requires native
fullscreen = another reason Fullscreen row is BorderlessFullscreen). Consider
LSMinimumSystemVersion → 11.0 (arm64-only target). Leave NSPrefersDisplaySafeAreaCompatibilityMode absent.

## 7. Sequencing

1. src/settings.rs — DisplaySettings resource, RON persistence, pure path resolution + tests,
   pre-DefaultPlugins load. No new deps.
2. Wire free knobs: window mode, windowed size, UiScale, PresentMode, MSAA, shadow distance.
   Delete F7.
3. src/render_scale.rs — Route A. Measure with F4: main-pass span moves, bloom/ui spans don't.
4. Info.plist keys + Game Mode verification.
5. Frame cap (floored ≥72) only if playtest asks.
6. ADR-0031: "Render scale, not resolution — the display surface is the window."

## Re-measure rather than trust

- Does AutoNoVsync actually tear/unthrottle on M4 + macOS 26.5? Is fullscreen hard-capped on
  Tahoe? (Both cheap F4 A/Bs; both change what the VSync row may promise.)
- Settings UI goes inside the existing Overlay::Menu as a page (command.rs:82 precedent:
  resource is the source of truth, systems react to Changed<>).
