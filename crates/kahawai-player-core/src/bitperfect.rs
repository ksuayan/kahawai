//! Bit-perfect output: exclusive, untouched PCM.
//!
//! In this mode the engine hands the decoded samples to the exclusive
//! (hog-mode) device at the file's own sample rate: no EQ, no loudness gain,
//! no software volume, no resampling. That is what lets a DAC that decodes
//! MQA see the MQA signal intact (any change to the samples destroys it), and
//! what audiophiles mean by "bit-perfect".
//!
//! Samples travel as packed little-endian 24-bit integers. The decoder yields
//! f32 scaled by 2^-23 (exact for 24-bit and narrower integer sources, since
//! f32 has a 24-bit significand and the scale is a power of two), so the
//! conversion back is lossless.

use serde::{Deserialize, Serialize};

/// When to use the exclusive bit-perfect path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitPerfect {
    /// Always shared-mode output with the DSP chain (default).
    #[default]
    Off,
    /// Only for MQA-encoded files (so an MQA DAC can decode them).
    Mqa,
    /// For every track that can be played untouched.
    All,
}

impl BitPerfect {
    /// Does this preference select bit-perfect output for a track?
    pub fn applies_to(self, is_mqa: bool) -> bool {
        match self {
            BitPerfect::Off => false,
            BitPerfect::Mqa => is_mqa,
            BitPerfect::All => true,
        }
    }
}

const SCALE: f32 = 8_388_608.0; // 2^23

/// Convert interleaved f32 samples to packed little-endian signed 24-bit,
/// appending to `out`. Values are rounded to the nearest integer and clamped
/// to the 24-bit range (so an over-range sample saturates instead of wrapping).
pub fn f32_to_i24_le(samples: &[f32], out: &mut Vec<u8>) {
    out.reserve(samples.len() * 3);
    for &s in samples {
        let v = (s * SCALE).round().clamp(-SCALE, SCALE - 1.0) as i32;
        out.extend_from_slice(&v.to_le_bytes()[..3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(samples: &[f32]) -> Vec<u8> {
        let mut v = Vec::new();
        f32_to_i24_le(samples, &mut v);
        v
    }

    fn i24(b: &[u8]) -> i32 {
        // sign-extend a 3-byte little-endian value
        let v = i32::from_le_bytes([b[0], b[1], b[2], 0]);
        (v << 8) >> 8
    }

    #[test]
    fn preference_selects_the_right_tracks() {
        assert!(!BitPerfect::Off.applies_to(true));
        assert!(!BitPerfect::Off.applies_to(false));
        assert!(BitPerfect::Mqa.applies_to(true));
        assert!(!BitPerfect::Mqa.applies_to(false));
        assert!(BitPerfect::All.applies_to(true));
        assert!(BitPerfect::All.applies_to(false));
        assert_eq!(BitPerfect::default(), BitPerfect::Off);
    }

    #[test]
    fn serializes_as_snake_case_strings() {
        assert_eq!(serde_json::to_string(&BitPerfect::Mqa).unwrap(), "\"mqa\"");
        assert_eq!(
            serde_json::from_str::<BitPerfect>("\"all\"").unwrap(),
            BitPerfect::All
        );
        assert!(serde_json::from_str::<BitPerfect>("\"bogus\"").is_err());
    }

    #[test]
    fn every_24_bit_integer_survives_the_round_trip_exactly() {
        // Exhaustive over a stride covering the whole range incl. both extremes.
        let mut ints: Vec<i32> = (-8_388_608..8_388_608).step_by(997).collect();
        ints.extend([-8_388_608, -8_388_607, -1, 0, 1, 8_388_606, 8_388_607]);
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / SCALE).collect();
        let out = bytes(&floats);
        assert_eq!(out.len(), ints.len() * 3);
        for (n, &want) in ints.iter().enumerate() {
            assert_eq!(i24(&out[n * 3..n * 3 + 3]), want, "sample {n}");
        }
    }

    #[test]
    fn sixteen_bit_material_lands_in_the_top_of_the_24_bit_word() {
        for s16 in [-32768i32, -1, 0, 1, 12345, 32767] {
            let out = bytes(&[s16 as f32 / 32768.0]);
            assert_eq!(i24(&out), s16 * 256, "16-bit {s16}");
            assert_eq!(out[0], 0, "low byte stays zero");
        }
    }

    #[test]
    fn known_byte_patterns() {
        assert_eq!(bytes(&[0.0]), vec![0, 0, 0]);
        assert_eq!(bytes(&[-1.0]), vec![0x00, 0x00, 0x80]); // most negative
        assert_eq!(bytes(&[0.5]), vec![0x00, 0x00, 0x40]);
        assert_eq!(bytes(&[1.0 / SCALE]), vec![0x01, 0x00, 0x00]); // one LSB
    }

    #[test]
    fn out_of_range_samples_saturate_instead_of_wrapping() {
        assert_eq!(i24(&bytes(&[1.0])), 8_388_607);
        assert_eq!(i24(&bytes(&[2.5])), 8_388_607);
        assert_eq!(i24(&bytes(&[-1.0])), -8_388_608);
        assert_eq!(i24(&bytes(&[-3.0])), -8_388_608);
        assert_eq!(i24(&bytes(&[f32::INFINITY])), 8_388_607);
        assert_eq!(i24(&bytes(&[f32::NEG_INFINITY])), -8_388_608);
        assert_eq!(bytes(&[f32::NAN]), vec![0, 0, 0], "NaN becomes silence");
    }

    #[test]
    fn interleaving_and_appending_are_preserved() {
        let mut v = vec![0xAA];
        f32_to_i24_le(&[0.5, -0.5], &mut v);
        assert_eq!(v.len(), 1 + 6);
        assert_eq!(v[0], 0xAA, "appends to existing content");
        assert_eq!(i24(&v[1..4]), 4_194_304);
        assert_eq!(i24(&v[4..7]), -4_194_304);
        let mut empty = Vec::new();
        f32_to_i24_le(&[], &mut empty);
        assert!(empty.is_empty());
    }
}
