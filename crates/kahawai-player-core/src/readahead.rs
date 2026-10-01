//! Read-ahead buffer for network streams.
//!
//! The playback thread decodes straight off the HTTP body, so without this a
//! stalled network read (a Wi-Fi dropout, a busy server) blocks decoding the
//! moment the sink's short ring buffer runs dry. [`ReadAhead`] moves the
//! network read onto its own thread, which keeps a bounded queue of bytes
//! filled ahead of the playhead; the playback thread reads from the queue and
//! only waits when the queue is genuinely empty.
//!
//! Properties worth knowing:
//! - **No pre-roll.** `read` returns as soon as any byte is queued, so playback
//!   starts as fast as before; the buffer fills while the first seconds play.
//! - **Bounded.** The fetch thread stops once `capacity` bytes are queued
//!   (backpressure) and resumes as the reader drains them.
//! - **Order-preserving errors.** A network error is delivered after every
//!   byte that arrived before it.
//! - **Cancels on drop.** Seeking or changing track drops the reader; the
//!   fetch thread then stops and releases the connection (immediately when it
//!   is waiting for space, at its next read when it is waiting on the network).

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Bytes the fetch thread asks the network for at a time.
const CHUNK: usize = 64 * 1024;

/// A speed sample closes once this much time has been spent waiting on the
/// network, or this many bytes have arrived, whichever comes first.
const SAMPLE_TIME: Duration = Duration::from_millis(200);
const SAMPLE_BYTES: u64 = 1024 * 1024;

/// What the read-ahead is doing, readable from any thread.
#[derive(Debug)]
pub struct ReadAheadStats {
    /// Smoothed network speed in bytes/s while data was being fetched; 0 until
    /// the first sample.
    rate_bps: AtomicU64,
    /// Bytes fetched but not yet read.
    queued: AtomicUsize,
    /// The whole response has been fetched (clean end of stream).
    finished: AtomicBool,
    capacity: usize,
}

impl ReadAheadStats {
    /// How fast the network delivers when asked, in bytes per second. This is
    /// measured only over the time spent waiting on the network, so a full
    /// buffer (the fetcher idle) does not read as a slow connection. `None`
    /// until enough has arrived to say.
    pub fn rate_bps(&self) -> Option<u64> {
        match self.rate_bps.load(Ordering::Relaxed) {
            0 => None,
            r => Some(r),
        }
    }

    /// Bytes fetched from the network that have not been read yet.
    pub fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    /// True once the entire response is in the buffer or already read, so
    /// whatever is queued is all that is left and a short queue is not a
    /// sign of a slow connection.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// The most the buffer will hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Debug)]
enum End {
    Eof,
    Failed(io::ErrorKind, String),
}

#[derive(Default)]
struct State {
    queue: VecDeque<Vec<u8>>,
    queued: usize,
    /// Set once by the fetch thread when it stops (clean end or error).
    end: Option<End>,
    /// Set when the reader is dropped.
    cancelled: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when data arrives or the fetch thread ends.
    readable: Condvar,
    /// Signalled when the reader frees space or cancels.
    writable: Condvar,
}

/// A [`Read`] that is fed by a background thread. See the module docs.
pub struct ReadAhead {
    shared: Arc<Shared>,
    stats: Arc<ReadAheadStats>,
    /// The chunk currently being consumed.
    current: Vec<u8>,
    pos: usize,
    /// Kept so the thread is not detached without an owner; never joined (the
    /// thread may be blocked on the network, and dropping must not wait).
    _fetcher: JoinHandle<()>,
}

impl ReadAhead {
    /// Start filling a queue of at most about `capacity` bytes from `source`.
    pub fn new<R: Read + Send + 'static>(source: R, capacity: usize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            readable: Condvar::new(),
            writable: Condvar::new(),
        });
        let capacity = capacity.max(CHUNK);
        let stats = Arc::new(ReadAheadStats {
            rate_bps: AtomicU64::new(0),
            queued: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
            capacity,
        });
        let fetcher = {
            let (shared, stats) = (shared.clone(), stats.clone());
            thread::Builder::new()
                .name("stream-read-ahead".into())
                .spawn(move || fetch(source, &shared, &stats, capacity))
                .expect("spawn read-ahead thread")
        };
        Self {
            shared,
            stats,
            current: Vec::new(),
            pos: 0,
            _fetcher: fetcher,
        }
    }

    /// Live figures for a gauge; stays valid after the reader is gone.
    pub fn stats(&self) -> Arc<ReadAheadStats> {
        self.stats.clone()
    }
}

