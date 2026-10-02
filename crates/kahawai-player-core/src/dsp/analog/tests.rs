//! Whole-stage tests: a tone through [`AnalogStage`], measured as a spectrum.
//! Harmonics, aliasing, level, latency, clicks, and every flavour.

use super::models::*;
use super::oversample::*;
use super::settings::*;
use super::stage::*;
use crate::dsp::{DspStage, EqBand, EqBandType, ParametricEq};
use std::f64::consts::PI;

fn on(flavour: AnalogFlavour, drive: f32, mix: f32) -> AnalogSettings {
    AnalogSettings {
        enabled: true,
        flavour,
        drive,
        mix,
        output_db: 0.0,
        auto_gain: false,
        antialias: AntiAliasChoice::Auto,
        sag: 0.0,
        transformer: 0.0,
    }
}

fn tone(bin: usize, n: usize, periods: usize, amp: f32, channels: usize) -> Vec<f32> {
    (0..n * periods)
        .flat_map(|i| {
            let v = amp * (2.0 * PI * bin as f64 * i as f64 / n as f64).sin() as f32;
            std::iter::repeat_n(v, channels)
        })
        .collect()
}

fn mono(x: &[f32], channels: usize) -> Vec<f64> {
    x.iter().step_by(channels).map(|&v| v as f64).collect()
}

fn spectrum(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let (mut re, mut im) = (x.to_vec(), vec![0.0; n]);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        for i in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (i + k, i + k + len / 2);
                let (vr, vi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - vr;
                im[b] = im[a] - vi;
                re[a] += vr;
                im[a] += vi;
            }
        }
        len <<= 1;
    }
    (0..n / 2)
        .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 2.0 / n as f64)
        .collect()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Run a stereo tone through the stage, return the 3rd of 4 periods
/// (settled, with samples on both sides).
fn run(stage: &mut AnalogStage, bin: usize, n: usize, amp: f32) -> Vec<f64> {
    let mut x = tone(bin, n, 4, amp, 2);
    stage.process(&mut x, 2);
    mono(&x, 2)[2 * n..3 * n].to_vec()
}

