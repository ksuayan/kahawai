//! CPU cost of the real `AnalogStage` for each anti-aliasing plan (release
//! build): one second of stereo audio in 4096-frame chunks, like the engine.
use kahawai_player_core::{
    triode_table, AnalogFlavour, AnalogSettings, AnalogStage, AntiAlias, AntiAliasChoice, DspStage, EqBand, EqBandType, ParametricEq,
};
use std::time::Instant;

fn main() {
    let t = Instant::now();
    let _ = triode_table();
    println!("first use of the triode table (build from Koren's equations): {:.1} ms\n", t.elapsed().as_secs_f64() * 1e3);
    for fs in [44_100usize, 96_000, 192_000] {
        let tone: Vec<f32> = (0..fs).flat_map(|i| { let v = 0.4 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / fs as f32).sin(); [v, v] }).collect();
        let mut run = |name: &str, f: &mut dyn FnMut(&mut [f32])| {
            let mut buf = tone.clone();
            for c in buf.chunks_mut(4096 * 2) { f(c); } // prime
            let reps = 5; let t = Instant::now();
            for _ in 0..reps { let mut b = tone.clone(); for c in b.chunks_mut(4096 * 2) { f(c); } std::hint::black_box(&b); }
            let dt = t.elapsed().as_secs_f64() / reps as f64;
            println!("  {fs:>6} Hz  {name:<34} {:>7.2} ms per 1 s of stereo audio -> {:>5.1}% of one core", dt * 1e3, dt * 100.0);
        };
        let mut eq = ParametricEq::new(fs as u32);
        eq.set_bands((0..8).map(|i| EqBand { band_type: EqBandType::Peaking, freq: 100.0 * 2f32.powi(i), gain_db: 3.0, q: 1.0 }).collect()).unwrap();
        run("existing 8-band EQ", &mut |c| DspStage::process(&mut eq, c, 2));
        for (label, flavour) in [("triode", AnalogFlavour::WarmTriode), ("solid state", AnalogFlavour::SolidState)] {
            for (factor, adaa) in [(1usize, false), (1, true), (2, false), (2, true), (4, false), (4, true)] {
                let mut st = AnalogStage::with_plan(fs as u32, AntiAlias { factor, adaa });
                let antialias = match (factor, adaa) { (1, false) => AntiAliasChoice::X1, (1, true) => AntiAliasChoice::X1Adaa, (2, false) => AntiAliasChoice::X2, (2, true) => AntiAliasChoice::X2Adaa, (_, false) => AntiAliasChoice::X4, (_, true) => AntiAliasChoice::X4Adaa };
                st.set_settings(AnalogSettings { enabled: true, flavour, drive: 0.6, mix: 0.5, output_db: 0.0, auto_gain: true, antialias, ..Default::default() });
                run(&format!("{label}: {factor}x{}", if adaa { " + ADAA" } else { "" }), &mut |c| st.process(c, 2));
            }
        }
        println!();
    }
}