/// The fetch thread: wait for room, read a chunk, queue it; repeat.
fn fetch<R: Read>(mut source: R, shared: &Shared, stats: &ReadAheadStats, capacity: usize) {
    // The open speed sample: bytes that arrived and time spent waiting for them.
    let (mut sample_bytes, mut sample_time) = (0u64, Duration::ZERO);
    loop {
        // Wait for space *before* reading, so no more than `capacity` (plus
        // nothing in flight) is ever held, and a cancel is seen promptly.
        {
            let mut st = shared.state.lock().expect("read-ahead lock");
            while st.queued >= capacity && !st.cancelled {
                st = shared.writable.wait(st).expect("read-ahead lock");
            }
            if st.cancelled {
                return;
            }
        }
        let mut chunk = vec![0u8; CHUNK];
        let started = Instant::now();
        let result = source.read(&mut chunk);
        let waited = started.elapsed();
        let mut st = shared.state.lock().expect("read-ahead lock");
        if st.cancelled {
            return;
        }
        match result {
            Ok(0) => {
                st.end = Some(End::Eof);
                stats.finished.store(true, Ordering::Relaxed);
                shared.readable.notify_all();
                return;
            }
            Ok(n) => {
                chunk.truncate(n);
                st.queued += n;
                stats.queued.store(st.queued, Ordering::Relaxed);
                st.queue.push_back(chunk);
                shared.readable.notify_all();
                sample_bytes += n as u64;
                sample_time += waited;
                if sample_time >= SAMPLE_TIME || sample_bytes >= SAMPLE_BYTES {
                    // Exponentially smoothed, so the figure is steady but
                    // follows a real change within a second or two.
                    let secs = sample_time.as_secs_f64().max(0.001);
                    let instant = sample_bytes as f64 / secs;
                    let prev = stats.rate_bps.load(Ordering::Relaxed);
                    let next = if prev == 0 {
                        instant
                    } else {
                        0.7 * prev as f64 + 0.3 * instant
                    };
                    stats.rate_bps.store(next as u64, Ordering::Relaxed);
                    (sample_bytes, sample_time) = (0, Duration::ZERO);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                st.end = Some(End::Failed(e.kind(), e.to_string()));
                shared.readable.notify_all();
                return;
            }
        }
    }
}

impl Read for ReadAhead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.current.len() {
            let mut st = self.shared.state.lock().expect("read-ahead lock");
            loop {
                if let Some(next) = st.queue.pop_front() {
                    st.queued -= next.len();
                    self.stats.queued.store(st.queued, Ordering::Relaxed);
                    self.shared.writable.notify_all();
                    self.current = next;
                    self.pos = 0;
                    break;
                }
                match &st.end {
                    // Everything before the end has been delivered.
                    Some(End::Eof) => return Ok(0),
                    Some(End::Failed(kind, msg)) => return Err(io::Error::new(*kind, msg.clone())),
                    None => st = self.shared.readable.wait(st).expect("read-ahead lock"),
                }
            }
        }
        let n = buf.len().min(self.current.len() - self.pos);
        buf[..n].copy_from_slice(&self.current[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Drop for ReadAhead {
    fn drop(&mut self) {
        let mut st = self.shared.state.lock().expect("read-ahead lock");
        st.cancelled = true;
        st.queue.clear();
        st.queued = 0;
        self.stats.queued.store(0, Ordering::Relaxed);
        self.shared.writable.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Poll `f` for up to 3 s.
    fn eventually(f: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    /// Deterministic bytes so order mistakes show up.
    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
    }

    /// Yields `data`, counting bytes handed out and flagging when dropped.
    struct Source {
        data: Vec<u8>,
        at: usize,
        taken: Arc<AtomicUsize>,
        dropped: Arc<AtomicBool>,
    }

    impl Source {
        fn new(data: Vec<u8>) -> (Self, Arc<AtomicUsize>, Arc<AtomicBool>) {
            let taken = Arc::new(AtomicUsize::new(0));
            let dropped = Arc::new(AtomicBool::new(false));
            (
                Self {
                    data,
                    at: 0,
                    taken: taken.clone(),
                    dropped: dropped.clone(),
                },
                taken,
                dropped,
            )
        }
    }

    impl Read for Source {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            self.taken.fetch_add(n, Ordering::SeqCst);
            Ok(n)
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn passes_every_byte_through_in_order_whatever_the_read_size() {
        let want = data(1_000_003); // not a multiple of the chunk size
        let (src, _, _) = Source::new(want.clone());
        let mut ra = ReadAhead::new(src, 256 * 1024);
        let mut got = Vec::new();
        let mut buf = [0u8; 7919]; // an awkward, prime-sized read
        loop {
            let n = ra.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, want);
        assert_eq!(ra.read(&mut buf).unwrap(), 0, "stays at EOF");
    }

    #[test]
    fn fills_ahead_of_the_reader_up_to_its_capacity_and_no_further() {
        let cap = 256 * 1024;
        let (src, taken, _) = Source::new(data(4 * 1024 * 1024));
        let ra = ReadAhead::new(src, cap);
        // The reader has not read a byte, yet the fetcher fills the buffer...
        assert!(
            eventually(|| taken.load(Ordering::SeqCst) >= cap),
            "reads ahead"
        );
        thread::sleep(Duration::from_millis(100));
        // ...and then stops: backpressure holds it at the capacity.
        let got = taken.load(Ordering::SeqCst);
        assert!(
            got <= cap + CHUNK,
            "stayed bounded: {got} bytes for a {cap} byte buffer"
        );
        drop(ra);
    }

    #[test]
    fn draining_the_queue_lets_the_fetcher_continue() {
        let cap = 128 * 1024;
        let (src, taken, _) = Source::new(data(2 * 1024 * 1024));
        let mut ra = ReadAhead::new(src, cap);
        assert!(eventually(|| taken.load(Ordering::SeqCst) >= cap));
        let before = taken.load(Ordering::SeqCst);
        let mut sink = vec![0u8; 64 * 1024];
        for _ in 0..8 {
            ra.read_exact(&mut sink).unwrap();
        }
        assert!(
            eventually(|| taken.load(Ordering::SeqCst) > before),
            "reading frees room, so the fetcher goes on"
        );
    }

    /// Hands out one chunk, then blocks until released: a network stall.
    struct Stalling {
        first: Option<Vec<u8>>,
        release: Arc<AtomicBool>,
    }

    impl Read for Stalling {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(first) = self.first.take() {
                let n = first.len().min(buf.len());
                buf[..n].copy_from_slice(&first[..n]);
                return Ok(n);
            }
            while !self.release.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(0)
        }
    }

    #[test]
    fn read_returns_what_is_queued_without_waiting_for_the_buffer_to_fill() {
        // No pre-roll: with one chunk in and the network stalled, the first
        // read must come back at once rather than wait for more.
        let release = Arc::new(AtomicBool::new(false));
        let mut ra = ReadAhead::new(
            Stalling {
                first: Some(data(1000)),
                release: release.clone(),
            },
            1024 * 1024,
        );
        let started = Instant::now();
        let mut buf = [0u8; 4096];
        let n = ra.read(&mut buf).unwrap();
        assert_eq!(n, 1000);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "did not wait for a fill"
        );
        release.store(true, Ordering::SeqCst);
        assert_eq!(ra.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn buffered_audio_keeps_playing_through_a_network_stall() {
        // The point of the feature: bytes fetched before the stall are still
        // served while the network is down.
        let release = Arc::new(AtomicBool::new(false));
        let mut ra = ReadAhead::new(
            Stalling {
                first: Some(data(CHUNK)),
                release: release.clone(),
            },
            1024 * 1024,
        );
        // Let the fetcher bank its chunk and block on the stalled network.
        thread::sleep(Duration::from_millis(100));
        let mut buf = vec![0u8; CHUNK];
        let started = Instant::now();
        ra.read_exact(&mut buf).unwrap(); // served from the buffer, not the network
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(buf, data(CHUNK));
        release.store(true, Ordering::SeqCst);
    }

    /// Hands out some bytes, then fails.
    struct Failing {
        good: Option<Vec<u8>>,
    }

    impl Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.good.take() {
                Some(g) => {
                    buf[..g.len()].copy_from_slice(&g);
                    Ok(g.len())
                }
                None => Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "reset by peer",
                )),
            }
        }
    }

    #[test]
    fn an_error_arrives_after_the_bytes_that_preceded_it() {
        let mut ra = ReadAhead::new(
            Failing {
                good: Some(data(500)),
            },
            1024 * 1024,
        );
        let mut buf = [0u8; 100];
        let mut got = 0;
        while got < 500 {
            got += ra.read(&mut buf).unwrap(); // all of the good bytes first
        }
        let err = ra.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        assert!(err.to_string().contains("reset by peer"));
        assert_eq!(
            ra.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::ConnectionReset,
            "and it sticks"
        );
    }

    #[test]
    fn dropping_the_reader_stops_the_fetcher_and_releases_the_source() {
        // A seek or track change drops the reader mid-stream; the connection
        // behind it must be released, not left filling a buffer nobody reads.
        let (src, taken, dropped) = Source::new(data(64 * 1024 * 1024));
        let ra = ReadAhead::new(src, 128 * 1024);
        assert!(eventually(|| taken.load(Ordering::SeqCst) >= 128 * 1024));
        drop(ra);
        assert!(
            eventually(|| dropped.load(Ordering::SeqCst)),
            "source released"
        );
        let after = taken.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(taken.load(Ordering::SeqCst), after, "no further reads");
    }

    #[test]
    fn an_empty_source_is_an_immediate_eof() {
        let (src, _, _) = Source::new(Vec::new());
        let mut ra = ReadAhead::new(src, 1024);
        let mut buf = [0u8; 16];
        assert_eq!(ra.read(&mut buf).unwrap(), 0);
        assert_eq!(
            ra.read(&mut []).unwrap(),
            0,
            "a zero-length read is a no-op"
        );
    }

    /// Hands out 10 KB per read, taking 20 ms each: a ~500 KB/s link.
    struct Paced {
        left: usize,
    }

    impl Read for Paced {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Ok(0);
            }
            thread::sleep(Duration::from_millis(20));
            let n = buf.len().min(10_000).min(self.left);
            buf[..n].fill(7);
            self.left -= n;
            Ok(n)
        }
    }

    #[test]
    fn measures_the_network_speed_while_fetching() {
        let mut ra = ReadAhead::new(Paced { left: 1_000_000 }, 4 * 1024 * 1024);
        let stats = ra.stats();
        assert_eq!(stats.rate_bps(), None, "nothing measured yet");
        assert!(
            eventually(|| stats.rate_bps().is_some()),
            "a sample closes within a few hundred ms"
        );
        // The link is about 500 KB/s; scheduling jitter allows a wide band.
        let rate = stats.rate_bps().unwrap();
        assert!((200_000..1_500_000).contains(&rate), "rate {rate} B/s");
        let mut sink = Vec::new();
        ra.read_to_end(&mut sink).unwrap();
        assert_eq!(sink.len(), 1_000_000);
        assert!(
            stats.rate_bps().is_some(),
            "the last speed stays readable after the stream ends"
        );
    }

    #[test]
    fn a_full_buffer_does_not_look_like_a_slow_connection() {
        // The fetcher idles on a full buffer; that must not drag the speed down.
        let cap = 2 * 1024 * 1024; // more than one speed sample's worth of bytes
        let (src, taken, _) = Source::new(data(16 * 1024 * 1024));
        let ra = ReadAhead::new(src, cap);
        let stats = ra.stats();
        assert!(eventually(|| taken.load(Ordering::SeqCst) >= cap));
        assert!(eventually(|| stats.rate_bps().is_some()));
        let before = stats.rate_bps().unwrap();
        thread::sleep(Duration::from_millis(400)); // idle, buffer full
        assert_eq!(stats.rate_bps().unwrap(), before, "unchanged while idle");
        drop(ra);
    }

    #[test]
    fn reports_how_much_is_buffered() {
        let cap = 256 * 1024;
        let (src, taken, _) = Source::new(data(4 * 1024 * 1024));
        let mut ra = ReadAhead::new(src, cap);
        let stats = ra.stats();
        assert_eq!(stats.capacity(), cap);
        assert!(eventually(|| stats.queued_bytes() >= cap));
        assert!(stats.queued_bytes() <= cap + CHUNK);
        let full = stats.queued_bytes();
        let mut buf = vec![0u8; 100 * 1024];
        ra.read_exact(&mut buf).unwrap();
        // Reading frees bytes, so the count drops (the fetcher may refill some).
        assert!(taken.load(Ordering::SeqCst) >= full);
        drop(ra);
        assert_eq!(stats.queued_bytes(), 0, "empty once the reader is gone");
    }

    #[test]
    fn knows_when_the_whole_response_has_been_fetched() {
        let (src, _, _) = Source::new(data(300_000));
        let mut ra = ReadAhead::new(src, 1024 * 1024);
        let stats = ra.stats();
        assert!(
            eventually(|| stats.finished()),
            "a small body is fully fetched at once"
        );
        assert!(
            stats.queued_bytes() > 0,
            "...while it is still waiting to be played"
        );
        let mut all = Vec::new();
        ra.read_to_end(&mut all).unwrap();
        assert!(stats.finished());

        // A failure is not a clean finish.
        let ra = ReadAhead::new(
            Failing {
                good: Some(data(10)),
            },
            1024,
        );
        let stats = ra.stats();
        thread::sleep(Duration::from_millis(100));
        assert!(!stats.finished());
        drop(ra);
    }
}