fn max_step(x: &[f32], channels: usize) -> f32 {
    let m: Vec<f32> = x.iter().step_by(channels).copied().collect();
    m.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn disabled_stage_is_bit_transparent() {
    let mut st = AnalogStage::new(44_100);
    let input = tone(200, 4096, 1, 0.5, 2);
    let mut out = input.clone();
    st.process(&mut out, 2);
    assert_eq!(out, input, "off from the start: untouched");

    // On, then off again: once faded out, bit-transparent once more.
    st.set_settings(on(AnalogFlavour::WarmTriode, 0.5, 1.0));
    let mut warm = tone(200, 4096, 2, 0.5, 2);
    st.process(&mut warm, 2);
    st.set_settings(AnalogSettings {
        enabled: false,
        ..on(AnalogFlavour::WarmTriode, 0.5, 1.0)
    });
    let mut fading = tone(200, 4096, 2, 0.5, 2);
    st.process(&mut fading, 2);
    let mut after = input.clone();
    st.process(&mut after, 2);
    assert_eq!(after, input, "faded out: untouched again");
}

#[test]
fn warm_triode_gives_even_harmonics_and_solid_state_odd() {
    let (n, bin) = (1 << 14, 200);
    let mut warm = AnalogStage::new(44_100);
    warm.set_settings(on(AnalogFlavour::WarmTriode, 0.4, 1.0));
    let sp = spectrum(&run(&mut warm, bin, n, 0.2));
    let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
    assert!(
        h2 > h3 + 10.0,
        "triode: 2nd ({h2:.1} dB) well above 3rd ({h3:.1} dB)"
    );
    assert!(
        h2 > -30.0 && h2 < -6.0,
        "audible but not extreme 2nd harmonic: {h2:.1} dB"
    );

    let mut solid = AnalogStage::new(44_100);
    solid.set_settings(on(AnalogFlavour::SolidState, 0.6, 1.0));
    let sp = spectrum(&run(&mut solid, bin, n, 0.5));
    let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
    assert!(
        h2 < -70.0,
        "solid state: no even harmonics, 2nd at {h2:.1} dB"
    );
    assert!(
        h3 > -50.0 && h3 < -6.0,
        "solid state: audible 3rd harmonic {h3:.1} dB"
    );
}

#[test]
fn harmonics_grow_with_level_and_drive() {
    let (n, bin) = (1 << 14, 200);
    let h2 = |drive: f32, amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(on(AnalogFlavour::WarmTriode, drive, 1.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        db(sp[bin * 2] / sp[bin])
    };
    assert!(
        h2(0.5, 0.4) > h2(0.5, 0.1) + 6.0,
        "louder input: more harmonic"
    );
    assert!(
        h2(0.7, 0.1) > h2(0.4, 0.1) + 3.0,
        "more drive: more harmonic"
    );
    assert!(
        h2(0.0, 0.05) < -40.0,
        "a quiet signal at zero drive stays very clean"
    );
}

/// Everything audible that is not a harmonic of the tone is aliasing.
fn alias_db_with(
    fs: u32,
    tone_hz: f64,
    drive: f32,
    amp: f32,
    flavour: AnalogFlavour,
    plan: Option<AntiAlias>,
) -> f64 {
    let n = 1 << 14;
    let bin = (tone_hz / fs as f64 * n as f64).round() as usize;
    let mut st = match plan {
        Some(p) => AnalogStage::with_plan(fs, p),
        None => AnalogStage::new(fs),
    };
    let aa = plan.map(AntiAliasChoice::from_plan).unwrap_or_default();
    st.set_settings(AnalogSettings {
        antialias: aa,
        ..on(flavour, drive, 1.0)
    });
    let sp = spectrum(&run(&mut st, bin, n, amp));
    let lim = (20_000.0 / fs as f64 * n as f64) as usize;
    let err = (1..lim.min(n / 2))
        .filter(|k| {
            let r = k % bin;
            r > 2 && r < bin - 2
        })
        .map(|k| sp[k] * sp[k])
        .sum::<f64>()
        .sqrt();
    db(err / sp[bin])
}

fn alias_db(fs: u32, tone_hz: f64, drive: f32, amp: f32) -> f64 {
    alias_db_with(fs, tone_hz, drive, amp, AnalogFlavour::WarmTriode, None)
}

/// Prints the aliasing of every plan (run with --ignored --nocapture); the
/// table in docs/v1/Analog-Emulation.md section 12 comes from this.
#[test]
#[ignore]
fn measure_anti_alias_plans() {
    let plans: [(usize, bool); 6] = [
        (1, false),
        (1, true),
        (2, false),
        (2, true),
        (4, false),
        (4, true),
    ];
    for (flavour, name) in [
        (AnalogFlavour::WarmTriode, "warm triode"),
        (AnalogFlavour::SolidState, "solid state"),
    ] {
        for (label, drive, amp, tone) in [
            (
                "hard test tone (drive 0.53, in 0.8, 9.5 kHz)",
                0.53f32,
                0.8f32,
                9_500.0,
            ),
            ("typical (drive 0.4, in 0.3, 5 kHz)", 0.4, 0.3, 5_000.0),
        ] {
            for fs in [44_100u32, 96_000] {
                let row: Vec<String> = plans
                    .iter()
                    .map(|&(f, a)| {
                        format!(
                            "{}x{}: {:>6.1}",
                            f,
                            if a { "+ADAA" } else { "     " },
                            alias_db_with(
                                fs,
                                tone,
                                drive,
                                amp,
                                flavour,
                                Some(AntiAlias { factor: f, adaa: a })
                            )
                        )
                    })
                    .collect();
                println!("{name:<12} {label:<44} {fs:>6} Hz | {}", row.join(" | "));
            }
        }
    }
}

#[test]
fn oversampling_keeps_aliasing_below_audibility() {
    // A hard test tone (research/analog-spike: the same drive without
    // protection aliases at -14 dB at 44.1 kHz).
    let a441 = alias_db(44_100, 9_500.0, 0.53, 0.8);
    assert!(a441 < -65.0, "44.1 kHz aliasing {a441:.1} dB");
    let a48 = alias_db(48_000, 9_500.0, 0.53, 0.8);
    assert!(a48 < -65.0, "48 kHz aliasing {a48:.1} dB");
    let a96 = alias_db(96_000, 9_500.0, 0.53, 0.8);
    assert!(a96 < -70.0, "96 kHz aliasing {a96:.1} dB");
    let a192 = alias_db(192_000, 9_500.0, 0.53, 0.8);
    assert!(
        a192 < -70.0,
        "192 kHz aliasing {a192:.1} dB (ADAA only, no oversampling)"
    );
    assert_eq!(
        (
            oversample_factor(44_100),
            oversample_factor(96_000),
            oversample_factor(192_000)
        ),
        (4, 2, 1)
    );
}

#[test]
fn triode_stage_matches_the_prototype_harmonic_profile() {
    // Research prototype (docs/v1/Analog-Emulation.md 10.2): a 12AX7 stage at input
    // 0.3 gave 2nd -32.5 dB and 3rd -60 dB; the 2nd rises 1 dB per dB.
    let (n, bin) = (1 << 14, 200);
    let h = |amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(on(AnalogFlavour::WarmTriode, 0.0, 1.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
    };
    let (h2, h3) = h(0.3);
    assert!(
        (-35.0..-30.0).contains(&h2),
        "2nd at input 0.3: {h2:.1} dB (prototype -32.5)"
    );
    assert!(h3 < -52.0, "3rd at input 0.3: {h3:.1} dB (prototype -60)");
    let (h2_low, _) = h(0.15);
    assert!(
        (h2 - h2_low - 6.0).abs() < 1.5,
        "2nd rises about 1 dB per dB of input"
    );
    let (h2_hi, h3_hi) = h(1.0);
    assert!(
        h2_hi > h3_hi + 12.0,
        "even harmonics keep dominating at full swing ({h2_hi:.1} vs {h3_hi:.1})"
    );
}

#[test]
fn dry_and_wet_are_time_aligned_and_latency_is_reported() {
    let mut st = AnalogStage::new(44_100);
    assert_eq!(st.latency_frames(), 0, "no latency while off");
    st.set_settings(on(AnalogFlavour::SolidState, 0.0, 0.5));
    assert_eq!(st.latency_frames(), 32);
    // Drive 0 and a tiny signal: the wet path is linear, so any mix of the
    // two paths must equal the input delayed by the latency, if aligned.
    let (n, bin) = (1 << 13, 186);
    let input = tone(bin, n, 4, 0.001, 1);
    let mut out = input.clone();
    st.process(&mut out, 1);
    let start = 2 * n;
    let err: f32 = (start..start + n)
        .map(|i| (out[i] - input[i - 32]).abs())
        .fold(0.0, f32::max);
    assert!(
        err < 0.001 * 0.06,
        "aligned: worst error {err} on a 0.001 tone"
    );
    assert_eq!(
        AnalogStage::new(192_000).latency(),
        0,
        "no oversampling, no latency at 192 kHz"
    );
}

#[test]
fn auto_gain_matches_the_processed_level_to_the_dry_level() {
    for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
        for drive in [0.4, 1.0] {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(AnalogSettings {
                auto_gain: true,
                ..on(flavour, drive, 1.0)
            });
            let (n, bin) = (1 << 14, 200);
            let out = run(&mut st, bin, n, 0.354);
            let rms = (out.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
            let want = 0.354 / 2f64.sqrt();
            assert!(
                (db(rms / want)).abs() < 1.0,
                "{flavour:?} drive {drive}: {:.2} dB off the dry level",
                db(rms / want)
            );
        }
    }
}

#[test]
fn live_changes_do_not_click() {
    let (n, bin) = (1 << 13, 40); // ~215 Hz
    let mut st = AnalogStage::new(44_100);
    st.set_settings(on(AnalogFlavour::WarmTriode, 0.2, 0.5));
    let mut a = tone(bin, n, 2, 0.4, 1);
    st.process(&mut a, 1);
    // Drive, mix and output all change at once, mid-signal.
    st.set_settings(AnalogSettings {
        output_db: 3.0,
        ..on(AnalogFlavour::WarmTriode, 0.9, 1.0)
    });
    let mut b = tone(bin, n, 2, 0.4, 1);
    let all = [a.clone(), b.clone()].concat();
    st.process(&mut b, 1);
    let seam = [a, b].concat();
    let _ = all;
    // A 215 Hz tone at 0.4 moves at most ~0.02 per sample (a little more with +3 dB).
    assert!(
        max_step(&seam[n..], 1) < 0.05,
        "no click on a live parameter change"
    );
}

#[test]
fn switching_flavour_live_fades_instead_of_jumping() {
    let (n, bin) = (1 << 13, 40);
    let mut st = AnalogStage::new(44_100);
    st.set_settings(on(AnalogFlavour::WarmTriode, 0.7, 1.0));
    let mut a = tone(bin, n, 2, 0.4, 1);
    st.process(&mut a, 1);
    st.set_settings(on(AnalogFlavour::SolidState, 0.7, 1.0));
    assert_eq!(
        st.active,
        AnalogFlavour::WarmTriode,
        "the swap waits for the fade-out"
    );
    let mut b = tone(bin, n, 2, 0.4, 1);
    st.process(&mut b, 1);
    assert_eq!(
        st.active,
        AnalogFlavour::SolidState,
        "swapped once faded out"
    );
    let seam = [a, b.clone()].concat();
    assert!(
        max_step(&seam[n..], 1) < 0.05,
        "no click across the flavour change"
    );
    // ...and the new curve is really in use (no even harmonics any more).
    let sp = spectrum(&mono(&b[n..2 * n], 1));
    assert!(db(sp[bin * 2] / sp[bin]) < -60.0);
}

fn with_aa(flavour: AnalogFlavour, aa: AntiAliasChoice) -> AnalogSettings {
    AnalogSettings {
        antialias: aa,
        ..on(flavour, 0.7, 1.0)
    }
}

#[test]
fn a_forced_plan_is_used_and_reported() {
    let mut st = AnalogStage::new(44_100);
    st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X1));
    let s = st.status().expect("on");
    assert_eq!(
        (s.plan.factor, s.plan.adaa, s.latency_frames),
        (1, false, 0)
    );
    assert_eq!(s.describe(), "no anti-aliasing, 0.0 ms latency");
    st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X4Adaa));
    assert_eq!(
        st.status().unwrap().describe(),
        "4x oversampling + ADAA, 0.7 ms latency"
    );
    // A forced plan survives a sample-rate change; auto follows it.
    st.prepare(96_000);
    assert_eq!(
        st.status().unwrap().plan,
        AntiAlias {
            factor: 4,
            adaa: true
        }
    );
    st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::Auto));
    assert_eq!(
        st.status().unwrap().plan,
        AntiAlias {
            factor: 2,
            adaa: true
        }
    );
    assert!(
        AnalogStage::new(44_100).status().is_none(),
        "off: nothing to report"
    );
}

