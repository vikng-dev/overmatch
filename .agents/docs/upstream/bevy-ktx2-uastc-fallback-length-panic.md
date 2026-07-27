# Upstream: KTX2 UASTC transcoding panics when no compressed format is supported

Status: **READY TO FILE — unreported, unfixed on `main`, no pending release addresses it.**
(Yan files under his own name; the repro and suggested patch below are written to be pasted.)
Found: 2026-07-27, Overmatch — the tank glb's PNG/JPEG textures are being replaced with mipped
UASTC KTX2 (`scripts/encode-tank-ktx2.sh`), which panics every headless composition on boot.
Root-caused from registry source, not inferred; the panic is reproduced in-tree by
`tests/bevy_ktx2_uastc_fallback.rs`.

## Upstream status (checked 2026-07-27)

- **Not reported.** ~26 distinct query sets against `bevyengine/bevy` issues *and* PRs (all states):
  `ktx2 transcode panic`, `level_bytes`, `UASTC`, `get_transcoded_formats`, `transcode_slice`,
  `block_copy_size`, `out of range for slice`, `ktx2 CompressedImageFormats::NONE`,
  `LowLevelUastcTranscoder`, `SliceParametersUastc`, plus a full enumeration of every open issue and
  every PR mentioning `ktx2`. A GitHub-wide code/issue search for `ktx2 uastc "level_bytes"` returns
  zero hits.
