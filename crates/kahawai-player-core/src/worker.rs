//! Runs a blocking producer on its own thread, so the playback thread never
//! blocks on it.
//!
//! Decoding reads the network. A read can block for as long as the network is
//! down, and a decoder asks for whole packets, so it can block even while some
//! bytes are buffered. If that happened on the playback thread, every command
//! (pause, seek, stop, even quitting) would wait behind it. [`ChunkWorker`]
//! moves the producer (a decoder, a DoP reader) onto a worker thread that
//! feeds a small bounded queue; the playback thread only ever *polls* it, with
//! a short wait, and stays free to serve commands whatever the network does.
//!
//! - **Bounded.** The worker runs at most `depth` chunks ahead (backpressure),
//!   so it never decodes a whole track into memory.
//! - **Cancels on drop.** Dropping the worker (seek, track change) stops it as
//!   soon as it reaches its next chunk. If it is stuck inside a read it stays
//!   stuck until that read returns, but nothing waits for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kahawai_core::MusicError;

enum Msg<C> {
    Chunk(C),
    End,
    Failed(MusicError),
}

/// What a poll found.
#[derive(Debug)]
pub enum Polled<C> {
    /// The next chunk.
    Ready(C),
    /// Nothing yet: the producer is still working (or waiting on the network).
    Empty,
    /// The producer finished cleanly; there is nothing more.
    Ended,
    /// The producer failed. Reported once; afterwards the worker reads as ended.
    Failed(MusicError),
}

pub struct ChunkWorker<C> {
    rx: Receiver<Msg<C>>,
    cancel: Arc<AtomicBool>,
    /// An end or failure has been delivered.
    done: bool,
    /// Never joined: the thread may be stuck in a network read, and dropping
    /// the worker must not wait for it.
    _thread: JoinHandle<()>,
}

impl<C: Send + 'static> ChunkWorker<C> {
    /// Start producing. `next` returns the next chunk, `Ok(None)` at the end
    /// of the stream, or an error (which ends the worker).
    pub fn spawn<F>(depth: usize, mut next: F) -> Self
    where
        F: FnMut() -> Result<Option<C>, MusicError> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<Msg<C>>(depth.max(1));
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = thread::Builder::new()
            .name("stream-decode".into())
            .spawn(move || loop {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let msg = match next() {
                    Ok(Some(chunk)) => Msg::Chunk(chunk),
                    Ok(None) => Msg::End,
                    Err(e) => Msg::Failed(e),
                };
                let last = !matches!(msg, Msg::Chunk(_));
                // A closed channel means the consumer is gone: stop.
                if tx.send(msg).is_err() || last {
                    return;
                }
            })
            .expect("spawn decode worker");
        Self {
            rx,
            cancel,
            done: false,
            _thread: thread,
        }
    }

    /// The next chunk, waiting at most `wait` for it.
    pub fn poll(&mut self, wait: Duration) -> Polled<C> {
        if self.done {
            return Polled::Ended;
        }
        match self.rx.recv_timeout(wait) {
            Ok(Msg::Chunk(c)) => Polled::Ready(c),
            Ok(Msg::End) => {
                self.done = true;
                Polled::Ended
            }
            Ok(Msg::Failed(e)) => {
                self.done = true;
                Polled::Failed(e)
            }
            Err(RecvTimeoutError::Timeout) => Polled::Empty,
            Err(RecvTimeoutError::Disconnected) => {
                // The thread vanished without a word (it panicked).
                self.done = true;
                Polled::Failed(MusicError::Metadata(
                    "the decoder stopped unexpectedly".into(),
                ))
            }
        }
    }
}