#[test]
fn switching_plans_live_fades_without_a_click() {
    let (n, bin) = (1 << 13, 40);
    let mut st = AnalogStage::new(44_100);
    st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X4Adaa));
    let mut a = tone(bin, n, 2, 0.4, 1);
    st.process(&mut a, 1);
    st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X1));
    assert_eq!(
        st.status().unwrap().plan.factor,
        4,
        "the swap waits for the fade-out"
    );
    let mut b = tone(bin, n, 2, 0.4, 1);
    st.process(&mut b, 1);
    let s = st.status().unwrap();
    assert_eq!(
        (s.plan.factor, s.latency_frames),
        (1, 0),
        "swapped once faded out"
    );
    let seam = [a, b.clone()].concat();
    assert!(
        max_step(&seam[n..], 1) < 0.05,
        "no click across the plan change"
    );
    // ...and it came back at full level, not stuck faded out.
    let rms = (b[n..].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    assert!(rms > 0.2, "signal present after the swap: rms {rms}");
}

fn colour(flavour: AnalogFlavour, drive: f32, sag: f32, transformer: f32) -> AnalogSettings {
    AnalogSettings {
        sag,
        transformer,
        ..on(flavour, drive, 1.0)
    }
}

/// RMS of the processed signal over `range` (samples of one channel).
fn rms(x: &[f32], range: std::ops::Range<usize>) -> f64 {
    let w = &x[range];
    (w.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
}

#[test]
fn sag_compresses_loud_passages_more_than_quiet_ones() {
    let (n, bin) = (1 << 14, 371); // ~1 kHz
    let gain_db = |sag: f32, amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::WarmTriode, 0.3, sag, 0.0));
        let out = run(&mut st, bin, n, amp);
        let o = (out.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        db(o / (amp as f64 / 2f64.sqrt()))
    };
    let squash = |sag| gain_db(sag, 0.1) - gain_db(sag, 0.8);
    assert!(
        squash(1.0) > squash(0.0) + 0.5,
        "sag adds compression: {:.2} dB vs {:.2} dB",
        squash(1.0),
        squash(0.0)
    );
}

