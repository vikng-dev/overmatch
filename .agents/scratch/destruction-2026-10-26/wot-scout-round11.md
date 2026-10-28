# WoT scout round 11: how hull damage marks are rendered

2026-10-28. Received from the `wot` session in reply to spall's round-11 question. Design input only:
mechanisms and pointers, never numbers or code. Source labels as given: DATA, ENGINE or SCRIPT, plus R
(read) or INFERRED. Sources are two loose configs in the client slice, `res/gpu_decals_config.xml` and
`res/decals_config.xml`, plus round 9.

## Summary

Hull damage marks are **GPU deferred decals** projected along the hit trace and written into **one
shared, clustered, block-allocated decal atlas**. There are no per-kind forward sprites.
- `decals_config.xml` names two decal classes, `GBufferDecals` and `OtherDecals` [DATA, R].
- Its trace parameters (`maxTraceLength`, `maxTraceSegmentLength`, `maxTraceOffset`,
  `maxTraceAdditionAngle`) are the ray-projection controls, matching round 9's segStart→segEnd
  placement.

## 1. Textures [DATA, R: gpu_decals_config.xml]

One atlas with one BC7 buffer per channel:
- **Albedo**;
- **Normal**;
- **GlossMetallic** (combined);
- a **4th BC7 buffer**, unnamed: likely height, alpha or an emissive mask [INFERRED];
- **Temperature**, a single BC4 channel.

The atlas attributes are `atlases`, `blockCapacity`, `blocksCount`, `clusterization`, `capacity`,
`firstMip`, `gradients` and `farPlane`. Every mark kind is a region in this one atlas. The per-kind region
names live in packed shot and vehicle data, which was not unpacked [limit].

## 2. Depth on metal [DATA R + INFERRED]

- The decal is a G-buffer decal over the surface: mainly the **normal** (the dent or crater) and
  **GlossMetallic** (bare metal, gloss and metallic up where paint is gone).
- No parallax or POM token appears, so depth comes from the normal map plus gloss and metallic, **not
  POM** [INFERRED].
- **Scorch** is the single **Temperature** channel driving a **gradient** ramp (a LUT): one scalar for
  black soot through to a glowing edge.
- The bare-metal ring and the black centre are authored into the atlas region.
- The decal shader's exact write mask and blend operation were not pulled [limit].

## 3. Adapting [mixed]

- **Oblique hit → ellipse:** free from the trace projection. It is bounded by `minWidth`, `maxWidth`,
  `minHeight`, `maxHeight` and `maxTraceAdditionAngle` [DATA, R].
- **Size by calibre and scratch length:** probably carried by the shot data (`stickerID`) [INFERRED,
  unconfirmed].
- **Random rotation or variant:** not in these configs [limit].

## 4. Blend with camo and wear [INFERRED from the architecture]

A deferred G-buffer decal modulates the underlying albedo, normal, gloss and metallic, so camo and wear
show through where the decal's mask is below full. Paint versus bare metal is expressed by the decal
writing higher metallic and gloss.

## 5. Cheap tricks worth copying

- **One clustered decal atlas** for every damage mark, with tile and cluster culling and a far-plane cull.
  VRAM and draw cost stay bounded however many marks accumulate.
- **One BC4 Temperature channel plus a gradient LUT** for soot and heat, instead of an RGB scorch texture.
- **Hard quality tiers** (MAX, MEDIUM, LOW, OFF) that scale `maxVisibleDecals`, with a downsampled decal
  buffer (`matTypeDownsample`, `decalsBufferEnabled`), `lodBias` and a `lifetimeMultiplier` fade.
- **Deferred G-buffer decals** inherit the scene's lighting and shadow for free.

## Not established this round (offered: needs the exe or packed data unpacked)

- The per-kind atlas regions and texture paths.
- The decal shader's write mask and blend operation.
- Whether calibre or graze scales the size, and whether a random variant is used.
