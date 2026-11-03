//! Required overlay, lobby and match-action presentation, admitted before those surfaces mount.

use crate::provenance::{Sourced, from_shipped_str};
use bevy::prelude::Color;

const FILE: &str = "assets/ui/overlay.ron";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Look {
    pub menu_scrim_srgba: Sourced<(u8, u8, u8, u8)>,
    pub connect_scrim_srgba: Sourced<[f32; 4]>,
    pub joining_scrim_srgba: Sourced<[f32; 4]>,
    pub action_button: ActionButton,
    pub leave_match: LeaveMatch,
    pub lobby: Lobby,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionButton {
    pub padding_x_px: Sourced<f32>,
    pub padding_y_px: Sourced<f32>,
    pub radius_px: Sourced<f32>,
    pub label_px: Sourced<f32>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LeaveMatch {
    pub bottom_percent: Sourced<f32>,
    pub button_srgba: Sourced<[f32; 4]>,
    pub button_hover_srgba: Sourced<[f32; 4]>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lobby {
    pub card_width_px: Sourced<f32>,
    pub card_max_height_percent: Sourced<f32>,
    pub card_padding_px: Sourced<f32>,
    pub card_radius_px: Sourced<f32>,
    pub card_row_gap_px: Sourced<f32>,
    pub list_row_gap_px: Sourced<f32>,
    pub footer_gap_px: Sourced<f32>,
    pub title_px: Sourced<f32>,
    pub row_px: Sourced<f32>,
    pub small_px: Sourced<f32>,
    pub row_gap_px: Sourced<f32>,
    pub row_padding_x_px: Sourced<f32>,
    pub row_padding_y_px: Sourced<f32>,
    pub row_radius_px: Sourced<f32>,
    pub retry_margin_px: Sourced<f32>,
    pub tick_width_px: Sourced<f32>,
    pub ping_width_px: Sourced<f32>,
    pub spread_width_px: Sourced<f32>,
    pub matches_width_px: Sourced<f32>,
    pub chevron_width_px: Sourced<f32>,
    pub scrim_srgba: Sourced<[f32; 4]>,
    pub row_hover_srgba: Sourced<[f32; 4]>,
    pub button_srgba: Sourced<[f32; 4]>,
    pub button_hover_srgba: Sourced<[f32; 4]>,
    pub button_disabled_srgba: Sourced<[f32; 4]>,
    pub warn_srgba: Sourced<[f32; 4]>,
}

pub(crate) fn get() -> &'static Look {
    static LOOK: std::sync::LazyLock<Look> = std::sync::LazyLock::new(|| {
        let look: Look = from_shipped_str(FILE, include_str!("../../assets/ui/overlay.ron"))
            .unwrap_or_else(|error| panic!("{error}"));
        let button = &look.action_button;
        let leave = &look.leave_match;
        let lobby = &look.lobby;
        for (field, size, positive) in [
            ("action_button.padding_x_px", button.padding_x_px.0, false),
            ("action_button.padding_y_px", button.padding_y_px.0, false),
            ("action_button.radius_px", button.radius_px.0, false),
            ("action_button.label_px", button.label_px.0, true),
            ("leave_match.bottom_percent", leave.bottom_percent.0, false),
            ("lobby.card_width_px", lobby.card_width_px.0, true),
            (
                "lobby.card_max_height_percent",
                lobby.card_max_height_percent.0,
                true,
            ),
            ("lobby.card_padding_px", lobby.card_padding_px.0, false),
            ("lobby.card_radius_px", lobby.card_radius_px.0, false),
            ("lobby.card_row_gap_px", lobby.card_row_gap_px.0, false),
            ("lobby.list_row_gap_px", lobby.list_row_gap_px.0, false),
            ("lobby.footer_gap_px", lobby.footer_gap_px.0, false),
            ("lobby.title_px", lobby.title_px.0, true),
            ("lobby.row_px", lobby.row_px.0, true),
            ("lobby.small_px", lobby.small_px.0, true),
            ("lobby.row_gap_px", lobby.row_gap_px.0, false),
            ("lobby.row_padding_x_px", lobby.row_padding_x_px.0, false),
            ("lobby.row_padding_y_px", lobby.row_padding_y_px.0, false),
            ("lobby.row_radius_px", lobby.row_radius_px.0, false),
            ("lobby.retry_margin_px", lobby.retry_margin_px.0, false),
            ("lobby.tick_width_px", lobby.tick_width_px.0, true),
            ("lobby.ping_width_px", lobby.ping_width_px.0, true),
            ("lobby.spread_width_px", lobby.spread_width_px.0, true),
            ("lobby.matches_width_px", lobby.matches_width_px.0, true),
            ("lobby.chevron_width_px", lobby.chevron_width_px.0, true),
        ] {
            assert!(
                size.is_finite() && size >= 0.0 && (!positive || size > 0.0),
                "{FILE}: {field} needs a finite size, positive for drawn extents and nonnegative for spacing; author a valid presentation size"
            );
        }
        for (field, percent) in [
            ("leave_match.bottom_percent", leave.bottom_percent.0),
            (
                "lobby.card_max_height_percent",
                lobby.card_max_height_percent.0,
            ),
        ] {
            assert!(
                percent <= 100.0,
                "{FILE}: {field} must be at most 100; author a viewport percentage"
            );
        }
        for (field, channels) in [
            ("connect_scrim_srgba", look.connect_scrim_srgba.0),
            ("joining_scrim_srgba", look.joining_scrim_srgba.0),
            ("leave_match.button_srgba", leave.button_srgba.0),
            ("leave_match.button_hover_srgba", leave.button_hover_srgba.0),
            ("lobby.scrim_srgba", lobby.scrim_srgba.0),
            ("lobby.row_hover_srgba", lobby.row_hover_srgba.0),
            ("lobby.button_srgba", lobby.button_srgba.0),
            ("lobby.button_hover_srgba", lobby.button_hover_srgba.0),
            ("lobby.button_disabled_srgba", lobby.button_disabled_srgba.0),
            ("lobby.warn_srgba", lobby.warn_srgba.0),
        ] {
            assert!(
                channels
                    .iter()
                    .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel)),
                "{FILE}: {field} needs finite sRGBA channels in [0, 1]; author a valid colour"
            );
        }
        look
    });
    &LOOK
}

pub(crate) fn color(value: Sourced<[f32; 4]>) -> Color {
    let [r, g, b, a] = value.0;
    Color::srgba(r, g, b, a)
}