impl<C> Drop for ChunkWorker<C> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    const WAIT: Duration = Duration::from_millis(500);

    fn eventually(mut f: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    /// Chunks 0..n, then the end.
    fn counting(n: usize) -> impl FnMut() -> Result<Option<usize>, MusicError> + Send {
        let mut i = 0;
        move || {
            if i == n {
                return Ok(None);
            }
            i += 1;
            Ok(Some(i - 1))
        }
    }

    #[test]
    fn delivers_every_chunk_in_order_then_ends() {
        let mut w = ChunkWorker::spawn(2, counting(50));
        for want in 0..50 {
            match w.poll(WAIT) {
                Polled::Ready(got) => assert_eq!(got, want),
                other => panic!("chunk {want}: {other:?}"),
            }
        }
        assert!(matches!(w.poll(WAIT), Polled::Ended));
        assert!(matches!(w.poll(WAIT), Polled::Ended), "and stays ended");
    }

    #[test]
    fn runs_only_a_bounded_distance_ahead_of_the_consumer() {
        let made = Arc::new(AtomicUsize::new(0));
        let m = made.clone();
        let mut w = ChunkWorker::spawn(3, move || {
            m.fetch_add(1, Ordering::SeqCst);
            Ok(Some(0u8))
        });
        assert!(eventually(|| made.load(Ordering::SeqCst) >= 3));
        thread::sleep(Duration::from_millis(100));
        let ahead = made.load(Ordering::SeqCst);
        assert!(
            ahead <= 3 + 2,
            "stopped near the depth, not running away: {ahead}"
        );
        assert!(matches!(w.poll(WAIT), Polled::Ready(_)));
        assert!(
            eventually(|| made.load(Ordering::SeqCst) > ahead),
            "taking one lets it make another"
        );
    }

    #[test]
    fn an_error_arrives_after_the_chunks_before_it_and_only_once() {
        let mut i = 0;
        let mut w = ChunkWorker::spawn(4, move || {
            i += 1;
            if i <= 2 {
                Ok(Some(i))
            } else {
                Err(MusicError::Metadata("boom".into()))
            }
        });
        assert!(matches!(w.poll(WAIT), Polled::Ready(1)));
        assert!(matches!(w.poll(WAIT), Polled::Ready(2)));
        assert!(matches!(w.poll(WAIT), Polled::Failed(_)));
        assert!(
            matches!(w.poll(WAIT), Polled::Ended),
            "then it reads as ended"
        );
    }

    #[test]
    fn a_stuck_producer_never_blocks_the_consumer() {
        // The point of the module: the producer is blocked (a dead network)
        // and the consumer still gets its turn back after the short wait.
        let release = Arc::new(AtomicBool::new(false));
        let r = release.clone();
        let mut sent = false;
        let mut w = ChunkWorker::spawn(2, move || {
            if !sent {
                sent = true;
                return Ok(Some(1));
            }
            while !r.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(None)
        });
        assert!(matches!(w.poll(WAIT), Polled::Ready(1)));
        for _ in 0..5 {
            let started = Instant::now();
            assert!(matches!(w.poll(Duration::from_millis(20)), Polled::Empty));
            assert!(
                started.elapsed() < Duration::from_millis(400),
                "returned promptly"
            );
        }
        release.store(true, Ordering::SeqCst);
        assert!(
            eventually(|| matches!(w.poll(Duration::from_millis(20)), Polled::Ended)),
            "and recovers"
        );
    }

    /// Drops its flag when dropped, to show the producer was released.
    struct Flag(Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn dropping_the_worker_stops_a_producer_waiting_for_room() {
        let released = Arc::new(AtomicBool::new(false));
        let guard = Flag(released.clone());
        let w = ChunkWorker::spawn(1, move || {
            let _keep = &guard;
            Ok(Some(0u8))
        });
        thread::sleep(Duration::from_millis(50)); // queue full, producer blocked sending
        drop(w);
        assert!(
            eventually(|| released.load(Ordering::SeqCst)),
            "the producer (and what it owns) was released"
        );
    }

    #[test]
    fn dropping_does_not_wait_for_a_producer_stuck_in_a_read() {
        let release = Arc::new(AtomicBool::new(false));
        let r = release.clone();
        let w: ChunkWorker<u8> = ChunkWorker::spawn(1, move || {
            while !r.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(None)
        });
        let started = Instant::now();
        drop(w);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "drop returned at once"
        );
        release.store(true, Ordering::SeqCst);
    }
}
