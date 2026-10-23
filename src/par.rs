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
    map_chunks_from(items, MIN_PARALLEL, work)
}

/// `map_chunks`, splitting from `min` items on: lower for work that waits
/// on the kernel, as a stat does, than for work that only computes.
pub fn map_chunks_from<T, R, F>(items: &[T], min: usize, work: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&[T]) -> Vec<R> + Sync,
{
    let threads = threads();
    if items.len() < min.max(2) || threads < 2 {
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

/// Work started on a thread of its own, to be collected later.
pub enum Pending<T> {
    Running(std::thread::JoinHandle<T>),
    /// No thread could be started, so the work was done in place.
    Done(T),
}

impl<T> Pending<T> {
    pub fn join(self) -> T {
        match self {
            Pending::Running(handle) => handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Pending::Done(value) => value,
        }
    }
}

/// Start `work` beside the caller, or run it now when no thread is to be had.
pub fn start<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Pending<T> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(work)));
    let theirs = slot.clone();
    let spawned = std::thread::Builder::new().spawn(move || {
        let work = theirs.lock().unwrap_or_else(|e| e.into_inner()).take().expect("work taken once");
        work()
    });
    match spawned {
        Ok(handle) => Pending::Running(handle),
        Err(_) => {
            let work = slot.lock().unwrap_or_else(|e| e.into_inner()).take().expect("work taken once");
            Pending::Done(work())
        }
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
