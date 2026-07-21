//! Server-side combat disclosure policy and the public tank-life summary.

use bevy::prelude::*;
use bevy_replicon::prelude::{AppVisibilityExt, AuthorizedClient, VisibilityFilter};
use serde::{Deserialize, Serialize};

use super::protocol::{NetCrew, NetTank, NetTrackGripAnchor};
use crate::damage::{KnockoutReason, TankKnockedOut};
use crate::tank::{TankServos, WeaponGate};
use crate::track::sim::TrackGripElements;

/// Public, server-authored tank-life fact. Detailed damage and ammunition state stay owner-only.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetTankStatus {
    #[default]
    Active,
    KnockedOut,
    CookedOff,
}

impl NetTankStatus {
    fn from_knockout(knocked_out: Option<&TankKnockedOut>) -> Self {
        match knocked_out.map(|knocked_out| knocked_out.reason) {
            None => Self::Active,
            Some(KnockoutReason::CrewLoss) => Self::KnockedOut,
            Some(KnockoutReason::Cookoff) => Self::CookedOff,
        }
    }

    fn knockout_reason(self) -> Option<KnockoutReason> {
        match self {
            Self::Active => None,
            Self::KnockedOut => Some(KnockoutReason::CrewLoss),
            Self::CookedOff => Some(KnockoutReason::Cookoff),
        }
    }
}

/// Immutable policy attached at tank spawn. It exposes internal combat snapshots only to `owner`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(immutable)]
pub(super) struct CombatDisclosure {
    pub(super) owner: Option<Entity>,
}

impl CombatDisclosure {
    pub(super) const fn owner(owner: Entity) -> Self {
        Self { owner: Some(owner) }
    }

    pub(super) const fn hidden() -> Self {
        Self { owner: None }
    }
}

impl VisibilityFilter for CombatDisclosure {
    type ClientComponent = AuthorizedClient;
    type Scope = (
        NetCrew,
        WeaponGate,
        TankServos,
        NetTrackGripAnchor,
        TrackGripElements,
    );

    fn is_visible(&self, client: Entity, authorized: Option<&AuthorizedClient>) -> bool {
        authorized.is_some() && self.owner == Some(client)
    }
}

/// Registers the authority-only disclosure policy and public status publisher.
///
/// Installed with the server's player and bot spawn constructors, which attach both the filter and
/// public status in the initial replicated bundle.
pub(super) fn install_server(app: &mut App) {
    app.add_visibility_filter::<CombatDisclosure>()
        .add_systems(FixedPostUpdate, publish_net_tank_status);
}

fn publish_net_tank_status(
    mut tanks: Query<(&mut NetTankStatus, Option<&TankKnockedOut>), With<NetTank>>,
) {
    for (mut status, knocked_out) in &mut tanks {
        status.set_if_neq(NetTankStatus::from_knockout(knocked_out));
    }
}

/// Realizes the public life fact without requiring a remote client to receive private internals.
pub(super) fn apply_net_tank_status(
    mut tanks: Query<
        (Entity, &NetTankStatus, Option<&mut TankKnockedOut>),
        With<lightyear::prelude::client::Remote>,
    >,
    mut commands: Commands,
) {
    for (tank, status, existing) in &mut tanks {
        match (status.knockout_reason(), existing) {
            (Some(reason), None) => {
                commands.entity(tank).insert(TankKnockedOut { reason });
            }
            (Some(reason), Some(mut existing)) if existing.reason != reason => {
                existing.reason = reason;
            }
            (None, Some(_)) => {
                commands.entity(tank).remove::<TankKnockedOut>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy_replicon::prelude::VisibilityFilter;
    use lightyear::prelude::client::Remote;

    use super::*;

    #[test]
    fn owner_disclosure_requires_the_authorized_owner_link() {
        let owner = Entity::from_raw_u32(4).unwrap();
        let observer = Entity::from_raw_u32(5).unwrap();
        let disclosure = CombatDisclosure::owner(owner);
        let authorized = AuthorizedClient;

        assert!(disclosure.is_visible(owner, Some(&authorized)));
        assert!(!disclosure.is_visible(observer, Some(&authorized)));
        assert!(!disclosure.is_visible(owner, None));
    }

    #[test]
    fn ownerless_bot_policy_hides_private_combat_from_an_authorized_client() {
        let client = Entity::from_raw_u32(4).unwrap();
        assert!(!CombatDisclosure::hidden().is_visible(client, Some(&AuthorizedClient)));
    }

    #[test]
    fn public_status_drives_remote_knockout_without_a_crew_snapshot() {
        let mut world = World::new();
        let active = world.spawn((Remote, NetTankStatus::Active)).id();
        let cooked = world.spawn((Remote, NetTankStatus::CookedOff)).id();
        let revived = world
            .spawn((
                Remote,
                NetTankStatus::Active,
                TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                },
            ))
            .id();
        let cookoff_upgrade = world
            .spawn((
                Remote,
                NetTankStatus::CookedOff,
                TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                },
            ))
            .id();

        world.run_system_once(apply_net_tank_status).unwrap();

        assert!(world.get::<TankKnockedOut>(active).is_none());
        assert_eq!(
            world.get::<TankKnockedOut>(cooked).unwrap().reason,
            KnockoutReason::Cookoff,
        );
        assert!(world.get::<TankKnockedOut>(revived).is_none());
        assert_eq!(
            world.get::<TankKnockedOut>(cookoff_upgrade).unwrap().reason,
            KnockoutReason::Cookoff,
        );
    }

    #[test]
    fn authority_status_keeps_the_knockout_cause_public() {
        assert_eq!(
            NetTankStatus::from_knockout(Some(&TankKnockedOut {
                reason: KnockoutReason::CrewLoss,
            })),
            NetTankStatus::KnockedOut,
        );
        assert_eq!(
            NetTankStatus::from_knockout(Some(&TankKnockedOut {
                reason: KnockoutReason::Cookoff,
            })),
            NetTankStatus::CookedOff,
        );
    }
}
