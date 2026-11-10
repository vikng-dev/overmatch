//! GUARD for the vendored moved-list index (`vendor/avian3d-0.7.0-child-pose`,
//! `OVERMATCH_PATCH.md`).
//!
//! The invariant: a collider tree's moved list and `MovedProxies` hold the proxies, in the order,
//! that stock Avian 0.7.0's scanning removal leaves: a removed proxy's place is taken by the last
//! one listed, and nothing else moves. Who relies on it: the broad phase, which makes its pairs in
//! `MovedProxies` order, and through the pair order the solver; and tree optimization, which
//! reinserts the tree's moved proxies in list order. A removal that took the wrong entry, or left
//! a removed proxy listed, changes both.
//!
//! The model below is the stock form: one list per structure, a removal by a scan from the front
//! and `swap_remove`. Counts, bounds and the generator's constants are fixture data.

use avian3d::collider_tree::{
    ColliderTree, ColliderTreeProxy, ColliderTreeProxyKey, ColliderTreeType, MovedProxies, ProxyId,
};
use avian3d::prelude::*;
use bevy::prelude::*;

/// A tree and `MovedProxies` beside the stock scanning form of both lists.
#[derive(Default)]
struct Lists {
    tree: ColliderTree,
    moved: MovedProxies,
    stock_tree: Vec<ProxyId>,
    stock_moved: Vec<ColliderTreeProxyKey>,
    live: Vec<ProxyId>,
}

fn key(id: ProxyId) -> ColliderTreeProxyKey {
    ColliderTreeProxyKey::new(id, ColliderTreeType::Dynamic)
}

impl Lists {
    /// As Avian's `add_to_tree_on`: a new proxy, marked moved in both lists.
    fn add(&mut self, at: f32) {
        let aabb = ColliderAabb::new(Vec3::splat(at), Vec3::ONE);
        let id = self.tree.add_proxy(
            aabb.into(),
            ColliderTreeProxy {
                collider: Entity::PLACEHOLDER,
                body: None,
                layers: CollisionLayers::default(),
                flags: default(),
            },
        );
        self.stock_tree.push(id);
        if self.moved.insert(key(id)) {
            self.stock_moved.push(key(id));
        }
        self.live.push(id);
    }

    /// As Avian's `remove_from_tree_on`: the proxy leaves the tree and both lists.
    fn remove(&mut self, index: usize) {
        let id = self.live.remove(index);
        self.tree
            .remove_proxy(id)
            .expect("fixture: a live proxy is in the tree");
        self.moved.remove(&key(id));
        if let Some(position) = self.stock_tree.iter().position(|listed| *listed == id) {
            self.stock_tree.swap_remove(position);
        }
        if let Some(position) = self
            .stock_moved
            .iter()
            .position(|listed| *listed == key(id))
        {
            self.stock_moved.swap_remove(position);
        }
    }

    /// As Avian's `update_tree`: a live proxy whose bounds moved, listed once.
    fn mark(&mut self, index: usize) {
        let id = self.live[index];
        if self.moved.insert(key(id)) {
            self.tree.mark_moved(id);
            self.stock_tree.push(id);
            self.stock_moved.push(key(id));
        }
    }

    /// As Avian's `clear_moved_proxies`, at the end of a physics step.
    fn clear(&mut self) {
        self.tree.clear_moved();
        self.moved.clear();
        self.stock_tree.clear();
        self.stock_moved.clear();
    }

    fn assert_stock(&self, when: &str) {
        assert_eq!(self.tree.moved_proxies(), self.stock_tree, "{when}");
        assert_eq!(self.moved.proxies(), self.stock_moved, "{when}");
    }
}

/// One tick that rehomes every collider of a broken construction, then drops the stale pairs of
/// half of them: each proxy is removed while unlisted and added again, then every second one is
/// removed while listed and added again. Both lists match the stock form after every step.
#[test]
fn a_tick_of_rehomes_and_toggles_leaves_the_stock_moved_lists() {
    const PROXIES: usize = 2_000;
    let mut lists = Lists::default();
    for i in 0..PROXIES {
        lists.add(i as f32);
    }
    lists.clear();
    for i in 0..PROXIES {
        // The oldest live proxy, unlisted since the clear; its replacement joins the back.
        lists.remove(0);
        lists.add(i as f32);
        lists.assert_stock("after a rehome");
    }
    for i in 0..PROXIES / 2 {
        // Listed this tick, anywhere in the lists.
        lists.remove(i);
        lists.add(i as f32);
        lists.assert_stock("after a toggle");
    }
    assert_eq!(lists.tree.moved_proxies().len(), PROXIES);
}

/// Adds, removals of listed and unlisted proxies, marks and clears in a generated order, with
/// reused proxy ids: both lists match the stock form after every operation.
#[test]
fn generated_moves_leave_the_stock_moved_lists() {
    let mut lists = Lists::default();
    // Fixture: a 64-bit LCG (Knuth's MMIX constants); the high bits choose each operation.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as usize) % bound
    };
    for step in 0..20_000 {
        match next(16) {
            0 => lists.clear(),
            1..=6 => lists.add(next(64) as f32),
            7..=11 if !lists.live.is_empty() => lists.remove(next(lists.live.len())),
            _ if !lists.live.is_empty() => lists.mark(next(lists.live.len())),
            _ => lists.add(next(64) as f32),
        }
        lists.assert_stock(&format!("after step {step}"));
    }
}

/// A tree lists a moved proxy once: a second entry would stay listed after the proxy's removal,
/// naming a freed or reused id to tree optimization.
#[test]
#[should_panic(expected = "marked moved twice")]
fn marking_a_listed_proxy_again_is_refused() {
    let mut lists = Lists::default();
    lists.add(0.0);
    let id = lists.live[0];
    lists.tree.mark_moved(id);
}