#[test]
fn sag_recovers_over_about_a_tenth_of_a_second() {
    let fs = 44_100usize;
    let quiet_level_after = |sag: f32, start_ms: usize| {
        let mut st = AnalogStage::new(fs as u32);
        st.set_settings(colour(AnalogFlavour::SolidState, 0.5, sag, 0.0));
        // 300 ms loud burst, then a quiet tone.
        let sr = fs as f64;
        let mut x: Vec<f32> = (0..fs / 3 * 2).map(|i| if i < fs * 3 / 10 { 0.9 } else { 0.1 } * (2.0 * PI * 1000.0 * i as f64 / sr).sin() as f32).collect();
        st.process(&mut x, 1);
        let after = fs * 3 / 10 + fs / 100; // skip the wet-path latency and the fade
        rms(
            &x,
            after + start_ms * fs / 1000..after + start_ms * fs / 1000 + fs / 50,
        )
    };
    let drop_db = |sag: f32| db(quiet_level_after(sag, 40) / quiet_level_after(sag, 180));
    assert!(
        drop_db(1.0) < -0.3,
        "just after the burst the stage is quieter: {:.2} dB",
        drop_db(1.0)
    );
    assert!(
        drop_db(0.0).abs() < 0.1,
        "no sag, no recovery curve: {:.2} dB",
        drop_db(0.0)
    );
}

