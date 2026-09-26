use std::f64::consts::PI;
use std::time::Instant;

// ---------- FFT ----------
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j ^= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci, re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
                re[i + k] = ur + vr; im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr; im[i + k + len / 2] = ui - vi;
                let t = cr * wr - ci * wi; ci = cr * wi + ci * wr; cr = t;
            }
        }
        len <<= 1;
    }
}
fn spectrum(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut re = x.to_vec(); let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 2.0 / n as f64).collect()
}
fn db(x: f64) -> f64 { 20.0 * x.max(1e-12).log10() }

// ---------- nonlinearities (memoryless) ----------
trait Shaper { fn f(&self, x: f64) -> f64; }

/// asymmetric tanh: even harmonics from bias b
struct AsymTanh { g: f64, b: f64 }
impl Shaper for AsymTanh { fn f(&self, x: f64) -> f64 { (self.g * x + self.b).tanh() - self.b.tanh() } }

/// symmetric tanh ("transistor" soft knee)
struct SymTanh { g: f64 }
impl Shaper for SymTanh { fn f(&self, x: f64) -> f64 { (self.g * x).tanh() } }

/// hard clip
struct HardClip { g: f64 }
impl Shaper for HardClip { fn f(&self, x: f64) -> f64 { (self.g * x).clamp(-1.0, 1.0) } }

/// Triode stage from Koren's equations (12AX7, Koren part-1 parameters), resistive load line.
struct Triode { table: Vec<f64>, lo: f64, hi: f64 }
fn koren_ip(vgk: f64, vpk: f64) -> f64 {
    let (mu, ex, kg1, kp, kvb) = (100.0, 1.4, 1060.0, 600.0, 300.0);
    let e1 = vpk / kp * (1.0 + (kp * (1.0 / mu + vgk / (kvb + vpk * vpk).sqrt())).exp()).ln();
    if e1 <= 0.0 { 0.0 } else { e1.powf(ex) / kg1 * 2.0 } // amps
}
impl Triode {
    fn new(bplus: f64, rl: f64, vbias: f64, swing: f64) -> Self {
        // x in [-1,1] maps to grid volts vbias + swing*x; solve vp = bplus - ip(vg,vp)*rl by bisection.
        let n = 8192;
        let (lo, hi) = (-1.0, 1.0);
        let vp_of = |vg: f64| { let (mut a, mut b) = (0.0f64, bplus); for _ in 0..80 { let m = 0.5 * (a + b); let g = m - (bplus - koren_ip(vg, m) * rl); if g > 0.0 { b = m } else { a = m } } 0.5 * (a + b) };
        let mut t: Vec<f64> = (0..=n).map(|i| { let x = lo + (hi - lo) * i as f64 / n as f64; -(vp_of(vbias + swing * x)) }).collect();
        let v0 = t[n / 2]; for v in t.iter_mut() { *v -= v0; }
        // normalise so small-signal slope at 0 is 1
        let slope = (t[n / 2 + 1] - t[n / 2 - 1]) / (2.0 * (hi - lo) / n as f64);
        for v in t.iter_mut() { *v /= slope; }
        Triode { table: t, lo, hi }
    }
    fn lookup(&self, x: f64) -> f64 {
        let n = self.table.len() - 1;
        let p = ((x - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0) * n as f64;
        let i = (p as usize).min(n - 1); let fr = p - i as f64;
        self.table[i] * (1.0 - fr) + self.table[i + 1] * fr
    }
}
/// Triode with drive: x scaled, curve clamped at the grid-swing ends (input beyond ±1 stays on the edge value).
struct TriodeDrive<'a> { t: &'a Triode, g: f64 }
impl Shaper for TriodeDrive<'_> { fn f(&self, x: f64) -> f64 { self.t.lookup(self.g * x) / self.g.max(1.0) } }

// ---------- methods ----------
fn naive(s: &dyn Shaper, x: &[f64]) -> Vec<f64> { x.iter().map(|&v| s.f(v)).collect() }

/// first-order ADAA with the analytic antiderivative of the asymmetric tanh.
fn ln_cosh(x: f64) -> f64 { let a = x.abs(); a + (1.0 + (-2.0 * a).exp()).ln() - std::f64::consts::LN_2 }
fn adaa1(s: &AsymTanh, x: &[f64]) -> Vec<f64> {
    let af = |v: f64| ln_cosh(s.g * v + s.b) / s.g - v * s.b.tanh();
    let mut prev = 0.0; let mut prev_af = af(0.0);
    x.iter().map(|&v| { let a = af(v); let y = if (v - prev).abs() < 1e-7 { s.f(0.5 * (v + prev)) } else { (a - prev_af) / (v - prev) }; prev = v; prev_af = a; y }).collect()
}

