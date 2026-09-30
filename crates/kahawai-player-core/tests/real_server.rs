//! Manual: plays two consecutive tracks from a real Kahawai server through the
//! real engine in bit-perfect mode into a capturing sink (no audio device), then
//! compares the two tracks' sample statistics. Noise, clipping or a shifted
//! byte alignment on the second track would show up as a wildly different
//! high-frequency ratio or full-scale fraction.
//!
//!   KAHAWAI_SERVER=http://10.0.0.233:8080 KAHAWAI_TRACKS=/path/tracks.json \
//!     cargo test -p kahawai-player-core --test real_server -- --ignored --nocapture

use std::sync::{Arc, Mutex};

use kahawai_core::{MusicError, Track};
use kahawai_player_core::{
    integrated_lufs, AudioSink, BitPerfect, HttpTransport, OutputPath, PcmChunk, Player,
    PlayerStatus, SinkState, VecSink, MAX_LOUDNESS_GAIN_DB, MIN_LOUDNESS_GAIN_DB,
};

#[derive(Clone)]
struct Shared(Arc<Mutex<VecSink>>);

impl AudioSink for Shared {
    fn open(&mut self, t: &Track) -> Result<(), MusicError> {
        self.0.lock().unwrap().open(t)
    }
    fn write(&mut self, c: PcmChunk) -> Result<(), MusicError> {
        self.0.lock().unwrap().write(c)
    }
    fn play(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().play()
    }
    fn pause(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().pause()
    }
    fn stop(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().stop()
    }
    fn state(&self) -> SinkState {
        self.0.lock().unwrap().state()
    }
    fn buffered_frames(&self) -> u64 {
        self.0.lock().unwrap().buffered_frames()
    }
    fn drain(&mut self) {
        self.0.lock().unwrap().drain()
    }
    fn exclusive_pcm_rate(&self, r: u32) -> Option<u32> {
        self.0.lock().unwrap().exclusive_pcm_rate(r)
    }
    fn open_exclusive_pcm(&mut self, r: u32, c: u16) -> Result<(), MusicError> {
        self.0.lock().unwrap().open_exclusive_pcm(r, c)
    }
    fn write_dop(&mut self, b: &[u8]) -> Result<(), MusicError> {
        self.0.lock().unwrap().write_dop(b)
    }
    fn select_output_path(&mut self, p: OutputPath) {
        self.0.lock().unwrap().select_output_path(p)
    }
    fn output_is_external_dac(&self) -> bool {
        true
    }
}

struct Stats {
    frames: usize,
    rms_db: f64,
    peak: f64,
    full_scale_frac: f64,
    /// Mean |x[n]-x[n-1]| over mean |x|: tonal music is well below 1, broadband noise is near or above 1.
    hf_ratio: f64,
    /// Left/right correlation: real stereo music is strongly positive; misaligned or noisy data is near 0.
    lr_corr: f64,
}

fn stats(bytes: &[u8]) -> Stats {
    let n = bytes.len() / 6;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for f in 0..n {
        let s = |o: usize| {
            let v = i32::from_le_bytes([0, bytes[o], bytes[o + 1], bytes[o + 2]]) >> 8;
            v as f64 / 8_388_608.0
        };
        l.push(s(f * 6));
        r.push(s(f * 6 + 3));
    }
    let mean_abs = |v: &[f64]| v.iter().map(|x| x.abs()).sum::<f64>() / v.len().max(1) as f64;
    let rms = (l.iter().map(|x| x * x).sum::<f64>() / n.max(1) as f64).sqrt();
    let peak = l.iter().chain(r.iter()).fold(0.0f64, |a, x| a.max(x.abs()));
    let full =
        l.iter().chain(r.iter()).filter(|x| x.abs() > 0.99).count() as f64 / (2 * n).max(1) as f64;
    let hf = l.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>()
        / (n.max(2) - 1) as f64
        / mean_abs(&l).max(1e-12);
    let (sl, sr, slr) = l.iter().zip(&r).fold((0.0, 0.0, 0.0), |a, (x, y)| {
        (a.0 + x * x, a.1 + y * y, a.2 + x * y)
    });
    Stats {
        frames: n,
        rms_db: 20.0 * rms.max(1e-12).log10(),
        peak,
        full_scale_frac: full,
        hf_ratio: hf,
        lr_corr: slr / (sl * sr).sqrt().max(1e-12),
    }
}