#[test]
fn transformer_saturates_the_bass_with_level_and_leaves_the_mids_alone() {
    let n = 1 << 14;
    let h3 = |bin: usize, amp: f32, xf: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::SolidState, 0.0, 0.0, xf));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        db(sp[bin * 3] / sp[bin])
    };
    let (bass, mid) = (22, 371); // ~59 Hz and ~1 kHz
    assert!(
        h3(bass, 0.3, 1.0) > h3(bass, 0.3, 0.0) + 8.0,
        "bass harmonics appear with the transformer on"
    );
    assert!(
        h3(bass, 0.6, 1.0) > h3(bass, 0.1, 1.0) + 12.0,
        "and grow with level"
    );
    assert!(
        h3(bass, 0.3, 1.0) > h3(bass, 0.3, 0.3),
        "and with the amount"
    );
    assert!(
        (h3(mid, 0.3, 1.0) - h3(mid, 0.3, 0.0)).abs() < 3.0,
        "1 kHz is unaffected"
    );
}

#[test]
fn linear_response_stays_flat_at_low_level_with_sag_and_transformer_on() {
    // Small signals see no colour: within +-1 dB from 30 Hz to 16 kHz, both flavours.
    let n = 1 << 14;
    for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
        for hz in [30.0, 60.0, 120.0, 500.0, 2_000.0, 8_000.0, 16_000.0] {
            let bin = (hz / 44_100.0 * n as f64).round() as usize;
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(flavour, 0.0, 0.3, 0.3));
            let sp = spectrum(&run(&mut st, bin, n, 0.01));
            let gain = db(sp[bin] / 0.01);
            assert!(gain.abs() < 1.0, "{flavour:?} at {hz} Hz: {gain:.2} dB");
        }
    }
}

#[test]
fn sag_and_transformer_changes_do_not_click() {
    let (n, bin) = (1 << 13, 12); // ~65 Hz bass tone
    let mut st = AnalogStage::new(44_100);
    st.set_settings(colour(AnalogFlavour::WarmTriode, 0.5, 0.0, 0.0));
    let mut a = tone(bin, n, 2, 0.4, 1);
    st.process(&mut a, 1);
    st.set_settings(colour(AnalogFlavour::WarmTriode, 0.5, 1.0, 1.0));
    let mut b = tone(bin, n, 2, 0.4, 1);
    st.process(&mut b, 1);
    let seam = [a, b].concat();
    assert!(
        max_step(&seam[n..], 1) < 0.05,
        "no click when sag and transformer change live"
    );
}

#[test]
fn single_ended_tubes_are_even_dominant_and_push_pull_is_odd_dominant() {
    let (n, bin) = (1 << 14, 200);
    let h = |f: AnalogFlavour, amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(f, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
    };
    for f in [
        AnalogFlavour::WarmTriode,
        AnalogFlavour::Tube12at7,
        AnalogFlavour::Tube12au7,
        AnalogFlavour::Tube6sn7,
        AnalogFlavour::Tube6dj8,
        AnalogFlavour::Tube300b,
        AnalogFlavour::Tube2a3,
        AnalogFlavour::Tube6sl7,
        AnalogFlavour::Tube12ay7,
        AnalogFlavour::Tube12ax7a,
    ] {
        let (h2, h3) = h(f, 0.2);
        assert!(
            h2 > h3 + 15.0,
            "{f:?}: 2nd {h2:.1} dB well above 3rd {h3:.1} dB"
        );
        assert!(
            h2 < -20.0 && h2 > -60.0,
            "{f:?}: audible but not extreme 2nd: {h2:.1} dB"
        );
    }
    let (h2, h3) = h(AnalogFlavour::PushPull, 0.3);
    assert!(
        h3 > h2 + 8.0,
        "push-pull: 3rd {h3:.1} dB above 2nd {h2:.1} dB"
    );
    assert!(
        h2 > -100.0,
        "...with a little even left from the imperfect match ({h2:.1} dB)"
    );
    // ...and it is much cleaner in the even harmonics than a single-ended 2A3.
    assert!(
        h2 < h(AnalogFlavour::Tube2a3, 0.3).0 - 15.0,
        "push-pull cancels the 2nd of its own tube"
    );
}

