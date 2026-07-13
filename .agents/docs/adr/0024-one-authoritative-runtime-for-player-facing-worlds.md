# Player-facing worlds use one authoritative Battle runtime

Overmatch's core product is official-server-hosted PvP, so the dedicated server is the sole authority for Battle rules and clients predict intent rather than outcomes. The shooting range launches that same authority runtime locally and connects the normal client; it does not revive a separate standalone gameplay path. Analytical tools such as armor inspection may drive the shared simulation directly because they are adapters for inquiry, not player-controlled Battle worlds.

## Consequences

- The client and dedicated server remain separate runtime roles over one shared simulation implementation.
- Local training hides server orchestration from the player but preserves authority, protocol, prediction, and replication behavior.
- Direct-simulation sandboxes and focused tests remain valid and must not be forced through networking.
- Player-hosted online Battles, listen-server behavior, and host migration are deferred rather than designed speculatively.
