//! A counting global allocator, for the tests that have to prove a memory claim.
//!
//! `#[cfg(test)]`: none of this is compiled into the component, which is why it is the only
//! hand-written `unsafe` in the crate and why `scripts/validate.sh` exempts exactly this file.
//!
//! The counters are thread-local and opt-in, so `cargo test`'s parallel threads cannot pollute one
//! another's measurement and an unarmed thread pays only a `Cell` read per allocation.
//!
//! # Two peaks, because a growing buffer has two honest answers
//!
//! The interesting allocation in this crate's hot path is not one the component makes: it is
//! `serde_json`'s output buffer, which starts at 128 bytes and doubles. Writing an eleven-megabyte
//! string into it, then a few hundred bytes after that, ends with a 22 MiB buffer. Whether the 11 MiB
//! buffer it grew out of was live at the same moment depends entirely on the allocator:
//!
//! - [`Measurement::in_place`] counts a reallocation as the size *difference*, which is what happens
//!   when the allocator extends a block where it already is. dlmalloc — what a `wasm32-unknown-unknown`
//!   guest links — does exactly that for a chunk adjacent to the top of the heap, which the newest
//!   large allocation is.
//! - [`Measurement::copying`] counts the new block before releasing the old one, which is what
//!   happens when it cannot. This is the bound: no allocator does worse.
//!
//! Reporting one without the other would be picking a number. The component has to fit in a 64 MiB
//! store either way, so both are asserted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Whether this thread is inside [`measure`].
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// Live bytes, counting a reallocation as its size difference.
    static IN_PLACE_LIVE: Cell<usize> = const { Cell::new(0) };
    /// The high-water mark of [`IN_PLACE_LIVE`].
    static IN_PLACE_PEAK: Cell<usize> = const { Cell::new(0) };
    /// Live bytes, counting a reallocation's new block as live before the old one is released.
    static COPYING_LIVE: Cell<usize> = const { Cell::new(0) };
    /// The high-water mark of [`COPYING_LIVE`].
    static COPYING_PEAK: Cell<usize> = const { Cell::new(0) };
}

/// What one [`measure`] observed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Measurement {
    /// Peak live bytes when a reallocation extends a block in place.
    pub(crate) in_place: usize,
    /// Peak live bytes when a reallocation copies into a second block first.
    pub(crate) copying: usize,
}

impl Measurement {
    /// Renders both peaks in mebibytes, for a test's own output.
    pub(crate) fn describe(self) -> String {
        format!(
            "{:.1} MiB in place, {:.1} MiB copying",
            self.in_place as f64 / (1024.0 * 1024.0),
            self.copying as f64 / (1024.0 * 1024.0)
        )
    }
}

/// The measuring allocator.
struct Counting;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

// SAFETY: every method forwards to `System`, the platform allocator, with the same pointer and
// layout it was handed, and only updates thread-local counters around the call.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size().cast_signed(), layout.size().cast_signed());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record(-layout.size().cast_signed(), -layout.size().cast_signed());
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // The copying counter sees the new block arrive before the old one leaves; the in-place
        // counter sees only the difference. `System.realloc` itself is left to do whichever it can,
        // so the measurement never makes the test slower than the thing it measures.
        record(0, new_size.cast_signed());
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if moved.is_null() {
            record(0, -new_size.cast_signed());
        } else {
            record(
                new_size.cast_signed() - layout.size().cast_signed(),
                -layout.size().cast_signed(),
            );
        }
        moved
    }
}

/// Folds one allocator event into this thread's counters.
///
/// `try_with` rather than `with`: an allocation during thread-local teardown must not panic inside
/// the allocator, and an unarmed thread must not pay for anything beyond the read.
fn record(in_place: isize, copying: isize) {
    let _ = ARMED.try_with(|armed| {
        if !armed.get() {
            return;
        }
        accumulate(&IN_PLACE_LIVE, &IN_PLACE_PEAK, in_place);
        accumulate(&COPYING_LIVE, &COPYING_PEAK, copying);
    });
}

/// Applies `delta` to one live counter and lifts its peak.
fn accumulate(
    live: &'static std::thread::LocalKey<Cell<usize>>,
    peak: &'static std::thread::LocalKey<Cell<usize>>,
    delta: isize,
) {
    let _ = live.try_with(|live| {
        let current = live.get().saturating_add_signed(delta);
        live.set(current);
        let _ = peak.try_with(|peak| {
            if current > peak.get() {
                peak.set(current);
            }
        });
    });
}

/// Runs `body` with this thread's allocations counted, returning its value and both peaks.
pub(crate) fn measure<T>(body: impl FnOnce() -> T) -> (T, Measurement) {
    assert!(
        !ARMED.get(),
        "a measurement is already running on this thread"
    );
    for counter in [&IN_PLACE_LIVE, &IN_PLACE_PEAK, &COPYING_LIVE, &COPYING_PEAK] {
        counter.set(0);
    }
    ARMED.set(true);
    let value = body();
    ARMED.set(false);
    (
        value,
        Measurement {
            in_place: IN_PLACE_PEAK.get(),
            copying: COPYING_PEAK.get(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::measure;

    /// The counter tracks a high-water mark, releases it, and ignores what was allocated before it
    /// was armed.
    #[test]
    fn the_counter_tracks_a_peak_and_then_releases_it() {
        let before = vec![0_u8; 4 * 1024 * 1024];
        let (kept, measured) = measure(|| {
            let transient = vec![0_u8; 8 * 1024 * 1024];
            assert_eq!(transient.len(), 8 * 1024 * 1024);
            drop(transient);
            vec![0_u8; 1024 * 1024]
        });
        assert_eq!(kept.len(), 1024 * 1024);
        assert!(measured.in_place >= 8 * 1024 * 1024, "{measured:?}");
        assert!(
            measured.in_place < 12 * 1024 * 1024,
            "the pre-armed allocation was counted: {measured:?}"
        );
        assert_eq!(before.len(), 4 * 1024 * 1024);

        let (_, quiet) = measure(|| ());
        assert_eq!(quiet.in_place, 0);
        assert_eq!(quiet.copying, 0);
    }

    /// A doubling buffer is where the two accounting models diverge, and by exactly the size of the
    /// block it grew out of.
    #[test]
    fn a_growing_buffer_separates_the_two_accounting_models() {
        let (length, measured) = measure(|| {
            let mut buffer: Vec<u8> = Vec::with_capacity(1024 * 1024);
            buffer.resize(1024 * 1024, 1);
            buffer.extend_from_slice(&[2; 1024]);
            buffer.len()
        });
        assert_eq!(length, 1024 * 1024 + 1024);
        assert!(measured.copying > measured.in_place, "{measured:?}");
        assert!(measured.in_place >= 2 * 1024 * 1024, "{measured:?}");
    }
}
