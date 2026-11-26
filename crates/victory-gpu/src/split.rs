//! Big copies of title memory into host memory shared out over a few
//! threads of our own (`VICTORY_COPY_THREADS=N`; off by default).
//!
//! The submitting thread copies everything the title changed for a list
//! into host buffers by itself: ~300 MB a frame in the lit cell with ray
//! tracing on ("moves" 44 ms, profile of 90fea82), about 7 GB a second,
//! where the memory of the machine moves several times that with more than
//! one core at it (the processor has 16). A copy of at least
//! `VICTORY_COPY_SPLIT_KB` (default 1024) is cut in pieces of at least half
//! that, page-aligned; the workers take pieces from a queue, the asking
//! thread takes its own share and any piece not yet taken, then waits for
//! the rest. Each piece is the same copy as before (`crate::plaincopy`), so
//! the bytes are the same; a piece that meets a page that is not there
//! makes the whole copy answer false, and the caller copies through the
//! kernel, as before.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Worker threads (`VICTORY_COPY_THREADS`, at most 15; 0, the default: off).
pub(crate) fn threads() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let n = std::env::var("VICTORY_COPY_THREADS").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0).min(15);
        if n != 0 {
            tracing::info!(threads = n, least_kb = least() >> 10, "big copies of title memory are shared out over threads of our own (VICTORY_COPY_THREADS)");
        }
        n
    })
}

/// The fewest bytes a copy is shared out from (`VICTORY_COPY_SPLIT_KB`).
pub(crate) fn least() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VICTORY_COPY_SPLIT_KB").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1024).max(64) << 10
    })
}

/// One share of a split: `[at, at + len)` of the whole, for `work`.
struct Piece {
    at: usize,
    len: usize,
    work: Arc<Work>,
}

/// What a split's pieces share: the function (alive until `left` is 0, see
/// [`split`]), how many pieces are still to finish, and whether all went
/// well.
struct Work {
    run: *const (dyn Fn(usize, usize) -> bool + Sync),
    left: AtomicUsize,
    ok: AtomicBool,
}

// SAFETY: `run` points at a `Sync` function that the asking thread keeps
// alive until every piece is done ([`split`] waits, also when unwinding).
unsafe impl Send for Work {}
// SAFETY: as above.
unsafe impl Sync for Work {}

impl Work {
    fn piece(&self, at: usize, len: usize) {
        /// Counts the piece done however the call ends (a piece that
        /// unwound would leave the asking thread waiting for good).
        struct Done<'a>(&'a Work, bool);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    self.0.ok.store(false, Ordering::Relaxed);
                }
                self.0.left.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let mut done = Done(self, false);
        // SAFETY: alive until `left` reaches 0, which this piece's end does
        // only after the call.
        done.1 = unsafe { (*self.run)(at, len) };
    }
}

struct Pool {
    queue: Mutex<VecDeque<Piece>>,
    ready: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<&'static Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let pool: &'static Pool = Box::leak(Box::new(Pool { queue: Mutex::new(VecDeque::new()), ready: Condvar::new() }));
        for i in 0..threads() {
            let spawned = std::thread::Builder::new().name(format!("victory-copy-{i}")).spawn(move || loop {
                let piece = {
                    let mut queue = pool.queue.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(piece) = queue.pop_front() {
                            break piece;
                        }
                        queue = pool.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                    }
                };
                piece.work.piece(piece.at, piece.len);
            });
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "a copy thread could not be started");
            }
        }
        pool
    })
}

/// Copies split, and their bytes, since the last line.
static COUNT: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Runs `run(at, len)` over `[0, len)` in pieces shared out over the
/// workers and this thread; true if every piece returned true. Only when
/// [`threads`] is on and `len` is at least [`least`]; else one call.
pub(crate) fn split(len: usize, run: &(dyn Fn(usize, usize) -> bool + Sync)) -> bool {
    let workers = threads();
    if workers == 0 || len < least() {
        return run(0, len);
    }
    const ALIGN: usize = 4096;
    let pieces = (len / (least() / 2)).clamp(1, workers + 1);
    if pieces == 1 {
        return run(0, len);
    }
    let size = (len / pieces).next_multiple_of(ALIGN);
    let mut cuts: Vec<(usize, usize)> = Vec::with_capacity(pieces);
    let mut at = 0;
    while at < len {
        let n = size.min(len - at);
        cuts.push((at, n));
        at += n;
    }
    COUNT[0].fetch_add(1, Ordering::Relaxed);
    COUNT[1].fetch_add(len as u64, Ordering::Relaxed);
    // SAFETY: only the lifetime is changed; `Wait` below keeps `run` alive
    // until every piece is done.
    let run_static: *const (dyn Fn(usize, usize) -> bool + Sync) = unsafe { std::mem::transmute(run as *const (dyn Fn(usize, usize) -> bool + Sync + '_)) };
    let work = Arc::new(Work { run: run_static, left: AtomicUsize::new(cuts.len()), ok: AtomicBool::new(true) });
    /// Waits for every piece, also if this thread unwinds.
    struct Wait<'a>(&'a Work);
    impl Drop for Wait<'_> {
        fn drop(&mut self) {
            let mut spins = 0u32;
            while self.0.left.load(Ordering::Acquire) != 0 {
                spins += 1;
                if spins < 2_000 {
                    std::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                }
            }
        }
    }
    let wait = Wait(&work);
    let pool = pool();
    let (first, rest) = cuts.split_first().expect("two pieces or more");
    {
        let mut queue = pool.queue.lock().unwrap_or_else(|e| e.into_inner());
        for &(at, n) in rest {
            queue.push_back(Piece { at, len: n, work: work.clone() });
        }
    }
    pool.ready.notify_all();
    work.piece(first.0, first.1);
    // Pieces of ours no worker has taken yet: taken here.
    loop {
        let mine = {
            let mut queue = pool.queue.lock().unwrap_or_else(|e| e.into_inner());
            let at = queue.iter().position(|p| Arc::ptr_eq(&p.work, &work));
            at.and_then(|i| queue.remove(i))
        };
        match mine {
            Some(piece) => work.piece(piece.at, piece.len),
            None => break,
        }
    }
    drop(wait);
    work.ok.load(Ordering::Relaxed)
}

/// With the frame line: what [`split`] did per frame over `frames`.
pub(crate) fn line(frames: f64) {
    if threads() == 0 {
        return;
    }
    let [copies, bytes] = [0, 1].map(|i| COUNT[i].swap(0, Ordering::Relaxed) as f64 / frames);
    tracing::info!("big copies shared out over threads per frame (VICTORY_COPY_THREADS): {copies:.0}, {:.1} MB", bytes / 1e6);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_cover_the_whole_once() {
        // Without workers: one call over all of it.
        let seen = Mutex::new(Vec::new());
        assert!(split(10 << 20, &|at, n| {
            seen.lock().unwrap().push((at, n));
            true
        }));
        let mut seen = seen.into_inner().unwrap();
        seen.sort_unstable();
        let mut at = 0;
        for (a, n) in seen {
            assert_eq!(a, at);
            at += n;
        }
        assert_eq!(at, 10 << 20);
    }
}