fn kaiser_lowpass(taps: usize, cutoff: f64) -> Vec<f64> { // cutoff as fraction of the (high) sample rate, 0..0.5
    let m = (taps - 1) as f64; let beta = 8.0;
    let i0 = |x: f64| { let mut s = 1.0; let mut t = 1.0; for k in 1..40 { t *= (x / (2.0 * k as f64)).powi(2); s += t; } s };
    (0..taps).map(|n| { let k = n as f64 - m / 2.0; let sinc = if k == 0.0 { 2.0 * cutoff } else { (2.0 * PI * cutoff * k).sin() / (PI * k) }; let w = i0(beta * (1.0 - (2.0 * n as f64 / m - 1.0).powi(2)).max(0.0).sqrt()) / i0(beta); sinc * w }).collect()
}

/// L-times oversampled processing with polyphase-style FIR up/down (linear phase).
fn oversampled(s: &dyn Shaper, x: &[f64], l: usize) -> Vec<f64> {
    let taps = 32 * l;                       // 32 taps per phase
    let h = kaiser_lowpass(taps + 1, 0.45 / l as f64);
    let gain = l as f64;
    let delay = (h.len() - 1) / 2;
    // upsample: y_up[n*l + p] = gain * sum_k h[p + k*l] * x[n - k]
    let mut up = vec![0.0; x.len() * l];
    for n in 0..x.len() { for p in 0..l { let mut acc = 0.0; let mut k = 0; while p + k * l < h.len() { if n >= k { acc += h[p + k * l] * x[n - k]; } k += 1; } up[n * l + p] = gain * acc; } }
    let sh: Vec<f64> = up.iter().map(|&v| s.f(v)).collect();
    // downsample: lowpass then take every l-th sample, compensate delay
    let mut out = vec![0.0; x.len()];
    for n in 0..x.len() { let c = n * l + 2 * delay; let mut acc = 0.0; for (j, hv) in h.iter().enumerate() { if c >= j && c - j < sh.len() { acc += hv * sh[c - j]; } } out[n] = acc; }
    out
}

// ---------- experiments ----------
fn sine(n: usize, bin: usize, amp: f64, fs_mult: usize) -> Vec<f64> { (0..n * fs_mult).map(|i| amp * (2.0 * PI * bin as f64 * i as f64 / (n * fs_mult) as f64).sin()).collect() }

fn harmonics(name: &str, s: &dyn Shaper, amp: f64) {
    let n = 1 << 14; let bin = 37; // ~ 100 Hz-ish at 44.1k*... exact periodic
    let y = naive(s, &sine(n, bin, amp, 16)); // 16x so harmonics up to 5th have no aliasing
    let sp = spectrum(&y);
    let f = sp[bin];
    let h = |k: usize| db(sp[bin * k] / f);
    println!("  {name:<28} amp={amp:<4}  2nd {:>7.1}  3rd {:>7.1}  4th {:>7.1}  5th {:>7.1} dBc   (THD {:.2}%)", h(2), h(3), h(4), h(5),
        100.0 * ((2..=9).map(|k| (sp[bin * k] / f).powi(2)).sum::<f64>()).sqrt());
}

fn alias_test(name: &str, s: &dyn Shaper, amp: f64, fs: f64, f0_hz: f64, run: &dyn Fn(&[f64]) -> Vec<f64>) -> f64 {
    let n = 1 << 14; let bin = (f0_hz / fs * n as f64).round() as usize;
    let x1 = sine(n, bin, amp, 1);
    let x4: Vec<f64> = x1.iter().chain(x1.iter()).chain(x1.iter()).chain(x1.iter()).copied().collect();
    let y4 = run(&x4);
    let y = y4[2 * n..3 * n].to_vec(); // third period: settled, and the filters have samples on both sides
    // reference: 16x, ideal band-limit by spectrum truncation
    let xr = sine(n, bin, amp, 16);
    let yr = naive(s, &xr);
    let spr = spectrum(&yr); // n*16/2 bins; bin index k at base rate <-> k at 16x too (same cycles per block)
    let sp = spectrum(&y);
    let kmax = n / 2;
    // error power over audible band < 20 kHz relative to fundamental
    let lim = ((20_000.0 / fs) * n as f64) as usize;
    let _ = &spr;
    let f = sp[bin];
    // Aliased components land between the true harmonics. Sum the energy in every audible bin that is not
    // (within 2 bins of) a harmonic of the tone: that is aliasing (plus a numerical floor), independent of any
    // filter droop on the harmonics themselves.
    let err: f64 = (1..lim.min(kmax)).filter(|k| { let r = k % bin; r > 2 && r < bin - 2 }).map(|k| sp[k] * sp[k]).sum::<f64>().sqrt();
    let e = db(err / f);
    println!("    {name:<26} inharmonic (alias) level: {:>7.1} dB", e);
    e
}