#[test]
#[ignore]
fn consecutive_tracks_from_a_real_server_have_the_same_character() {
    let url = std::env::var("KAHAWAI_SERVER").expect("KAHAWAI_SERVER");
    let path = std::env::var("KAHAWAI_TRACKS").expect("KAHAWAI_TRACKS");
    let tracks: Vec<Track> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(tracks.len() >= 2);

    let sink = Shared(Arc::new(Mutex::new(VecSink::new())));
    sink.0.lock().unwrap().exclusive_rates = vec![44_100];
    let mut p = Player::new(Box::new(sink.clone()), Box::new(HttpTransport::new(url)));
    p.set_bit_perfect(BitPerfect::All);
    p.play_queue(tracks[..2].to_vec(), 0);

    let mut boundary = None;
    let mut guard = 0;
    while p.status() == PlayerStatus::Playing && guard < 2_000_000 {
        p.pump();
        guard += 1;
        if boundary.is_none() && p.snapshot().current_id == Some(tracks[1].id) {
            boundary = Some(sink.0.lock().unwrap().exclusive_bytes.len());
        }
    }
    let all = sink.0.lock().unwrap().exclusive_bytes.clone();
    let snap = p.snapshot();
    println!(
        "final status {:?} error {:?} notice {:?} path {:?}",
        p.status(),
        snap.error,
        snap.notice,
        snap.output_path
    );
    let cut = boundary.expect("reached track 2") / 6 * 6;
    let (t1, t2) = all.split_at(cut);
    let (a, b) = (stats(t1), stats(t2));
    // What the shared path (loudness normalization at -14 LUFS, +12 dB max, no limiter) would do.
    for (name, bytes) in [("track 1", t1), ("track 2", t2)] {
        let n = bytes.len() / 6;
        let mut inter = Vec::with_capacity(n * 2);
        for f in 0..n * 2 {
            let o = f * 3;
            inter.push(
                (i32::from_le_bytes([0, bytes[o], bytes[o + 1], bytes[o + 2]]) >> 8) as f32
                    / 8_388_608.0,
            );
        }
        if let Some(lufs) = integrated_lufs(&inter, 2, 44_100) {
            let gain_db = (-14.0 - lufs).clamp(MIN_LOUDNESS_GAIN_DB, MAX_LOUDNESS_GAIN_DB);
            let g = 10f32.powf(gain_db / 20.0);
            let over =
                inter.iter().filter(|x| (x.abs() * g) > 1.0).count() as f64 / inter.len() as f64;
            let peak = inter.iter().fold(0.0f32, |a, x| a.max(x.abs())) * g;
            println!("{name}: {lufs:.1} LUFS -> gain {gain_db:+.1} dB -> peak {peak:.2} FS, {:.3}% of samples clipped without a limiter", over * 100.0);
        }
    }
    for (name, s) in [("track 1", &a), ("track 2", &b)] {
        println!(
            "{name}: {} frames ({:.0}s)  rms {:.1} dBFS  peak {:.3}  >0.99FS {:.5}  hf_ratio {:.3}  L/R corr {:.3}",
            s.frames, s.frames as f64 / 44100.0, s.rms_db, s.peak, s.full_scale_frac, s.hf_ratio, s.lr_corr
        );
    }
}

/// The same two tracks through the *shared* path with the user's own EQ and
/// loudness settings, counting samples the device would hard-clip (|x| > 1.0).
#[test]
#[ignore]
fn shared_path_with_the_users_eq_and_loudness_clips_track_two() {
    use kahawai_player_core::{EqBand, EqBandType};
    let url = std::env::var("KAHAWAI_SERVER").expect("KAHAWAI_SERVER");
    let path = std::env::var("KAHAWAI_TRACKS").expect("KAHAWAI_TRACKS");
    let tracks: Vec<Track> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

    let sink = Shared(Arc::new(Mutex::new(VecSink::new())));
    let mut p = Player::new(Box::new(sink.clone()), Box::new(HttpTransport::new(url)));
    p.set_bit_perfect(BitPerfect::Off);
    let b = |t, f, g, q| EqBand {
        band_type: t,
        freq: f,
        gain_db: g,
        q,
    };
    p.set_eq_bands(vec![
        b(EqBandType::LowShelf, 100.0, 2.5, 0.7),
        b(EqBandType::Peaking, 250.0, 1.5, 1.0),
        b(EqBandType::Peaking, 3000.0, 1.5, 1.0),
        b(EqBandType::HighShelf, 10000.0, 2.0, 0.7),
    ])
    .unwrap();
    p.set_eq_enabled(true);
    p.set_loudness_enabled(true);
    p.set_loudness_target(-14.0);
    p.play_queue(tracks[..2].to_vec(), 0);

    let mut boundary = None;
    let mut guard = 0;
    while p.status() == PlayerStatus::Playing && guard < 2_000_000 {
        p.pump();
        guard += 1;
        if boundary.is_none() && p.snapshot().current_id == Some(tracks[1].id) {
            boundary = Some(sink.0.lock().unwrap().samples.len());
        }
    }
    let all = sink.0.lock().unwrap().samples.clone();
    let cut = boundary.expect("reached track 2") / 2 * 2;
    for (name, s) in [("track 1", &all[..cut]), ("track 2", &all[cut..])] {
        let over = s.iter().filter(|x| x.abs() > 1.0).count();
        let peak = s.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        println!(
            "{name}: peak {peak:.2} FS, {over} samples over full scale ({:.3}% of {} samples)",
            over as f64 * 100.0 / s.len().max(1) as f64,
            s.len()
        );
    }
}
