//! Splitting pure, per-row work across the machine's cores.
//!
//! Building and scoring rows is the same small job thousands of times over,
//! with no state shared between rows. A big session splits the rows into a
//! few contiguous chunks, works them on scoped threads and joins the results
//! back in order, so the answer is exactly the one a single pass gives. A
//! small session never pays for a thread.

use std::sync::OnceLock;

/// Below this many rows a thread costs more than it saves.
pub const MIN_PARALLEL: usize = 512;
/// Past four the chunks get too small to matter, and the bar shares the
/// machine with whatever the user is running.
const MAX_THREADS: usize = 4;

fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, MAX_THREADS))
}

/// `work(items)`, computed chunk by chunk, possibly in parallel, and
/// concatenated in the original order.
pub fn map_chunks<T, R, F>(items: &[T], work: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&[T]) -> Vec<R> + Sync,
{
    let threads = threads();
    if items.len() < MIN_PARALLEL || threads < 2 {
        return work(items);
    }
    let size = items.len().div_ceil(threads);
    let mut chunks = items.chunks(size);
    let first = chunks.next().unwrap_or(&[]);
    std::thread::scope(|scope| {
        let work = &work;
        let handles: Vec<_> = chunks.map(|chunk| scope.spawn(move || work(chunk))).collect();
        let mut out = work(first);
        for handle in handles {
            out.extend(handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)));
        }
        out
    })
}

/// Free a big structure on a short-lived thread instead of the caller's.
///
/// A snapshot or pool of thousands of rows takes a millisecond or more to
/// free, allocation by allocation, and the caller is usually about to draw
/// or read a key. Freeing has no effect anyone can observe, so it can just
/// as well happen beside them. Small values are freed in place.
pub fn drop_aside<T: Send + 'static>(value: T, big: bool) {
    if big {
        // No thread to be had: the value goes with the failed closure, here.
        let _ = std::thread::Builder::new().spawn(move || drop(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order() {
        let items: Vec<usize> = (0..5000).collect();
        let doubled = map_chunks(&items, |chunk| chunk.iter().map(|i| i * 2).collect());
        assert_eq!(doubled, items.iter().map(|i| i * 2).collect::<Vec<_>>());
        let few: Vec<usize> = (0..10).collect();
        assert_eq!(map_chunks(&few, |chunk| chunk.to_vec()), few);
    }
}
