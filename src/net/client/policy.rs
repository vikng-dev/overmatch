//! Required reconnect and joining-screen policy, admitted before the client starts.

use crate::provenance::Sourced;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClientPolicy {
    pub reconnect_give_up_s: Sourced<f64>,
    pub reconnect_base_s: Sourced<f64>,
    pub reconnect_step_s: Sourced<f64>,
    pub reconnect_cap_s: Sourced<f64>,
    pub unanswered_after_attempts: Sourced<u32>,
}

pub(super) fn client_policy() -> &'static ClientPolicy {
    static POLICY: std::sync::LazyLock<ClientPolicy> = std::sync::LazyLock::new(|| {
        let policy: ClientPolicy = crate::provenance::from_shipped_str(
            "assets/net/client.ron",
            include_str!("../../../assets/net/client.ron"),
        )
        .expect("assets/net/client.ron: author every reconnect policy field with provenance");
        for (field, value) in [
            ("reconnect_give_up_s", policy.reconnect_give_up_s.0),
            ("reconnect_base_s", policy.reconnect_base_s.0),
            ("reconnect_step_s", policy.reconnect_step_s.0),
            ("reconnect_cap_s", policy.reconnect_cap_s.0),
        ] {
            assert!(
                value.is_finite() && value > 0.0,
                "assets/net/client.ron: {field} must be finite and positive; author a retry duration"
            );
        }
        assert!(
            policy.reconnect_cap_s.0 >= policy.reconnect_base_s.0,
            "assets/net/client.ron: reconnect_cap_s must cover reconnect_base_s; raise the cap or lower the base"
        );
        assert!(
            policy.unanswered_after_attempts.0 > 0,
            "assets/net/client.ron: unanswered_after_attempts must be positive; allow an initial connection attempt"
        );
        policy
    });
    &POLICY
}