#[test]
fn class_ab_pentode_pairs_are_odd_dominant_with_crossover_grit_at_low_level() {
    let (n, bin) = (1 << 14, 200);
    let h = |f: AnalogFlavour, amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(f, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
    };
    let class_a = h(AnalogFlavour::PushPull, 0.1).1;
    for f in [
        AnalogFlavour::PushPullEl34,
        AnalogFlavour::PushPull6l6gc,
        AnalogFlavour::PushPullKt88,
    ] {
        let (h2, h3) = h(f, 0.3);
        assert!(
            h3 > h2 + 8.0,
            "{f:?}: odd-dominant, 3rd {h3:.1} dB vs 2nd {h2:.1} dB"
        );
        // Class AB leaves a crossover region: the 3rd is already there at low level,
        // much more than in the class-A pair of 2A3s.
        let low = h(f, 0.1).1;
        assert!(
            low > class_a + 15.0,
            "{f:?}: crossover distortion at low level ({low:.1} dB vs {class_a:.1} dB)"
        );
    }
}

#[test]
fn single_ended_pentode_has_both_kinds_of_harmonic() {
    let (n, bin) = (1 << 14, 200);
    let mut st = AnalogStage::new(44_100);
    st.set_settings(colour(AnalogFlavour::TubeEl84, 0.4, 0.0, 0.0));
    let sp = spectrum(&run(&mut st, bin, n, 0.1));
    let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
    assert!(
        h2 > -45.0 && h3 > -60.0,
        "a class-A pentode is not clean: 2nd {h2:.1}, 3rd {h3:.1} dB"
    );
}

#[test]
fn jfet_is_square_law_and_diodes_clip_symmetrically_or_not() {
    let (n, bin) = (1 << 14, 200);
    let h = |f: AnalogFlavour, amp: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(f, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
    };
    let (h2, h3) = h(AnalogFlavour::Jfet, 0.2);
    assert!(h2 > -30.0 && h2 < -12.0, "JFET 2nd {h2:.1} dB");
    assert!(
        h2 > h3 + 40.0,
        "square law: almost no 3rd ({h3:.1} dB vs {h2:.1} dB)"
    );
    let (s2, s3) = h(AnalogFlavour::SiliconDiode, 0.2);
    assert!(
        s2 < -80.0 && s3 > -45.0,
        "silicon pair is symmetric: 2nd {s2:.1}, 3rd {s3:.1} dB"
    );
    let (g2, g3) = h(AnalogFlavour::GermaniumDiode, 0.1);
    assert!(
        g2 > -40.0 && g3 > -45.0,
        "germanium against silicon is lopsided: 2nd {g2:.1}, 3rd {g3:.1} dB"
    );
}

#[test]
fn iron_and_sag_only_has_no_distortion_curve_but_still_colours_the_bass_and_dynamics() {
    let (n, bin) = (1 << 14, 371);
    let mut st = AnalogStage::new(44_100);
    st.set_settings(colour(AnalogFlavour::IronSag, 0.7, 0.0, 0.0));
    let sp = spectrum(&run(&mut st, bin, n, 0.6));
    assert!(
        db(sp[bin * 3] / sp[bin]) < -100.0 && db(sp[bin * 2] / sp[bin]) < -100.0,
        "a linear curve adds no harmonics"
    );
    // The transformer still saturates the bass.
    let h3 = |xf: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::IronSag, 0.0, 0.0, xf));
        let sp = spectrum(&run(&mut st, 22, n, 0.5));
        db(sp[66] / sp[22])
    };
    assert!(
        h3(1.0) > h3(0.0) + 30.0,
        "bass harmonics from the transformer alone"
    );
}

