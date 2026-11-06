# Local draft: prepare collider mass before insertion

Subject: the game's environment physics; game state only. No external issue or message sent.

Avian 0.7's `ColliderBackendPlugin::on_insert` recomputes contact-shape mass even when
`ColliderMassProperties` is supplied. Convex shapes triangulate there, during exclusive ECS
application. Its later collider-mass update reads the shape again.

The local `Collider::with_mass_properties_cache` method prepares the shape's current mass at a
specified density. The existing backend reads through `mass_properties`; matching reads use the
cache. Shape mutation/replacement and changed scale invalidate it; other densities calculate
normally. Serialization omits the cache. The patch covers both dimension implementations without
changing the backend's initialization, scale, sensor or occupied-override contracts.

Retire this patch when the pinned engine supplies an insertion contract that consumes prepared
shape mass without recomputing it and preserves invalidation. The construction insertion and
mutation witness names the required behavior. See the vendored `OVERMATCH_PATCH.md` inventory.
