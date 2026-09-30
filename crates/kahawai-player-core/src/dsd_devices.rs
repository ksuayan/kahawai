//! DACs known to decode DSD-over-PCM (DoP), so "Auto" DSD handling can use
//! native playback for them and stay on the safe FLAC conversion elsewhere.
//!
//! Being able to run at 176.4 kHz is not enough: a DAC that accepts the
//! rate but doesn't recognise the DoP markers plays loud noise. So Auto only
//! goes native for devices listed here or confirmed by the user in Settings.

/// Output-device names (matched case-insensitively as a substring, ignoring
/// surrounding whitespace) that are known to handle DoP.
pub const BUILT_IN: &[&str] = &["FIIO K15"];

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// True when `device` is a built-in known DoP DAC or one the user confirmed.
pub fn is_known_dsd_device(device: &str, user_confirmed: &[String]) -> bool {
    let d = norm(device);
    if d.is_empty() {
        return false;
    }
    BUILT_IN.iter().any(|k| d.contains(&norm(k))) || user_confirmed.iter().any(|u| norm(u) == d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_built_ins_loosely() {
        assert!(
            is_known_dsd_device("FIIO K15 ", &[]),
            "CoreAudio's trailing space"
        );
        assert!(is_known_dsd_device("fiio k15", &[]));
        assert!(!is_known_dsd_device("MacBook Pro Speakers", &[]));
        assert!(!is_known_dsd_device("", &[]));
    }

    #[test]
    fn user_confirmed_devices_match_exactly_ignoring_case_and_padding() {
        let mine = vec!["Topping D90".to_string()];
        assert!(is_known_dsd_device(" topping d90 ", &mine));
        assert!(!is_known_dsd_device("Topping D90 SE", &mine));
    }
}
