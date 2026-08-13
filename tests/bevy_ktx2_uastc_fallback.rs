//! UPSTREAM TRIPWIRE for the bevy_image KTX2/UASTC fallback length panic that the
//! `CompressedImageFormatSupport` insertion in `src/headless_test.rs` works around. Full decode +
//! suggested upstream fix: `upstream/bevy-ktx2-uastc-fallback-length-panic.md`.
//!
//! The mechanism being pinned, in bevy_image 0.19 (`src/ktx2.rs`, `ktx2_buffer_to_image`,
//! `TranscodeFormat::Uastc` arm): the length of the SOURCE slice handed to
//! `LowLevelUastcTranscoder::transcode_slice` (`ktx2.rs:209`) is computed at `ktx2.rs:176-193`
//! from the DESTINATION `texture_format`'s block geometry. The source is always UASTC 4x4 /
//! 16 B per block, so the arithmetic is only accidentally right when the destination happens to
//! share that geometry (ASTC 4x4 or BC7). With no compressed-format support at all,
//! `get_transcoded_formats` (`ktx2.rs:346`) falls back to `RGBA32`/`Rgba8UnormSrgb` — 1x1 blocks
//! of 4 B — and `level_bytes` comes out exactly 4x too large, so the slice index PANICS
//! ("range end index N out of range for slice of length N/4") instead of returning a
//! `TextureError`. Every headless composition here builds `RenderPlugin { backends: None }`, so
//! `CompressedImageFormatSupport` is absent and bevy_gltf loads with
//! `CompressedImageFormats::NONE` — i.e. the tank glb's UASTC textures would panic any headless
//! composition that opens the VIEW artifact on boot. The dedicated server is not one of them: it
//! opens `<id>.sim.glb`, which carries no image at all.
//!
//! **IF `fallback_transcode_still_panics` FAILS, bevy FIXED ktx2 fallback transcoding.** The test
//! is INVERTED on purpose: it passes while upstream is broken, so its failure is the retirement
//! signal, not a regression. On that day: delete the `CompressedImageFormatSupport` insertion in
//! `src/headless_test.rs`; delete this test file and `tests/fixtures/uastc_16x16_mipped.ktx2`; and
//! move `upstream/bevy-ktx2-uastc-fallback-length-panic.md` to DO-NOT-FILE, citing the fixing PR.
//!
//! The panic is raised on this thread inside `Image::from_buffer`, so `catch_unwind` observes it
//! directly. No panic hook is installed: a `thread ... panicked at ... range end index` line in
//! the test output is EXPECTED here and is the tripwire doing its job, not a failure.

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, Image, ImageSampler, ImageType, TextureError};

/// 16x16 UASTC 4x4 with a full 5-level mip chain and Zstandard supercompression — the same
/// `basisu` recipe `scripts/encode-tank-ktx2.sh` applies to the Tiger's sRGB colour maps
/// (`-uastc -uastc_level 2 -mip_srgb -ktx2 -ktx2_zstandard_level 9 -mipmap`), just tiny.
/// Generated once and committed (637 B); `basisu` is NOT invoked at test time (CI has none).
/// The mip chain matters: it exercises the per-level loop that miscomputes the slice length.
const UASTC_KTX2: &[u8] = include_bytes!("fixtures/uastc_16x16_mipped.ktx2");

/// Loads the fixture exactly the way `bevy_gltf`'s `load_image` does for a glb-embedded texture
/// (`bevy_gltf-0.19.0/src/loader/mod.rs:1205`): by mime type, with whatever compressed-format
/// support the app resolved, `is_srgb` from the material role.
fn load_like_gltf(supported: CompressedImageFormats) -> Result<Image, TextureError> {
    Image::from_buffer(
        UASTC_KTX2,
        ImageType::MimeType("image/ktx2"),
        supported,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
}

/// THE TRIPWIRE. Upstream is broken while this passes; see the module header for the removal
/// checklist the failure demands.
#[test]
fn fallback_transcode_still_panics() {
    let outcome = std::panic::catch_unwind(|| load_like_gltf(CompressedImageFormats::NONE));

    let payload = outcome.err().unwrap_or_else(|| {
        panic!(
            "bevy_image no longer panics transcoding UASTC KTX2 with CompressedImageFormats::NONE \
             — upstream fixed the ktx2 fallback slice length. Remove the \
             CompressedImageFormatSupport insertion in src/headless_test.rs, delete this test and \
             tests/fixtures/uastc_16x16_mipped.ktx2, and update \
             upstream/bevy-ktx2-uastc-fallback-length-panic.md (see module header)."
        )
    });

    // Pin the mechanism, not just "something panicked": a different panic here means the defect
    // moved and the workaround's justification has to be re-derived before it is trusted.
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".into());
    assert!(
        message.contains("range end index"),
        "expected the ktx2.rs:209 slice-index panic, got a different one: {message}",
    );
}

/// The workaround's premise: claiming ASTC support makes the destination geometry match UASTC's
/// (4x4 / 16 B), the length arithmetic coincides, and the same bytes transcode cleanly. Headless
/// never uploads the result, so the claim costs nothing but the RAM — and ASTC 4x4 is 8 bpp
/// against RGBA8's 32, so it costs LESS than the honest fallback would have.
///
/// This one stays true after an upstream fix; it is the control, not the tripwire.
#[test]
fn astc_support_transcodes_the_same_bytes() {
    let image = load_like_gltf(CompressedImageFormats::ASTC_LDR)
        .expect("UASTC transcodes to ASTC 4x4 losslessly — this is the path the mitigation buys");
    assert_eq!(
        image.texture_descriptor.mip_level_count, 5,
        "the fixture's whole mip chain must survive transcoding",
    );
}
