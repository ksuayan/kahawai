//! Internet radio through the engine: a station is a queue item with a
//! negative id whose `path` is its address. No network, no audio hardware.

use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kahawai_core::{format::AudioFormat, MusicError, Track};
use kahawai_player_core::{
    AudioSink, OutputPath, PcmChunk, Player, PlayerStatus, StationStream, StreamInfo,
    StreamOptions, Transport, VecSink,
};

#[derive(Clone)]
struct SharedSink(Arc<Mutex<VecSink>>);

impl AudioSink for SharedSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.0.lock().unwrap().open(track)
    }
    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        self.0.lock().unwrap().write(chunk)
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
    fn state(&self) -> kahawai_player_core::SinkState {
        self.0.lock().unwrap().state()
    }
    fn preferred_sample_rate(&self) -> Option<u32> {
        self.0.lock().unwrap().preferred_sample_rate()
    }
    fn buffered_frames(&self) -> u64 {
        self.0.lock().unwrap().buffered_frames()
    }
    fn set_output_device(&mut self, name: Option<&str>) {
        self.0.lock().unwrap().set_output_device(name)
    }
    fn drain(&mut self) {
        self.0.lock().unwrap().drain()
    }
    fn exclusive_pcm_rate(&self, rate_hz: u32) -> Option<u32> {
        self.0.lock().unwrap().exclusive_pcm_rate(rate_hz)
    }
    fn output_is_external_dac(&self) -> bool {
        self.0.lock().unwrap().external
    }
    fn open_exclusive_pcm(&mut self, rate_hz: u32, channels: u16) -> Result<(), MusicError> {
        self.0.lock().unwrap().open_exclusive_pcm(rate_hz, channels)
    }
    fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
        self.0.lock().unwrap().write_dop(bytes)
    }
    fn select_output_path(&mut self, path: OutputPath) {
        self.0.lock().unwrap().select_output_path(path)
    }
}

