//! What the analog stage is asked to do: the flavours, the anti-alias choice,
//! and [`AnalogSettings`] with its clamping and defaults.

use serde::{Deserialize, Serialize};

use super::oversample::{anti_alias_plan, AntiAlias};

/// Which character to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnalogFlavour {
    /// A 12AX7 triode stage (Koren model): mostly 2nd harmonic (even).
    #[default]
    WarmTriode,
    /// Class-A push-pull pair of 2A3 power triodes: odd harmonics, a little
    /// even from imperfect matching.
    PushPull,
    /// Symmetric soft curve: odd harmonics only, like a transistor stage.
    SolidState,
    /// A near-hard symmetric clip: clean below the knee, harsh above it.
    HardTransistor,
    /// 12AT7 / ECC81: medium-high mu small-signal triode.
    #[serde(rename = "tube_12at7")]
    Tube12at7,
    /// 12AU7 / ECC82: low-mu, clean small-signal triode.
    #[serde(rename = "tube_12au7")]
    Tube12au7,
    /// 6SN7: low-mu octal triode.
    #[serde(rename = "tube_6sn7")]
    Tube6sn7,
    /// 6DJ8 / ECC88: medium-mu low-noise triode.
    #[serde(rename = "tube_6dj8")]
    Tube6dj8,
    /// 300B: single-ended directly-heated power triode.
    #[serde(rename = "tube_300b")]
    Tube300b,
    /// 2A3: single-ended directly-heated power triode.
    #[serde(rename = "tube_2a3")]
    Tube2a3,
    /// 6SL7GT: high-mu octal triode.
    #[serde(rename = "tube_6sl7")]
    Tube6sl7,
    /// 12AY7: low-noise medium-mu triode.
    #[serde(rename = "tube_12ay7")]
    Tube12ay7,
    /// 12AX7A (Sylvania fit): a second 12AX7 flavour.
    #[serde(rename = "tube_12ax7a")]
    Tube12ax7a,
    /// EL84: single-ended class-A pentode.
    #[serde(rename = "tube_el84")]
    TubeEl84,
    /// EL34 pair, push-pull class AB.
    #[serde(rename = "push_pull_el34")]
    PushPullEl34,
    /// 6L6GC pair, push-pull class AB.
    #[serde(rename = "push_pull_6l6gc")]
    PushPull6l6gc,
    /// KT88 pair, push-pull class AB.
    #[serde(rename = "push_pull_kt88")]
    PushPullKt88,
    /// A JFET stage: square-law, 2nd-harmonic warmth.
    Jfet,
    /// A silicon diode-pair clipper: symmetric soft clip.
    SiliconDiode,
    /// Germanium against silicon diodes: asymmetric clip.
    GermaniumDiode,
    /// No distortion curve: only the sag and the transformer colour.
    IronSag,
}

/// How to keep aliasing out of the audible band. `Auto` follows the sample
/// rate (see [`anti_alias_plan`]); the others force a plan, for A/B listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AntiAliasChoice {
    #[default]
    Auto,
    X1,
    X1Adaa,
    X2,
    X2Adaa,
    X4,
    X4Adaa,
}

impl AntiAliasChoice {
    /// The plan this choice means at `sample_rate`.
    pub fn resolve(self, sample_rate: u32) -> AntiAlias {
        let (factor, adaa) = match self {
            Self::Auto => return anti_alias_plan(sample_rate),
            Self::X1 => (1, false),
            Self::X1Adaa => (1, true),
            Self::X2 => (2, false),
            Self::X2Adaa => (2, true),
            Self::X4 => (4, false),
            Self::X4Adaa => (4, true),
        };
        AntiAlias { factor, adaa }
    }

    pub(super) fn from_plan(plan: AntiAlias) -> Self {
        match (plan.factor, plan.adaa) {
            (1, false) => Self::X1,
            (1, true) => Self::X1Adaa,
            (2, false) => Self::X2,
            (2, true) => Self::X2Adaa,
            (_, false) => Self::X4,
            (_, true) => Self::X4Adaa,
        }
    }
}

/// User-facing settings; persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalogSettings {
    pub enabled: bool,
    pub flavour: AnalogFlavour,
    /// 0..=1: how hard the signal is pushed into the curve.
    pub drive: f32,
    /// 0..=1: parallel blend of the processed signal (1 = fully processed).
    pub mix: f32,
    /// Output trim in dB, -6..=6.
    pub output_db: f32,
    /// Match the processed level to the dry level (at a -12 dBFS reference).
    pub auto_gain: bool,
    /// Anti-aliasing plan; `auto` follows the sample rate.
    pub antialias: AntiAliasChoice,
    /// 0..=1: power-supply sag. Loud passages lower the stage's headroom and
    /// gain a little, and it recovers over about a tenth of a second.
    pub sag: f32,
    /// 0..=1: output-transformer colour. The low bass saturates as the level
    /// rises, adding bass harmonics; mids and highs are untouched.
    pub transformer: f32,
}