fn timeit(name: &str, fs: f64, f: &mut dyn FnMut()) {
    let secs_audio = 4.0; // process 4 s of stereo audio per rep
    let _ = secs_audio;
    let t = Instant::now(); let reps = 5; for _ in 0..reps { f(); } let dt = t.elapsed().as_secs_f64() / reps as f64;
    let _ = fs;
    println!("    {name:<34} {:>8.2} ms per 1 s of stereo audio  -> {:>6.1}% of one core", dt * 1e3, dt * 100.0);
}

fn main() {
    println!("=== 1. Harmonic profile (dBc relative to the fundamental) ===");
    let tri = Triode::new(300.0, 100_000.0, -1.5, 1.5);
    for amp in [0.1, 0.3, 0.6, 1.0] {
        harmonics("asym tanh (bias 0.4, g=1.5)", &AsymTanh { g: 1.5, b: 0.4 }, amp);
    }
    for amp in [0.1, 0.3, 0.6, 1.0] { harmonics("sym tanh (g=2)", &SymTanh { g: 2.0 }, amp); }
    for amp in [0.1, 0.3, 0.6, 1.0] { harmonics("hard clip (g=2)", &HardClip { g: 2.0 }, amp); }
    for amp in [0.1, 0.3, 0.6, 1.0] { harmonics("12AX7 stage (Koren, RL 100k)", &TriodeDrive { t: &tri, g: 1.0 }, amp); }

    println!("\n=== 2. Aliasing (error vs ideal band-limited reference, audible band) ===");
    for (fs, f0) in [(44_100.0, 6_000.0), (44_100.0, 9_500.0), (96_000.0, 9_500.0)] {
        let amp = 0.8;
        let s = AsymTanh { g: 3.0, b: 0.4 };
        println!("  fs {fs} Hz, tone {f0} Hz, asym tanh drive g=3:");
        alias_test("naive (no protection)", &s, amp, fs, f0, &|x| naive(&s, x));
        alias_test("ADAA 1st order", &s, amp, fs, f0, &|x| adaa1(&s, x));
        alias_test("2x oversampling", &s, amp, fs, f0, &|x| oversampled(&s, x, 2));
        alias_test("4x oversampling", &s, amp, fs, f0, &|x| oversampled(&s, x, 4));
        alias_test("8x oversampling", &s, amp, fs, f0, &|x| oversampled(&s, x, 8));
    }

    println!("\n=== 3. CPU cost (1 s of stereo audio, this machine, release build) ===");
    use kahawai_player_core::{EqBand, EqBandType, ParametricEq};
    for fs in [44_100usize, 96_000, 192_000] {
        println!("  {fs} Hz:");
        let mono: Vec<f64> = (0..fs).map(|i| 0.5 * (2.0 * PI * 1000.0 * i as f64 / fs as f64).sin()).collect();
        let stereo_f32: Vec<f32> = mono.iter().flat_map(|&v| [v as f32, v as f32]).collect();
        let bands: Vec<EqBand> = (0..8).map(|i| EqBand { band_type: EqBandType::Peaking, freq: 100.0 * 2f32.powi(i), gain_db: 3.0, q: 1.0 }).collect();
        let mut eq = ParametricEq::new(fs as u32); eq.set_bands(bands).unwrap();
        let mut buf = stereo_f32.clone();
        eq.process(&mut buf[..4096 * 2], 2); // prime
        timeit("existing 8-band EQ (f32 API)", fs as f64, &mut || { let mut b = stereo_f32.clone(); for c in b.chunks_mut(4096 * 2) { eq.process(c, 2); } std::hint::black_box(&b); });
        let s = AsymTanh { g: 3.0, b: 0.4 };
        let tri = Triode::new(300.0, 100_000.0, -1.5, 1.5); let td = TriodeDrive { t: &tri, g: 2.0 };
        timeit("naive asym-tanh x2ch", fs as f64, &mut || { for _ in 0..2 { std::hint::black_box(naive(&s, &mono)); } });
        timeit("triode table lookup x2ch", fs as f64, &mut || { for _ in 0..2 { std::hint::black_box(naive(&td, &mono)); } });
        timeit("ADAA1 tanh (analytic) x2ch", fs as f64, &mut || { for _ in 0..2 { std::hint::black_box(adaa1(&s, &mono)); } });
        timeit("2x oversampled tanh x2ch", fs as f64, &mut || { for _ in 0..2 { std::hint::black_box(oversampled(&s, &mono, 2)); } });
        timeit("4x oversampled tanh x2ch", fs as f64, &mut || { for _ in 0..2 { std::hint::black_box(oversampled(&s, &mono, 4)); } });
    }
}
