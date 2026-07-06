# lightyear_avian3d 0.28: rollback restore assumes EnlargedAabb was rolled back — false for child colliders

**Target:** lightyear_avian3d 0.28 · **Severity for us:** MEDIUM (moot after the ApplyPosToTransform fix; latent) · **Status:** unfiled

## Suggested title

restore_collider_tree_from_enlarged_aabbs rebuilds the BVH from un-restored child-collider AABBs

## Mechanism

`rollback_resources: true` rolls back `ContactGraph`/`ConstraintGraph`/`PhysicsIslands` and
rebuilds the collider BVH via `restore_collider_tree_from_enlarged_aabbs` +
`repair_missing_contact_pairs_from_restored_aabbs` (lightyear_avian3d-0.28.0/src/plugin.rs:
355-570). The restore's own comment says "The rollback just restored EnlargedAabb" — but
`prepare_rollback` only restores components carrying `PredictionHistory`, which is only attached
to Predicted/PreSpawned/etc. entities (lightyear_prediction-0.28.0/src/predicted_history.rs:
237-267). CHILD COLLIDERS (plain local children of the predicted root) have no history: their
`ColliderAabb`/`EnlargedAabb` — and hence the rebuilt tree leaves and the repair's pair
intersection — keep the abandoned (mispredicted) timeline's values through the restore. The
repair can therefore never resurrect a pair the abandoned timeline lost for AABB-disjointness,
which is exactly how such pairs die.

Additionally the restore path calls `set_proxy_aabb` + `refit_all` (plugin.rs:427-444) without
the `init_primitives_to_nodes_if_uninit` guard avian's native update paths use
(avian3d-0.7.0/src/collider_tree/update.rs:811, 941).

## Measured

Instrumented at tick level (SPIKE_CONTACT_PROBE): tree leaves and moved-set faithfully track the
(wrong, un-restored) component AABBs; contact re-forms at k=1 in 62/69 rollbacks where the
abandoned timeline still had the pair vs fails in 80/85 where it didn't. NOTE: in our codebase
the visible symptom was dominated by the ApplyPosToTransform poisoning (separate report) — with
that fixed, avian's per-tick AABB refresh self-heals this within a tick, so for us it is latent,
not damaging. It remains logically wrong upstream and will bite layouts where child-collider
poses genuinely diverge during prediction.

## Suggested upstream fix

Derive child-collider AABBs from the restored ROOT pose during restore (recompute child collider
Position/AABB from the rolled-back parent instead of trusting non-existent histories), and add
the init guard before `set_proxy_aabb`.

## Our workaround

None needed post-33cc4e4; documenting for upstream correctness. If it ever resurfaces, the
probe (`SPIKE_CONTACT_PROBE=1`) discriminates it in one run.