impl Default for AnalogSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            flavour: AnalogFlavour::WarmTriode,
            drive: 0.4,
            mix: 0.4,
            output_db: 0.0,
            auto_gain: true,
            antialias: AntiAliasChoice::Auto,
            sag: 0.3,
            transformer: 0.3,
        }
    }
}

impl AnalogSettings {
    /// Pull every value into its allowed range.
    pub fn clamped(mut self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.drive = fin(self.drive, 0.4).clamp(0.0, 1.0);
        self.mix = fin(self.mix, 0.4).clamp(0.0, 1.0);
        self.output_db = fin(self.output_db, 0.0).clamp(-6.0, 6.0);
        self.sag = fin(self.sag, 0.3).clamp(0.0, 1.0);
        self.transformer = fin(self.transformer, 0.3).clamp(0.0, 1.0);
        self
    }
}

#[cfg(test)]
pub(super) const ALL_FLAVOURS: [AnalogFlavour; 21] = [
    AnalogFlavour::WarmTriode,
    AnalogFlavour::PushPull,
    AnalogFlavour::SolidState,
    AnalogFlavour::HardTransistor,
    AnalogFlavour::Tube12at7,
    AnalogFlavour::Tube12au7,
    AnalogFlavour::Tube6sn7,
    AnalogFlavour::Tube6dj8,
    AnalogFlavour::Tube300b,
    AnalogFlavour::Tube2a3,
    AnalogFlavour::Tube6sl7,
    AnalogFlavour::Tube12ay7,
    AnalogFlavour::Tube12ax7a,
    AnalogFlavour::TubeEl84,
    AnalogFlavour::PushPullEl34,
    AnalogFlavour::PushPull6l6gc,
    AnalogFlavour::PushPullKt88,
    AnalogFlavour::Jfet,
    AnalogFlavour::SiliconDiode,
    AnalogFlavour::GermaniumDiode,
    AnalogFlavour::IronSag,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_clamped_and_defaults_fill_gaps() {
        let s = AnalogSettings {
            drive: 5.0,
            mix: -1.0,
            output_db: 40.0,
            ..Default::default()
        }
        .clamped();
        assert_eq!((s.drive, s.mix, s.output_db), (1.0, 0.0, 6.0));
        assert_eq!(
            AnalogSettings {
                drive: f32::NAN,
                ..Default::default()
            }
            .clamped()
            .drive,
            0.4
        );
        let parsed: AnalogSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(parsed.enabled && parsed.flavour == AnalogFlavour::WarmTriode && parsed.mix == 0.4);
        let json = serde_json::to_string(&AnalogSettings::default()).unwrap();
        assert!(json.contains("\"warm_triode\""), "{json}");
    }

    #[test]
    fn anti_alias_choices_map_to_plans_and_serialize_by_name() {
        assert_eq!(
            AntiAliasChoice::Auto.resolve(44_100),
            AntiAlias {
                factor: 4,
                adaa: true
            }
        );
        assert_eq!(
            AntiAliasChoice::Auto.resolve(192_000),
            AntiAlias {
                factor: 1,
                adaa: true
            }
        );
        assert_eq!(
            AntiAliasChoice::X2.resolve(44_100),
            AntiAlias {
                factor: 2,
                adaa: false
            }
        );
        assert_eq!(
            AntiAliasChoice::X4Adaa.resolve(192_000),
            AntiAlias {
                factor: 4,
                adaa: true
            }
        );
        for (c, name) in [
            (AntiAliasChoice::Auto, "auto"),
            (AntiAliasChoice::X1, "x1"),
            (AntiAliasChoice::X1Adaa, "x1_adaa"),
            (AntiAliasChoice::X2Adaa, "x2_adaa"),
            (AntiAliasChoice::X4, "x4"),
        ] {
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{name}\""));
        }
        let plan = AntiAlias {
            factor: 2,
            adaa: true,
        };
        assert_eq!(AntiAliasChoice::from_plan(plan).resolve(48_000), plan);
    }

    #[test]
    fn colour_settings_are_clamped_and_default_in() {
        let s = AnalogSettings {
            sag: 3.0,
            transformer: -1.0,
            ..Default::default()
        }
        .clamped();
        assert_eq!((s.sag, s.transformer), (1.0, 0.0));
        let parsed: AnalogSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!((parsed.sag, parsed.transformer), (0.3, 0.3));
    }
}
