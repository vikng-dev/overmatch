#!/usr/bin/env python3
"""camo_stripes.py — generate the shared `stripes` camouflage pattern.

    python3 scripts/visuals/camo_stripes.py <out.png>

A camouflage pattern is a MASK, not a picture: one palette colour's coverage per channel (R the
livery's first camouflage colour, G its second, B its third), linear, 0 where the base paint shows.
A livery recolours it (`assets/<tank>/<tank>.livery.ron`), so the same file serves any palette and
any vehicle. Channels never overlap: where the first colour covers, the second is cut away.

THE LOOK (ASSUMED, a design choice, not a reconstruction of any documented scheme): wavy bands
sprayed over the base coat, two interleaved families running diagonally, each band's width wandering
and the bands breaking up, with a soft sprayed edge. Every term is a sinusoid of an INTEGER number
of periods across the tile, so the pattern tiles seamlessly in both directions by construction.

Deterministic: a fixed seed, so the shipped file is reproducible from this script.
"""

from __future__ import annotations

import sys

import numpy as np
from PIL import Image

#: Texels per side. The tile spans the livery's `metres_per_repeat`.
SIZE = 1024
#: Fixed so the generated file is reproducible.
SEED = 1943
#: Half-width of the sprayed edge, in units of the stripe field (which runs -1..1). ASSUMED.
EDGE = 0.07


def periodic_noise(u, v, rng, terms, max_freq, amplitude):
    """A sum of `terms` sinusoids with integer frequencies up to `max_freq` on the unit torus."""
    field = np.zeros_like(u)
    for _ in range(terms):
        i, j = rng.integers(-max_freq, max_freq + 1, size=2)
        if i == 0 and j == 0:
            i = 1
        phase = rng.uniform(0.0, 2.0 * np.pi)
        weight = rng.uniform(0.4, 1.0) / np.hypot(i, j)
        field += weight * np.sin(2.0 * np.pi * (i * u + j * v) + phase)
    peak = np.max(np.abs(field))
    return amplitude * field / max(peak, 1e-9)


def stripe_family(u, v, rng, direction, threshold):
    """One colour's coverage: bands along `direction` (integer periods per tile), warped, with a
    wandering width and gaps, soft-edged."""
    warp = periodic_noise(u, v, rng, terms=6, max_freq=3, amplitude=1.6)
    detail = periodic_noise(u, v, rng, terms=8, max_freq=9, amplitude=0.35)
    band = np.sin(2.0 * np.pi * (direction[0] * u + direction[1] * v) + warp + detail)
    width = threshold + periodic_noise(u, v, rng, terms=5, max_freq=2, amplitude=0.25)
    gaps = periodic_noise(u, v, rng, terms=5, max_freq=2, amplitude=1.0)
    keep = np.clip((gaps + 0.45) / 0.12, 0.0, 1.0)
    edge = np.clip((band - width + EDGE) / (2.0 * EDGE), 0.0, 1.0)
    edge = edge * edge * (3.0 - 2.0 * edge)
    return edge * keep


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rng = np.random.default_rng(SEED)
    axis = (np.arange(SIZE, dtype=np.float64) + 0.5) / SIZE
    u, v = np.meshgrid(axis, axis)
    first = stripe_family(u, v, rng, direction=(3, 2), threshold=0.6)
    second = stripe_family(u + 0.37, v + 0.21, rng, direction=(3, 2), threshold=0.66)
    second = second * (1.0 - first)
    mask = np.stack([first, second, np.zeros_like(first)], axis=-1)
    Image.fromarray(np.round(mask * 255.0).astype(np.uint8), mode="RGB").save(sys.argv[1])
    covered = (first.mean(), second.mean())
    print(f"stripes ▸ {sys.argv[1]}: coverage {covered[0]:.2f} first, {covered[1]:.2f} second")
    return 0


if __name__ == "__main__":
    sys.exit(main())