#[test]
fn hard_transistor_is_clean_below_the_knee_and_harsh_above_it() {
    let (n, bin) = (1 << 14, 200);
    let h3 = |amp: f32, drive: f32| {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::HardTransistor, drive, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, amp));
        (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
    };
    assert!(h3(0.1, 0.4).1 < -90.0, "clean well below the knee");
    let (h2, h3_hard) = h3(0.6, 0.4);
    assert!(
        h3_hard > -30.0,
        "strong 3rd once it clips ({h3_hard:.1} dB)"
    );
    assert!(h2 < -80.0, "symmetric: no even harmonics ({h2:.1} dB)");
    // Harsher than the soft symmetric curve at the same setting.
    let soft = {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::SolidState, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, 0.3));
        db(sp[bin * 5] / sp[bin])
    };
    let hard = {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::HardTransistor, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, 0.75));
        db(sp[bin * 5] / sp[bin])
    };
    assert!(
        hard > soft,
        "the hard clip's 5th ({hard:.1} dB) exceeds the soft one's at a lower level ({soft:.1} dB)"
    );
}

#[test]
fn every_flavour_keeps_aliasing_low_at_every_rate_and_is_linear_when_quiet() {
    for f in ALL_FLAVOURS {
        // 96 kHz (2x oversampling) is measured for every flavour in the ignored
        // profile test and for the reference flavour above; 44.1 and 192 kHz here.
        for fs in [44_100u32, 192_000] {
            let a = alias_db_with(fs, 9_500.0, 0.53, 0.8, f, None);
            assert!(a < -60.0, "{f:?} at {fs} Hz aliases at {a:.1} dB");
        }
        let n = 1 << 14;
        let bin = 371;
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(f, 0.0, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, 0.01));
        assert!(
            db(sp[bin] / 0.01).abs() < 1.0,
            "{f:?}: unity gain for a quiet signal"
        );
    }
}

#[test]
fn flavour_names_round_trip_and_every_flavour_can_be_selected_live() {
    for (f, name) in [
        (AnalogFlavour::WarmTriode, "warm_triode"),
        (AnalogFlavour::PushPull, "push_pull"),
        (AnalogFlavour::SolidState, "solid_state"),
        (AnalogFlavour::HardTransistor, "hard_transistor"),
        (AnalogFlavour::Tube12at7, "tube_12at7"),
        (AnalogFlavour::Tube12au7, "tube_12au7"),
        (AnalogFlavour::Tube6sn7, "tube_6sn7"),
        (AnalogFlavour::Tube6dj8, "tube_6dj8"),
        (AnalogFlavour::Tube300b, "tube_300b"),
        (AnalogFlavour::Tube2a3, "tube_2a3"),
        (AnalogFlavour::Tube6sl7, "tube_6sl7"),
        (AnalogFlavour::Tube12ay7, "tube_12ay7"),
        (AnalogFlavour::Tube12ax7a, "tube_12ax7a"),
        (AnalogFlavour::TubeEl84, "tube_el84"),
        (AnalogFlavour::PushPullEl34, "push_pull_el34"),
        (AnalogFlavour::PushPull6l6gc, "push_pull_6l6gc"),
        (AnalogFlavour::PushPullKt88, "push_pull_kt88"),
        (AnalogFlavour::Jfet, "jfet"),
        (AnalogFlavour::SiliconDiode, "silicon_diode"),
        (AnalogFlavour::GermaniumDiode, "germanium_diode"),
        (AnalogFlavour::IronSag, "iron_sag"),
    ] {
        assert_eq!(serde_json::to_string(&f).unwrap(), format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<AnalogFlavour>(&format!("\"{name}\"")).unwrap(),
            f
        );
    }
    // Step through every flavour while audio flows: each swap fades, nothing clicks or blows up.
    let (n, bin) = (1 << 12, 40);
    let mut st = AnalogStage::new(44_100);
    let mut all = Vec::new();
    for f in ALL_FLAVOURS {
        st.set_settings(colour(f, 0.5, 0.2, 0.2));
        let mut x = tone(bin, n, 1, 0.4, 1);
        st.process(&mut x, 1);
        assert!(
            x.iter().all(|v| v.is_finite() && v.abs() < 2.0),
            "{f:?}: finite and bounded"
        );
        all.extend(x);
    }
    assert!(
        max_step(&all[n..], 1) < 0.1,
        "no click while stepping through the flavours"
    );
}

