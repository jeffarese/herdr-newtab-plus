//! Counting the questions the picker asks the filesystem, for the tests
//! that hold a frame to a budget of them (src/tests/bench.rs).
//!
//! A frame that stats the filesystem is a frame whose cost is not its own: on
//! a network mount or a cold cache the lookup dominates everything else. What
//! is counted is the question, not the syscalls it takes to answer: resolving
//! a path costs one lstat per component, so counting syscalls would make the
//! number depend on how deep the home directory is. A question asked while
//! answering another is not counted again.
//!
//! Outside the tests both functions compile to nothing.

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static TOTAL: Cell<usize> = const { Cell::new(0) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// `answer()`, counted as one question.
#[inline(always)]
pub fn ask<R>(answer: impl FnOnce() -> R) -> R {
    ask_many(1, answer)
}

/// `answer()`, counted as `count` questions, however many threads it
/// spreads them over: what it asks on this one is not counted again.
#[inline(always)]
pub fn ask_many<R>(count: usize, answer: impl FnOnce() -> R) -> R {
    #[cfg(test)]
    {
        struct Leave;
        impl Drop for Leave {
            fn drop(&mut self) {
                DEPTH.with(|depth| depth.set(depth.get() - 1));
            }
        }
        DEPTH.with(|depth| {
            if depth.get() == 0 {
                TOTAL.with(|total| total.set(total.get() + count));
            }
            depth.set(depth.get() + 1);
        });
        let _leave = Leave;
        answer()
    }
    #[cfg(not(test))]
    {
        let _ = count;
        answer()
    }
}

/// What `work` returns, and how many questions it asked on this thread.
#[cfg(test)]
pub fn counted<R>(work: impl FnOnce() -> R) -> (R, usize) {
    let before = TOTAL.with(Cell::get);
    let result = work();
    (result, TOTAL.with(Cell::get) - before)
}