- **Not fixed on `main`.** `crates/bevy_image/src/ktx2.rs` on `main` (fetched 2026-07-27) is
  byte-identical to the `v0.19.0` tag for this arm. The logic dates to the original KTX2 PR
  [#3884](https://github.com/bevyengine/bevy/pull/3884) (2022-03-15, then
  `bevy_render/src/texture/ktx2.rs` using `describe().block_size`), survived the bevy_image split
  ([#15650](https://github.com/bevyengine/bevy/pull/15650)), and no commit in the file's history
  touches the arithmetic — recent ones are the ktx2 crate bump (#23900), ASTC HDR (#21886), renames
  (#23267, #23160), zstd (#19793), `TextureDataOrder` (#19829).
- **No release fixes it.** Latest release is v0.19.0 (2026-06-18). 0.19.1 and 0.20 exist only as
  open milestones; all 65 items in the 0.19.1 milestone were enumerated — nothing ktx2-, image-, or
  transcode-related.

Prior art, all distinct from this defect:

- [#9121](https://github.com/bevyengine/bevy/issues/9121) / [#9158](https://github.com/bevyengine/bevy/pull/9158)
  "Fix panic whilst loading UASTC encoded ktx2 textures" (0.11.1). **Same trigger family, different
  panic.** There the abort was downstream in wgpu (`create_texture` without the ASTC feature) and the
  fix was to defer `ImageTextureLoader` init until the real formats are known. It never touched this
  arithmetic — and it is the PR that institutionalized `CompressedImageFormats::NONE` as the
  no-`RenderDevice` default, i.e. it made *this* panic path reachable by design. Right prior-art
  reference to cite when filing.
- [#11099](https://github.com/bevyengine/bevy/issues/11099) (open) "Crash if KTX2 images are
  'unaligned' sizes" — same *message class* (`range end index … out of range`) but the panic site is
  `wgpu/src/util/device.rs`, blocked on [gfx-rs/wgpu#7677](https://github.com/gfx-rs/wgpu/issues/7677).
  Also #13289, #14315, #19124 (same wgpu site).
- [#16859](https://github.com/bevyengine/bevy/issues/16859) (open) — `Uastc(Rgb)` transcoding
  *error*, not a panic.

## Mechanism (verified against bevy_image-0.19.0 source)

`ktx2_buffer_to_image`, `TranscodeFormat::Uastc` arm, `bevy_image-0.19.0/src/ktx2.rs:172-225`:

```rust
let (transcode_block_format, texture_format) =
    get_transcoded_formats(supported_compressed_formats, data_format, is_srgb);
let texture_format_info = texture_format;                    // ktx2.rs:175 — DESTINATION format
let (block_width_pixels, block_height_pixels) = (
    texture_format_info.block_dimensions().0,                // ktx2.rs:176-179
    texture_format_info.block_dimensions().1,
);
let block_bytes = texture_format_info.block_copy_size(None).unwrap();   // ktx2.rs:181
...
    let (num_blocks_x, num_blocks_y) = (
        level_width.div_ceil(block_width_pixels).max(1),     // ktx2.rs:189-192
        level_height.div_ceil(block_height_pixels).max(1),
    );
    let level_bytes = (num_blocks_x * num_blocks_y * block_bytes) as usize;   // ktx2.rs:193
    ...
        transcoder.transcode_slice(
            &level_data[offset..(offset + level_bytes)],     // ktx2.rs:209 — SOURCE slice
```

`level_data` is the **UASTC** payload: always 4x4-pixel blocks of 16 bytes, by definition of the
format being transcoded *from*. But every quantity used to size that source slice — and to fill
`SliceParametersUastc.num_blocks_x/_y` at `ktx2.rs:200-206` — is read off `texture_format`, the
format being transcoded *to*.

The bug is invisible in practice because `get_transcoded_formats` (`ktx2.rs:310-388`) picks ASTC 4x4
or BC7 whenever the GPU supports them, and both are 4x4 pixels / 16 bytes per block — the same
geometry as the source, so the wrong derivation lands on the right number. The fallback branches are
where it separates:

| supported formats | dest format (`ktx2.rs`) | dest block | `level_bytes` vs truth |
|---|---|---|---|
| `ASTC_LDR` | `Astc{B4x4}` (:346) | 4x4 / 16 B | correct (by coincidence) |
| `BC` | `Bc7RgbaUnorm[Srgb]` (:358) | 4x4 / 16 B | correct (by coincidence) |
| `ETC2` | `Etc2Rgba8Unorm[Srgb]` (:367) | 4x4 / 16 B | correct (by coincidence) |
| **`NONE`** (Rgb/Rgba) | `Rgba8Unorm[Srgb]` (:376-385) | **1x1 / 4 B** | **4x too large → PANIC** |
| **`NONE`** (Rrr) | `R8Unorm` (:325) | **1x1 / 1 B** | **4x too small → silent short read** |
| **`NONE`** (Rrrg / Rg) | `Rg8Unorm` (:337) | **1x1 / 2 B** | **2x too small → silent short read** |

So the colour path aborts the process and the greyscale/two-channel paths quietly transcode garbage
from a misaligned window — the same root cause with two different symptoms. Only the panic is
observable today.

Reproduced (Overmatch `tests/bevy_ktx2_uastc_fallback.rs`, 16x16 UASTC 4x4, 5 mip levels):

```
thread '…' panicked at bevy_image-0.19.0/src/ktx2.rs:209:52:
range end index 1024 out of range for slice of length 256
```

16x16 = 4x4 UASTC blocks x 16 B = **256 B** of real level-0 data; the destination-derived
computation asks for 16 x 16 x 4 = **1024 B**. Exactly 4x, on the first level.

Same panic on the real asset (`assets/tiger_1/tiger_1.mipped.glb`, mitigation removed, headless
boot): `range end index 1048576 out of range for slice of length 262144` (the 512² track-link maps)
and `16777216 out of range for slice of length 4194304` (the 4k hull/turret atlas) — 4x every time.

Symptom shape depends on the caller. Through the asset server the unwind happens on an IO-task-pool
thread and `bevy_asset` converts it into `Failed to load asset '…', asset loader
'bevy_gltf::loader::GltfLoader' panicked` — so it is a hard, unrecoverable load failure rather than
a process abort, and any app that treats a missing tank as fatal simply never boots. Called
directly, `Image::from_buffer` unwinds on the caller's thread (which is what makes the tripwire
below `catch_unwind`-able).

### Repro (for the issue)

Any app without a wgpu device loading a UASTC KTX2 — which after
[#9158](https://github.com/bevyengine/bevy/pull/9158) means any headless app:

```rust
// bevy 0.19.0, features = ["basis-universal"]
App::new().add_plugins(DefaultPlugins.set(RenderPlugin {
    render_creation: WgpuSettings { backends: None, ..default() }.into(),
    ..default()
}));
// … load any .ktx2 holding UASTC 4x4 (basisu -uastc -ktx2 -mipmap), or directly:
Image::from_buffer(
    uastc_ktx2_bytes,
    ImageType::MimeType("image/ktx2"),
    CompressedImageFormats::NONE,   // what RenderPlugin{backends:None} resolves to
    true,
    ImageSampler::Default,
    RenderAssetUsages::default(),
);   // panics instead of returning TextureError
```

Note this is not a "you asked for the impossible" case: `CompressedImageFormats::NONE` selects a
legitimate, fully-implemented `TranscoderBlockFormat::RGBA32` path. The transcode is *supposed* to
work here; only the slice arithmetic is wrong. Even if it were unsupported, a loader is expected to
return `TextureError`, not to unwind through the asset pipeline.

### Suggested fix

Derive the source slice length (and the `SliceParametersUastc` block counts) from the **source**
format's geometry. UASTC is fixed at 4x4 pixels / 16 bytes per block, so the constants are not a
guess:

```rust
// UASTC 4x4 is the format being transcoded FROM: its blocks are 4x4 texels of 16 bytes,
// regardless of what we are transcoding TO.
const UASTC_BLOCK_DIM: u32 = 4;
const UASTC_BLOCK_BYTES: u32 = 16;
...
let (num_blocks_x, num_blocks_y) = (
    level_width.div_ceil(UASTC_BLOCK_DIM).max(1),
    level_height.div_ceil(UASTC_BLOCK_DIM).max(1),
);
let level_bytes = (num_blocks_x * num_blocks_y * UASTC_BLOCK_BYTES) as usize;
```

`transcode_block_format` already carries the destination geometry into the transcoder, so nothing
downstream needs the old values. A slice-bounds check (`offset + level_bytes <= level_data.len()`
→ `TextureError::SuperDecompressionError`) is worth adding on top so malformed KTX2 files error
rather than panic. A unit test in `crates/bevy_image/src/ktx2.rs` over a tiny UASTC fixture with
`CompressedImageFormats::NONE` pins it cheaply.

## Local resolution

Both headless compositions insert the resource bevy would have inserted if a device existed:

```rust
app.insert_resource(bevy::image::CompressedImageFormatSupport(
    bevy::image::CompressedImageFormats::ASTC_LDR,
));
```

- `src/headless_test.rs` (`headless_app_on`, before `app.finish()`)
- `src/net/server.rs` (`run`, before `app.run()`)

Ordering is load-bearing: both `bevy_render`'s `TexturePlugin::finish` (`texture/mod.rs:47-60`) and
`bevy_gltf`'s `finish` (`bevy_gltf-0.19.0/src/lib.rs:282-299`) read the resource once, at finish
time, and warn-then-default to `NONE` when it is absent.

Why the claim is safe rather than merely convenient: it makes the destination geometry (ASTC 4x4,
16 B) match the source's, so the buggy derivation lands on the correct number and the transcode is
*exact* — UASTC→ASTC 4x4 is the lossless path bevy's own comment at `ktx2.rs:343-345` prefers.
Neither headless composition has a GPU to upload to, so the only consequence is which bytes sit in
RAM, and ASTC 4x4 is 8 bpp against the RGBA8 fallback's 32 — strictly less memory than the honest
answer would have cost. Nothing reads texture *contents* on the server; the tank's collision and
ballistic geometry come from the baked blueprint, not from image data.

**Retirement is automatic.** `tests/bevy_ktx2_uastc_fallback.rs::fallback_transcode_still_panics` is
inverted — it asserts the panic still happens, so it PASSES while upstream is broken and FAILS the
first time a bevy upgrade fixes the arithmetic. That failure is the instruction to delete both
insertions, the test, its fixture, and to move this doc to DO-NOT-FILE with the fixing PR. The
companion test `astc_support_transcodes_the_same_bytes` pins the workaround's premise (same bytes,
5 mip levels, `Ok`) and stays valid after a fix.

## Artifacts

- `tests/bevy_ktx2_uastc_fallback.rs` — the inverted tripwire + the control.
- `tests/fixtures/uastc_16x16_mipped.ktx2` — 637 B, sha256
  `613f2dd06229d7faa9b07452c322524d702ec1ee268d6455745f9ba4cf0ea333`. Generated once from a
  synthetic 16x16 RGB PNG with
  `basisu -uastc -uastc_level 2 -mip_srgb -ktx2 -ktx2_zstandard_level 9 -mipmap` (basis_universal
  v2.10.0) — the same recipe `scripts/encode-tank-ktx2.sh` uses for the Tiger's sRGB colour maps,
  so the fixture exercises the shipping encode (UASTC 4x4, 5-level mip chain, Zstandard
  supercompression). Committed as bytes; `basisu` is never invoked at test time.
- `scripts/encode-tank-ktx2.sh` — the bake whose output triggers this in production.