fn wav(freq: f32, frames: usize) -> Vec<u8> {
    let rate = 44_100u32;
    let data: Vec<u8> = (0..frames)
        .flat_map(|j| {
            let s = (2.0 * std::f32::consts::PI * freq * j as f32 / rate as f32).sin() * 0.5;
            let q = ((s * 32767.0) as i16).to_le_bytes();
            [q, q].concat()
        })
        .collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// What the stub station does on each connection.
#[derive(Clone)]
enum Behaviour {
    /// Serve this body (then the connection ends), announcing a title.
    Serve(Vec<u8>),
    /// Refuse the connection.
    Refuse,
}

struct StubStation {
    behaviour: Mutex<Behaviour>,
    connections: AtomicU32,
    urls: Mutex<Vec<String>>,
}

struct Wrap(Arc<StubStation>);

impl Transport for Wrap {
    fn open_stream(&self, track_id: i64, _o: &StreamOptions) -> Result<StreamInfo, MusicError> {
        panic!("a station must not ask the server for track {track_id}");
    }
    fn open_station(
        &self,
        url: &str,
        mut on_title: Box<dyn FnMut(String) + Send>,
    ) -> Result<StationStream, MusicError> {
        let me = &self.0;
        me.connections.fetch_add(1, Ordering::SeqCst);
        me.urls.lock().unwrap().push(url.to_string());
        match me.behaviour.lock().unwrap().clone() {
            Behaviour::Refuse => Err(MusicError::Http("stub: refused".into())),
            Behaviour::Serve(body) => {
                on_title("Test Artist - Test Song".into());
                Ok(StationStream {
                    reader: Box::new(Cursor::new(body)),
                    content_type: "audio/wav".into(),
                    name: Some("Test FM".into()),
                    bitrate: Some(128),
                })
            }
        }
    }
}

fn station(id: i64) -> Track {
    Track {
        id,
        path: "http://stub.example/live".into(),
        hash: None,
        format: AudioFormat::Wav,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        duration_ms: None,
        bitrate: None,
        title: Some("Test FM".into()),
        album: None,
        artist: None,
        album_id: None,
        track_no: None,
        disc_no: None,
        genre: None,
        year: None,
        missing: false,
        decodable: true,
        mqa: false,
        original_sample_rate: None,
    }
}

fn player(b: Behaviour) -> (Player, Arc<StubStation>, SharedSink) {
    let stub = Arc::new(StubStation {
        behaviour: Mutex::new(b),
        connections: AtomicU32::new(0),
        urls: Mutex::new(Vec::new()),
    });
    let sink = SharedSink(Arc::new(Mutex::new(VecSink::new())));
    let mut p = Player::new(Box::new(sink.clone()), Box::new(Wrap(stub.clone())));
    p.set_radio_backoff_ms_per_s(2);
    (p, stub, sink)
}

#[test]
fn a_station_plays_and_reports_its_title_and_bitrate() {
    let (mut p, stub, sink) = player(Behaviour::Serve(wav(440.0, 22_050)));
    p.play_queue(vec![station(-3)], 0);
    for _ in 0..40 {
        p.pump();
    }
    let s = p.snapshot();
    assert_eq!(stub.urls.lock().unwrap()[0], "http://stub.example/live");
    assert!(
        !sink.0.lock().unwrap().samples.is_empty(),
        "audio reached the sink"
    );
    let radio = s.radio.expect("a station is playing");
    assert_eq!(radio.title.as_deref(), Some("Test Artist - Test Song"));
    assert_eq!(radio.bitrate_kbps, Some(128));
}

#[test]
fn music_has_no_radio_state() {
    let (mut p, _stub, _sink) = player(Behaviour::Refuse);
    assert!(p.snapshot().radio.is_none());
    p.stop();
    assert!(p.snapshot().radio.is_none());
}

#[test]
fn a_dropped_station_is_reconnected_not_skipped_or_stopped() {
    // The body ends after a fraction of a second, as a dropped connection does.
    let (mut p, stub, _sink) = player(Behaviour::Serve(wav(440.0, 4_410)));
    p.play_queue(vec![station(-1)], 0);
    let mut saw_reconnecting = false;
    for _ in 0..4_000 {
        p.pump();
        if p.snapshot().radio.as_ref().is_some_and(|r| r.reconnecting) {
            saw_reconnecting = true;
        }
        if stub.connections.load(Ordering::SeqCst) >= 4 {
            break;
        }
    }
    assert!(
        stub.connections.load(Ordering::SeqCst) >= 4,
        "kept reconnecting"
    );
    assert!(saw_reconnecting, "the reconnecting state was visible");
    let s = p.snapshot();
    assert_eq!(s.status, PlayerStatus::Playing, "never stopped");
    assert!(s.error.is_none());
}

#[test]
fn a_station_that_never_connects_gives_up_with_a_reason() {
    let (mut p, stub, _sink) = player(Behaviour::Refuse);
    p.play_queue(vec![station(-2)], 0);
    for _ in 0..4_000 {
        p.pump();
        if p.status() == PlayerStatus::Stopped {
            break;
        }
    }
    let s = p.snapshot();
    assert_eq!(s.status, PlayerStatus::Stopped);
    assert!(s.error.is_some(), "says why");
    assert_eq!(
        stub.connections.load(Ordering::SeqCst),
        4,
        "the first try and three more"
    );
    assert!(s.radio.is_none());
}

#[test]
fn seeking_a_live_station_does_nothing() {
    let (mut p, stub, _sink) = player(Behaviour::Serve(wav(440.0, 44_100 * 30)));
    p.play_queue(vec![station(-5)], 0);
    for _ in 0..10 {
        p.pump();
    }
    let before = stub.connections.load(Ordering::SeqCst);
    p.seek_ms(30_000);
    for _ in 0..10 {
        p.pump();
    }
    assert_eq!(
        stub.connections.load(Ordering::SeqCst),
        before,
        "no reopen for a seek"
    );
}
