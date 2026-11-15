# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.20.1 crate, unchanged: no source change is carried
yet. Each patch, when it lands, gets a section here and an `OVERMATCH PATCH` tag at its sites.

Produced from `https://static.crates.io/crates/bevy_pbr/bevy_pbr-0.20.1.crate`, whose sha256
`1138d6ed9bbcc43a4d83878062869c20d4292f1112b177acfc1a20cf6ae14f3e` matches the crates.io index's
`cksum`, by `tar -xzf bevy_pbr-0.20.1.crate -C vendor/bevy_pbr-0.20.1-overmatch --strip-components=1
--exclude='*/Cargo.lock' --exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record
are omitted; neither is read for a path dependency.
