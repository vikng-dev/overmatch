//! A round's fuze as authored, in the game's damage model: where it sits, what functions it, and
//! the named modes the crew picks among, each with its delay. The march runs it
//! ([`crate::ballistics::burst::Fuze`]); a fired round carries one mode's delay in its burst
//! ([`crate::tank::WeaponRound::in_mode`]), and the fire event names that mode by its index here.
//!
//! Units: the arming spend reference-mm, the sensitivity N (the force law's dE/dx at entry, J/m),
//! the delay s.

use crate::provenance::Sourced;
use serde::Deserialize;

/// Where the fuze sits in the round. Data for now: no law reads it yet.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub(crate) enum Position {
    Nose,
    Base,
}

/// What functions the fuze.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub(crate) enum Trigger {
    /// Functions on leaving a body the round spent at least `arming_spend` reference-mm in; a
    /// body met before the burst restarts the delay.
    SpendExit { arming_spend: f32 },
    /// Functions at contact with a body whose resisting force on the round reaches
    /// `sensitivity_n` newtons; the ground's is unbounded. Nothing restarts the delay.
    Impact { sensitivity_n: f32 },
}

/// One mode the crew can pick: its name, drawn on the HUD, and its delay (s).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mode {
    pub name: String,
    pub delay_s: f32,
}

/// A fuze block resolved and validated: modes nonempty, at most 256 (a mode index is a `u8` on
/// the wire), names unique, non-blank and drawable whole by the HUD font, delays and the trigger's
/// threshold finite and not negative, `default_mode` an index into `modes`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spec {
    pub position: Position,
    pub trigger: Trigger,
    pub modes: Vec<Mode>,
    pub default_mode: u8,
}

impl Spec {
    /// The delay (s) of mode `index`; `None` for an index the fuze does not list.
    pub(crate) fn delay_s(&self, index: u8) -> Option<f32> {
        self.modes.get(usize::from(index)).map(|mode| mode.delay_s)
    }

    /// The default mode's delay (s).
    pub(crate) fn default_delay_s(&self) -> f32 {
        self.modes[usize::from(self.default_mode)].delay_s
    }
}

/// A fuze block as authored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    position: Sourced<Position>,
    trigger: Sourced<Trigger>,
    modes: Vec<ModeFile>,
    default_mode: Sourced<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeFile {
    name: Sourced<String>,
    delay_s: Sourced<f32>,
}

impl File {
    /// The validated [`Spec`], or what is wrong with the block.
    pub(super) fn resolve(self) -> Result<Spec, String> {
        let (field, threshold) = match self.trigger.0 {
            Trigger::SpendExit { arming_spend } => ("arming_spend", arming_spend),
            Trigger::Impact { sensitivity_n } => ("sensitivity_n", sensitivity_n),
        };
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(format!(
                "{field} must be finite and not negative (got {threshold})"
            ));
        }
        if self.modes.is_empty() || self.modes.len() > usize::from(u8::MAX) + 1 {
            return Err(format!(
                "{} modes listed; a fuze lists 1 to 256, its mode index travels as a u8",
                self.modes.len()
            ));
        }
        let mut modes: Vec<Mode> = Vec::with_capacity(self.modes.len());
        for mode in self.modes {
            let (name, delay_s) = (mode.name.0, mode.delay_s.0);
            if name.trim().is_empty() || crate::ui_font::drawable(&name).as_deref() != Some(&name) {
                return Err(format!(
                    "mode name {name:?} must be non-blank and drawn whole by the HUD font"
                ));
            }
            if modes.iter().any(|m| m.name == name) {
                return Err(format!("mode {name:?} is listed twice"));
            }
            if !delay_s.is_finite() || delay_s < 0.0 {
                return Err(format!(
                    "mode {name:?}: delay_s must be finite and not negative (got {delay_s})"
                ));
            }
            modes.push(Mode { name, delay_s });
        }
        let default_mode = modes
            .iter()
            .position(|m| m.name == self.default_mode.0)
            .ok_or_else(|| {
                format!(
                    "default_mode {:?} names no listed mode",
                    self.default_mode.0
                )
            })?;
        Ok(Spec {
            position: self.position.0,
            trigger: self.trigger.0,
            modes,
            default_mode: u8::try_from(default_mode).expect("at most 256 modes, checked above"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(modes: &str, default: &str, trigger: &str) -> Result<Spec, String> {
        ron::from_str::<File>(&format!(
            r#"(position: (Nose, Tuned("fixture")),
            trigger: ({trigger}, Tuned("fixture")), modes: [{modes}],
            default_mode: ({default:?}, Tuned("fixture")))"#
        ))
        .map_err(|e| e.to_string())?
        .resolve()
    }

    const INSTANT: &str =
        r#"(name: ("instant", Tuned("fixture")), delay_s: (0.0, Tuned("fixture")))"#;
    const DELAY: &str = r#"(name: ("delay", Tuned("fixture")), delay_s: (0.1, Tuned("fixture")))"#;

    /// The game's damage model: a fuze block that resolves names its default by index into its
    /// ordered modes, and one that would let the crew pick a mode the server or the HUD cannot
    /// read is refused at load, never defaulted. The fire event's `u8` mode index, the server's
    /// bound check on it and the HUD's mode name all rely on these: an empty list, a repeated or
    /// undrawable name, a default naming no mode, or a negative or non-finite number.
    #[test]
    fn a_fuze_block_resolves_or_is_refused_at_load() {
        let spec = parse(
            &format!("{INSTANT},{DELAY}"),
            "delay",
            "Impact(sensitivity_n: 30.0)",
        )
        .expect("a valid block resolves");
        assert_eq!(spec.default_mode, 1);
        assert_eq!(spec.default_delay_s(), 0.1);
        assert_eq!(spec.delay_s(0), Some(0.0));
        assert_eq!(spec.delay_s(2), None, "an unlisted index has no delay");
        let undrawable = INSTANT.replace("instant", "inst\u{0101}nt");
        for (modes, default, trigger) in [
            (String::new(), "instant", "Impact(sensitivity_n: 30.0)"),
            (
                format!("{INSTANT},{INSTANT}"),
                "instant",
                "Impact(sensitivity_n: 30.0)",
            ),
            (undrawable, "inst\u{0101}nt", "Impact(sensitivity_n: 30.0)"),
            (
                INSTANT.replace("\"instant\"", "\" \""),
                " ",
                "Impact(sensitivity_n: 30.0)",
            ),
            (INSTANT.to_owned(), "absent", "Impact(sensitivity_n: 30.0)"),
            (INSTANT.to_owned(), "instant", "Impact(sensitivity_n: NaN)"),
            (INSTANT.to_owned(), "instant", "Impact(sensitivity_n: -1.0)"),
            (
                INSTANT.to_owned(),
                "instant",
                "SpendExit(arming_spend: inf)",
            ),
            (
                INSTANT.replace("0.0", "-0.1"),
                "instant",
                "SpendExit(arming_spend: 35.0)",
            ),
        ] {
            assert!(
                parse(&modes, default, trigger).is_err(),
                "refused: modes [{modes}], default {default:?}, trigger {trigger}"
            );
        }
    }
}
