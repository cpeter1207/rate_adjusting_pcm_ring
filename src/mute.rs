//! Bounded SPSC publication of source mute intervals, never edits shared PCM slots.
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
};

/// Maximum intervals admitted by one producer operation.
pub(crate) const MAX_SPANS: usize = 64;
/// Two maximum batches keep publication bounded without waiting on audio.
const QUEUE: usize = MAX_SPANS * 2;

/// Half-open absolute input-rate source interval, independent of storage cursors.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

/// One source sample's immutable generation and absolute position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) generation: u64,
    pub(crate) source: u64,
}

/// Immutable mailbox/history record including its source-generation fence.
#[derive(Clone, Copy)]
pub(crate) struct Interval {
    pub(crate) generation: u64,
    pub(crate) span: Span,
}

/// One output sample's conservative source support, retained through converter/PLC caches.
#[derive(Clone, Copy)]
pub(crate) struct Position {
    pub(crate) generation: u64,
    pub(crate) low: u64,
    pub(crate) high: u64,
}

/// One producer publishes complete immutable records; one consumer copies before retiring.
pub(crate) struct Mailbox {
    slots: Box<[UnsafeCell<Interval>]>,
    written: AtomicU64,
    read: AtomicU64,
}
// Payload access follows the same Release/Acquire SPSC ownership as the PCM storage.
unsafe impl Send for Mailbox {}
unsafe impl Sync for Mailbox {}
impl Mailbox {
    /// Reserve every immutable record slot before audio begins.
    pub(crate) fn new() -> Result<Self, ()> {
        let mut slots = Vec::new();
        slots.try_reserve_exact(QUEUE).map_err(|_| ())?;
        slots.resize_with(QUEUE, || {
            UnsafeCell::new(Interval {
                generation: 0,
                span: Span { start: 0, end: 0 },
            })
        });
        Ok(Self {
            slots: slots.into_boxed_slice(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
        })
    }
    /// Sole producer: reserve before publishing PCM; no overwrite or wait when full.
    pub(crate) fn free(&self) -> usize {
        let written = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        QUEUE - ((written + (QUEUE * 2) as u64 - read) % (QUEUE * 2) as u64) as usize
    }
    /// Caller owns enough credit. One final release publishes the entire bounded batch.
    pub(crate) fn publish(&self, generation: u64, spans: &[Span], end: u64) {
        let mut written = self.written.load(Ordering::Relaxed);
        for span in spans {
            let clipped = Span {
                start: span.start,
                end: span.end.min(end),
            };
            if clipped.start < clipped.end {
                unsafe {
                    *self.slots[written as usize % QUEUE].get() = Interval {
                        generation,
                        span: clipped,
                    };
                }
                written = (written + 1) % (QUEUE * 2) as u64;
            }
        }
        self.written.store(written, Ordering::Release);
    }
    /// Sole consumer copies one published record before releasing its slot.
    pub(crate) fn pop(&self) -> Option<Interval> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.written.load(Ordering::Acquire) {
            return None;
        }
        let interval = unsafe { *self.slots[read as usize % QUEUE].get() };
        self.read
            .store((read + 1) % (QUEUE * 2) as u64, Ordering::Release);
        Some(interval)
    }
    /// A reset discards only records published before its acquired snapshot.
    pub(crate) fn reset(&self) {
        self.read
            .store(self.written.load(Ordering::Acquire), Ordering::Release);
    }
}

/// Consumer-owned union; never grows on audio. Typical DTMF spans coalesce to one entry.
pub(crate) struct History(VecDeque<Interval>);
impl History {
    /// Reserve the bounded live-source interval budget on control.
    pub(crate) fn new(capacity: usize) -> Result<Self, ()> {
        let mut entries = VecDeque::new();
        entries.try_reserve_exact(capacity).map_err(|_| ())?;
        Ok(Self(entries))
    }
    /// Discard interval state while retaining its prepared allocation.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
    /// Merge a late/overlapping interval into the sorted union without allocation.
    pub(crate) fn insert(&mut self, mut interval: Interval) -> Result<(), ()> {
        let mut index = 0;
        while index < self.0.len() {
            let other = self.0[index];
            if other.generation == interval.generation
                && other.span.end >= interval.span.start
                && other.span.start <= interval.span.end
            {
                interval.span.start = interval.span.start.min(other.span.start);
                interval.span.end = interval.span.end.max(other.span.end);
                self.0.remove(index);
            } else {
                index += 1;
            }
        }
        if self.0.len() == self.0.capacity() {
            return Err(());
        }
        let index = self.0.partition_point(|other| {
            (other.generation, other.span.start) < (interval.generation, interval.span.start)
        });
        self.0.insert(index, interval);
        Ok(())
    }
    /// Logarithmic lookup at actual emitted source support, not FIFO occupancy.
    pub(crate) fn muted(&self, position: Option<Position>) -> bool {
        position.is_some_and(|position| {
            let index = self.0.partition_point(|interval| {
                (interval.generation, interval.span.start) <= (position.generation, position.high)
            });
            index.checked_sub(1).is_some_and(|index| {
                let interval = self.0[index];
                interval.generation == position.generation && interval.span.end > position.low
            })
        })
    }
    /// Retire only positions actually emitted after optional PLC lookahead, never storage reads.
    pub(crate) fn retire(&mut self, position: Option<Position>) {
        if let Some(position) = position {
            while self.0.front().is_some_and(|interval| {
                interval.generation != position.generation || interval.span.end <= position.low
            }) {
                self.0.pop_front();
            }
        }
    }
}