/// Prints each flavour's operating point and harmonic profile (run with
/// --ignored --nocapture); section 15 of docs/v1/Analog-Emulation.md comes from this.
#[test]
#[ignore]
fn print_flavour_profiles() {
    for t in ALL_TUBES {
        let sp = t.spec();
        let q = sp.quiescent();
        println!(
            "{:?}: bias {:.1} V, plate {:.0} V, {:.1} mA, load {:.0} ohm",
            t,
            q.vgk,
            q.vp,
            q.ip * 1000.0,
            q.r_ac
        );
    }
    let (n, bin) = (1 << 14, 200);
    for f in ALL_FLAVOURS {
        let mut line = format!("{f:?}:");
        for amp in [0.1f32, 0.3, 0.6] {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(f, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            let d = |k: usize| db(sp[bin * k] / sp[bin]);
            line += &format!(
                "  in {amp}: 2nd {:6.1} 3rd {:6.1} 4th {:6.1} 5th {:6.1} |",
                d(2),
                d(3),
                d(4),
                d(5)
            );
        }
        println!("{line}");
    }
    for f in ALL_FLAVOURS {
        let a = |fs: u32| alias_db_with(fs, 9_500.0, 0.53, 0.8, f, None);
        println!(
            "ALIAS {f:?}: 44.1k {:.1}  96k {:.1}  192k {:.1}",
            a(44_100),
            a(96_000),
            a(192_000)
        );
    }
}

#[test]
fn survives_every_sample_rate_and_channel_count() {
    // Odd rates (DSD-derived PCM up to 705.6 kHz, telephone rates) and channel counts (mono to 7.1)
    // must neither panic nor produce non-finite or runaway samples, for a curve of each kind.
    for f in [
        AnalogFlavour::WarmTriode,
        AnalogFlavour::PushPullEl34,
        AnalogFlavour::SolidState,
        AnalogFlavour::GermaniumDiode,
    ] {
        for rate in [
            8_000u32, 11_025, 22_050, 32_000, 48_000, 88_200, 176_400, 352_800, 705_600,
        ] {
            for channels in [1usize, 2, 6, 8] {
                let mut st = AnalogStage::new(rate);
                st.set_settings(colour(f, 0.8, 0.5, 0.5));
                let frames = 2048;
                let mut x: Vec<f32> = (0..frames * channels)
                    .map(|i| {
                        0.5 * (2.0 * PI * 440.0 * (i / channels) as f64 / rate as f64).sin() as f32
                    })
                    .collect();
                st.process(&mut x, channels);
                assert!(
                    x.iter().all(|v| v.is_finite() && v.abs() < 4.0),
                    "{f:?} at {rate} Hz, {channels} ch"
                );
            }
        }
    }
}

#[test]
fn changing_the_channel_count_or_rate_mid_stream_is_safe() {
    let mut st = AnalogStage::new(44_100);
    st.set_settings(colour(AnalogFlavour::WarmTriode, 0.6, 0.3, 0.3));
    let mut mono = tone(40, 4096, 1, 0.4, 1);
    st.process(&mut mono, 1);
    let mut stereo = tone(40, 4096, 1, 0.4, 2);
    st.process(&mut stereo, 2); // channel count changes
    st.prepare(96_000); // rate changes
    let mut hi = tone(40, 4096, 1, 0.4, 2);
    st.process(&mut hi, 2);
    assert!(hi.iter().all(|v| v.is_finite() && v.abs() < 2.0));
    let mut empty: [f32; 0] = [];
    st.process(&mut empty, 2); // an empty chunk is fine
    st.process(&mut stereo, 0); // and so is a zero channel count
}

#[test]
fn stages_share_one_trait() {
    let mut eq = ParametricEq::new(44_100);
    eq.set_bands(vec![EqBand {
        band_type: EqBandType::Peaking,
        freq: 1000.0,
        gain_db: 6.0,
        q: 1.0,
    }])
    .unwrap();
    let mut stages: Vec<Box<dyn DspStage>> = vec![Box::new(eq), Box::new(AnalogStage::new(44_100))];
    let input = tone(200, 4096, 1, 0.3, 2);
    let mut out = input.clone();
    for s in stages.iter_mut() {
        s.prepare(44_100);
        s.process(&mut out, 2);
    }
    assert_ne!(out, input, "the EQ stage changed the audio");
    assert_eq!(stages[0].latency_frames(), 0);
    stages.iter_mut().for_each(|s| s.reset());
}
