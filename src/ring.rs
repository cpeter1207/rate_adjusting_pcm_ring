//! Lock-free SPSC PCM storage, rate recovery, and bounded concealment.
//!
//! The producer and consumer share only bounded atomic cursors and
//! observable counters.  Conversion, occupancy control, and concealment are
//! owned exclusively by the consumer, so neither audio operation allocates,
//! locks, logs, or performs I/O after construction.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::collections::VecDeque;

use crate::mute::{self, History, Mailbox, Position, Span, Stamp};
use crate::plc::Plc;
use crate::samplerate_adapter::{AdapterFunctions, Converter};

/// Maximum input/output chunk handed to the persistent converter per refill.
const CONVERTER_QUANTUM: usize = 256;
/// Smallest workspace that can grow once after a zero-progress SRC call.
pub(crate) const MINIMUM_CAPACITY: usize = CONVERTER_QUANTUM * 2;
/// Largest source workspace accepted by the adapter's `uint32_t` ABI.
pub(crate) const MAXIMUM_CAPACITY: usize = u32::MAX as usize;
/// Dynamic adapter's documented lower bound for a continuing conversion ratio.
const MINIMUM_RATIO: f64 = 1.0 / 256.0;
/// Dynamic adapter's documented upper bound for a continuing conversion ratio.
const MAXIMUM_RATIO: f64 = 256.0;

/// Concealment selected once when creating the consumer's workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlcMode {
    /// Missing output is silence without a lookahead delay or PLC workspace.
    Disabled,
    /// Sample-domain G.711 Appendix I at the output sample rate.
    G711AppendixI,
}

impl PlcMode {
    /// Reject unpublished selectors at the C boundary.
    pub(crate) fn from_ffi(value: u32) -> Result<Self, ()> {
        match value {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::G711AppendixI),
            _ => Err(()),
        }
    }
}

/// Immutable input-domain buffer policy and maximum public block sizes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings {
    pub(crate) capacity: u64,
    pub(crate) input_rate_hz: u32,
    pub(crate) output_rate_hz: u32,
    pub(crate) reserve: u64,
    pub(crate) target: u64,
    /// Largest producer block, in input samples.
    pub(crate) max_producer: u64,
    /// Largest consumer block, in output samples.
    pub(crate) max_output: u64,
    pub(crate) plc: PlcMode,
}

impl Settings {
    /// Validate source headroom for a worst-ratio callback and producer burst.
    pub(crate) fn validate(&self) -> Result<(), CreateError> {
        let invalid = CreateError::Invalid;
        let ratio = f64::from(self.output_rate_hz) / f64::from(self.input_rate_hz);
        if !(MINIMUM_CAPACITY as u64..=MAXIMUM_CAPACITY as u64).contains(&self.capacity)
            || self.input_rate_hz == 0
            || self.output_rate_hz == 0
            || !(MINIMUM_RATIO..=MAXIMUM_RATIO).contains(&ratio)
            || self.reserve > self.capacity
            || self.target > self.capacity
            || self.max_producer == 0
            || self.max_output == 0
            || self.max_producer > isize::MAX as u64 / size_of::<f32>() as u64
            || self.max_output > isize::MAX as u64 / size_of::<f32>() as u64
        {
            return Err(invalid);
        }
        if self.plc == PlcMode::G711AppendixI {
            // Use integer rational arithmetic so an exact boundary cannot be
            // rounded down by floating-point source/output rate conversion.
            let numerator = u128::from(self.output_rate_hz) * 999;
            let denominator = u128::from(self.input_rate_hz) * 1000;
            let budget = if numerator * 256 < denominator {
                self.max_output as u128 * 256
            } else {
                (self.max_output as u128 * denominator).div_ceil(numerator)
            } + 1;
            // Widen before arithmetic; sums/products of validated u64/u32
            // counts fit u128 even at the public interface's largest values.
            let space = u128::from(self.target) + u128::from(self.max_producer);
            if u128::from(self.reserve) < budget
                || self.target < self.reserve
                || space > u128::from(self.capacity)
            {
                return Err(invalid);
            }
        }
        Ok(())
    }
}

/// Lock-free counters exposed as a best-effort snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observation {
    /// Immutable sample capacity.
    pub(crate) capacity_samples: u64,
    /// Source samples currently readable by the consumer.
    pub(crate) available_samples: u64,
    /// Immutable source-sample playout priming floor.
    pub(crate) reserve_samples: u64,
    /// Low-pass filtered source occupancy.
    pub(crate) filtered_occupancy_samples: u64,
    /// Immutable source-sample target used by the slow controller.
    pub(crate) target_samples: u64,
    /// Applied conversion-ratio correction relative to nominal rate.
    pub(crate) ratio_correction_ppm: i32,
    /// Input samples rejected because unread storage was full.
    pub(crate) discarded_samples: u64,
    /// Mapped gaps and converter shortfalls, excluding priming and PLC lookahead.
    pub(crate) missing_samples: u64,
    /// Current consecutive missing-output run, including mapped source gaps.
    pub(crate) consecutive_shortfall_samples: u64,
    /// Ten-second moving average of consecutive shortfall in millisamples.
    pub(crate) shortfall_average_milli: u64,
    /// Consumer SRC calls rejected after successful construction.
    pub(crate) adapter_error_count: u64,
}

/// Control-plane creation failure classified for the stable C ABI.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CreateError {
    /// Buffer geometry, rates or callback bounds are invalid.
    Invalid,
    /// Preallocated Rust storage could not be reserved.
    NoMemory,
    /// The required dynamic converter adapter could not initialize.
    Adapter,
}

/// Consumer refill outcome, preserving adapter faults separately from loss.
enum RefillResult {
    /// Converted PCM is ready in the consumer cache.
    Ready,
    /// No usable converted PCM is currently available.
    Starved,
    /// The mandatory sample-rate adapter rejected real-time conversion.
    AdapterError,
}

/// One source slot; missing PCM is zero only inside the persistent converter.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SourceSample {
    pcm: f32,
    missing: bool,
    stamp: Option<Stamp>,
}

/// Half-open missing interval on the consumer's accepted source timeline.
struct GapSpan {
    start: u128,
    end: u128,
}

/// Persistent state touched exclusively by the consumer endpoint.
struct ConsumerState {
    converter: Converter,
    input: Box<[f32]>,
    output: Box<[f32]>,
    input_offset: usize,
    input_pending: usize,
    output_offset: usize,
    output_pending: usize,
    output_missing: bool,
    source_taken: u128,
    source_accepted: u128,
    gaps: VecDeque<GapSpan>,
    plc: Option<Plc>,
    zero_progress: bool,
    occupancy_milli: u64,
    ratio: f64,
    primed: bool,
    /// A failed adapter reset blocks conversion until a later reset succeeds.
    reset_failed: bool,
    /// Generation/source offset captured from actual source slots, never occupancy.
    source_base: Option<(u64, i128)>,
    cache_start: i128,
    cache_end: i128,
    cache_count: usize,
    mute_history: History,
    mute_fault: bool,
    plc_positions: Box<[Option<Position>]>,
    plc_position: usize,
}

/// All preallocated payload and consumer workspaces owned by one ring.
struct Workspaces {
    storage: Box<[UnsafeCell<SourceSample>]>,
    input: Box<[f32]>,
    output: Box<[f32]>,
}

/// Lock-free producer/consumer state and consumer-only DSP workspace.
///
/// `UnsafeCell` stores the payload because one producer writes a slot before
/// its release-store advances `written`, while one consumer reads that slot
/// only after acquiring `written`.  The SPSC contract is part of the public
/// ABI.  Every other shared field is an atomic diagnostic or cursor.
/// Cursors wrap at twice capacity so their distance distinguishes full from
/// empty and their storage indices stay aligned across every wrap. Release
/// publication and acquire consumption preserve the SPSC payload ordering.
pub(crate) struct Ring {
    storage: Box<[UnsafeCell<SourceSample>]>,
    capacity: usize,
    capacity_u64: u64,
    written: AtomicU64,
    read: AtomicU64,
    discarded: AtomicU64,
    missing: AtomicU64,
    consecutive_shortfall: AtomicU64,
    /// Internal precision prevents a one-sample EMA update rounding to zero.
    shortfall_average_micro: AtomicU64,
    reserve_samples: u64,
    target_samples: u64,
    filtered_occupancy_milli: AtomicU64,
    ratio_correction_ppm: AtomicI32,
    adapter_error_count: AtomicU64,
    input_rate_hz: u32,
    output_rate_hz: u32,
    output_delay_samples: u64,
    max_producer: usize,
    max_output: usize,
    consumer: UnsafeCell<ConsumerState>,
    /// Only the one producer touches its optional source stamp.
    producer_stamp: UnsafeCell<Option<Stamp>>,
    mute_mailbox: Mailbox,
    reset_serial: AtomicU64,
    producer_epoch: UnsafeCell<u64>,
}

// The only non-atomic sharing is protected by the documented one-producer,
// one-consumer cursor protocol described above.  The sole consumer owns the
// `ConsumerState` behind its UnsafeCell for the lifetime of a running ring.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Allocate a ring and its converter before either real-time endpoint runs.
    pub(crate) fn create(
        settings: Settings,
        adapter: AdapterFunctions,
    ) -> Result<Self, CreateError> {
        settings.validate()?;
        let Settings {
            capacity,
            input_rate_hz,
            output_rate_hz,
            ..
        } = settings;
        let capacity = capacity as usize;
        let plc = match settings.plc {
            PlcMode::Disabled => None,
            PlcMode::G711AppendixI => {
                Some(Plc::new(output_rate_hz).map_err(|()| CreateError::NoMemory)?)
            }
        };
        let Workspaces {
            storage,
            input,
            output,
        } = match allocate_workspaces(capacity) {
            Ok(workspaces) => workspaces,
            Err(()) => return Err(CreateError::NoMemory),
        };
        let mut converter = match adapter.create_converter(
            input_rate_hz,
            output_rate_hz,
            capacity as u32,
            CONVERTER_QUANTUM as u32,
        ) {
            Ok(converter) => converter,
            Err(()) => return Err(CreateError::Adapter),
        };
        // At most capacity input-workspace slots plus capacity adapter backlog
        // and FIR history can retain gap markers. Even alternating one-frame
        // spans fit this conservative bound without runtime growth.
        let delay = converter.input_delay().map_err(|()| CreateError::Adapter)? as u64;
        let history = delay.div_ceil(1_000_000_000) as usize;
        let mut gaps = VecDeque::new();
        gaps.try_reserve_exact(capacity * 2 + history + 2)
            .map_err(|_| CreateError::NoMemory)?;
        let mute_history =
            History::new(capacity * 2 + history + 2).map_err(|_| CreateError::NoMemory)?;
        let mute_mailbox = Mailbox::new().map_err(|_| CreateError::NoMemory)?;
        let delay = if settings.plc == PlcMode::G711AppendixI {
            (u64::from(output_rate_hz) * 15).div_ceil(4000) as usize
        } else {
            0
        };
        let mut plc_positions = Vec::new();
        plc_positions
            .try_reserve_exact(delay)
            .map_err(|_| CreateError::NoMemory)?;
        plc_positions.resize(delay, None);
        Ok(Self {
            storage,
            capacity,
            capacity_u64: capacity as u64,
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            discarded: AtomicU64::new(0),
            missing: AtomicU64::new(0),
            consecutive_shortfall: AtomicU64::new(0),
            shortfall_average_micro: AtomicU64::new(0),
            reserve_samples: settings.reserve,
            target_samples: settings.target,
            filtered_occupancy_milli: AtomicU64::new(0),
            ratio_correction_ppm: AtomicI32::new(0),
            adapter_error_count: AtomicU64::new(0),
            input_rate_hz,
            output_rate_hz,
            output_delay_samples: u64::from(converter.output_delay_samples),
            max_producer: settings.max_producer as usize,
            max_output: settings.max_output as usize,
            producer_stamp: UnsafeCell::new(None),
            mute_mailbox,
            reset_serial: AtomicU64::new(0),
            producer_epoch: UnsafeCell::new(0),
            consumer: UnsafeCell::new(ConsumerState {
                converter,
                input,
                output,
                input_offset: 0,
                input_pending: 0,
                output_offset: 0,
                output_pending: 0,
                output_missing: false,
                source_taken: 0,
                source_accepted: 0,
                gaps,
                plc,
                zero_progress: false,
                occupancy_milli: 0,
                ratio: 0.0,
                primed: false,
                reset_failed: false,
                source_base: None,
                cache_start: 0,
                cache_end: 0,
                cache_count: 0,
                mute_history,
                mute_fault: false,
                plc_positions: plc_positions.into_boxed_slice(),
                plc_position: 0,
            }),
        })
    }

    /// Publish one canonical input sample without waiting or allocating.
    pub(crate) fn producer_push_sample(&self, sample: f32) -> bool {
        self.producer_publish(SourceSample {
            pcm: canonicalize(sample),
            missing: false,
            stamp: None,
        })
    }

    /// Publish one source-timeline slot through the SPSC cursor protocol.
    fn producer_publish(&self, mut sample: SourceSample) -> bool {
        self.sync_producer_epoch();
        let written = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if self.cursor_distance(written, read) >= self.capacity_u64 {
            saturating_add_counter(&self.discarded, 1);
            return false;
        }
        let index = (written % self.capacity_u64) as usize;
        // Producer-only state supplies a monotonic source identity, even when
        // PCM/gap writes use the older compatible entrypoints between tagged blocks.
        let stamp = unsafe { &mut *self.producer_stamp.get() };
        if let Some(current) = stamp {
            let Some(next) = current.source.checked_add(1) else {
                return false;
            };
            sample.stamp = Some(*current);
            current.source = next;
        }
        // The producer owns this slot until the release store below publishes
        // it.  The sole consumer acquires `written` before reading it.
        unsafe {
            *self.storage[index].get() = sample;
        }
        self.written
            .store((written + 1) % (self.capacity_u64 * 2), Ordering::Release);
        true
    }

    /// Publish a bounded block and return the number accepted chronologically.
    pub(crate) fn producer_push(&self, input: &[f32]) -> u64 {
        let mut accepted = 0_u64;
        for (index, &sample) in input.iter().enumerate() {
            if self.producer_push_sample(sample) {
                accepted = accepted.saturating_add(1);
            } else {
                // Do not accept a later sample after dropping an earlier one:
                // callers can retry only the rejected chronological suffix.
                let rejected_tail = input.len().saturating_sub(index + 1) as u64;
                saturating_add_counter(&self.discarded, rejected_tail);
                break;
            }
        }
        accepted
    }

    /// Publish mute intervals before their PCM prefix. Retry only unaccepted source suffixes.
    /// A new generation requires the caller's completed consumer reset/quiescence boundary.
    pub(crate) fn producer_push_tagged(
        &self,
        input: &[f32],
        generation: u64,
        source_start: u64,
        spans: &[Span],
    ) -> Result<u64, ()> {
        self.sync_producer_epoch();
        let end = source_start.checked_add(input.len() as u64).ok_or(())?;
        if input.len() > self.max_producer
            || spans.len() > mute::MAX_SPANS
            || spans.iter().any(|span| {
                span.start >= span.end
                    || span.end > end
                    || span.start < source_start.saturating_sub(self.capacity_u64)
            })
        {
            return Err(());
        }
        let previous = unsafe { *self.producer_stamp.get() };
        if previous
            .is_some_and(|stamp| stamp.generation == generation && stamp.source != source_start)
        {
            return Err(());
        }
        if self.mute_mailbox.free() < spans.len() {
            return Ok(0);
        }
        let count = input
            .len()
            .min((self.capacity_u64 - self.available()) as usize);
        if count == 0 {
            return Ok(0);
        }
        self.mute_mailbox
            .publish(generation, spans, source_start + count as u64);
        unsafe {
            *self.producer_stamp.get() = Some(Stamp {
                generation,
                source: source_start,
            });
        }
        let accepted = self.producer_push(&input[..count]);
        saturating_add_counter(&self.discarded, input.len() as u64 - accepted);
        Ok(accepted)
    }

    /// Update only already accepted source positions. Returns interval-prefix credit;
    /// zero is bounded metadata backpressure, never permission to drop the update.
    pub(crate) fn producer_mute(&self, generation: u64, spans: &[Span]) -> Result<usize, ()> {
        self.sync_producer_epoch();
        let current = unsafe { *self.producer_stamp.get() }.ok_or(())?;
        if current.generation != generation
            || spans.len() > mute::MAX_SPANS
            || spans.iter().any(|span| {
                span.start >= span.end
                    || span.end > current.source
                    || span.start < current.source.saturating_sub(self.capacity_u64)
            })
        {
            return Err(());
        }
        let count = spans.len().min(self.mute_mailbox.free());
        self.mute_mailbox
            .publish(generation, &spans[..count], current.source);
        Ok(count)
    }

    /// Consumer reset publishes only an epoch; it never writes producer-owned state.
    fn sync_producer_epoch(&self) {
        let reset = self.reset_serial.load(Ordering::Acquire);
        unsafe {
            if *self.producer_epoch.get() != reset {
                *self.producer_stamp.get() = None;
                *self.producer_epoch.get() = reset;
            }
        }
    }

    /// Publish an ordered missing-source prefix without allocating or waiting.
    pub(crate) fn producer_gap(&self, samples: u64) -> u64 {
        for accepted in 0..samples {
            if !self.producer_publish(SourceSample {
                pcm: 0.0,
                missing: true,
                stamp: None,
            }) {
                saturating_add_counter(&self.discarded, samples - accepted - 1);
                return accepted;
            }
        }
        samples
    }

    /// Consume one source sample for the renderer's persistent converter.
    fn consumer_take_source_sample(&self) -> Option<SourceSample> {
        let read = self.read.load(Ordering::Relaxed);
        let written = self.written.load(Ordering::Acquire);
        if read == written {
            return None;
        }
        let index = (read % self.capacity_u64) as usize;
        // The producer's release publication above makes this slot readable;
        // the consumer alone advances `read`, so no second consumer can race.
        let sample = unsafe { *self.storage[index].get() };
        self.read
            .store((read + 1) % (self.capacity_u64 * 2), Ordering::Release);
        Some(sample)
    }

    /// Maximum accepted producer call size, in input samples.
    pub(crate) fn max_producer_samples(&self) -> usize {
        self.max_producer
    }

    /// Maximum accepted consumer call size, in output samples.
    pub(crate) fn max_output_samples(&self) -> usize {
        self.max_output
    }

    /// Immutable FIR delay in output samples, excluding optional PLC lookahead.
    pub(crate) fn output_delay_samples(&self) -> u64 {
        self.output_delay_samples
    }

    /// Render one output sample using the same priming policy as block playout.
    pub(crate) fn consumer_render_sample(&self) -> (f32, bool, bool) {
        // Only the single consumer accesses the converter, lookahead and PLC.
        let state = unsafe { &mut *self.consumer.get() };
        // Bound this drain even if the producer keeps publishing concurrently.
        for _ in 0..mute::MAX_SPANS * 2 {
            let Some(interval) = self.mute_mailbox.pop() else {
                break;
            };
            if state.mute_history.insert(interval).is_err() {
                state.mute_fault = true;
            }
        }
        if state.mute_fault {
            saturating_add_counter(&self.adapter_error_count, 1);
            return (0.0, false, true);
        }
        if !state.primed && !state.reset_failed {
            if self.available() < self.reserve_samples {
                return (0.0, false, false);
            }
            state.primed = true;
        }
        let refill = state.refill_output(self, self.target_samples);
        if state.mute_fault {
            saturating_add_counter(&self.adapter_error_count, 1);
            return (0.0, false, true);
        }
        let failed = matches!(refill, RefillResult::AdapterError);
        let position = if matches!(refill, RefillResult::Ready) {
            state.cache_position()
        } else {
            None
        };
        let mut input = if matches!(refill, RefillResult::Ready) {
            let sample = state.output[state.output_offset];
            state.output_offset += 1;
            state.output_pending -= 1;
            if state.output_pending == 0 {
                state.output_offset = 0;
            }
            (!state.output_missing).then(|| canonicalize(sample))
        } else {
            None
        };
        // Intentional silence is real input, never loss or a PLC request. Apply
        // again after PLC so a late interval also reaches its retained lookahead.
        if state.mute_history.muted(position) {
            input = Some(0.0);
        }
        self.record_shortfall(input.is_none());
        if failed {
            saturating_add_counter(&self.adapter_error_count, 1);
        }
        // Reset failures silence all output; the reset has already cleared PLC.
        let (mut output, real) = if state.reset_failed {
            (0.0, false)
        } else {
            match &mut state.plc {
                Some(plc) => plc.process(input),
                None => (input.unwrap_or(0.0), input.is_some()),
            }
        };
        let delayed_position = if state.plc_positions.is_empty() {
            position
        } else {
            let old = state.plc_positions[state.plc_position];
            state.plc_positions[state.plc_position] = position;
            state.plc_position = (state.plc_position + 1) % state.plc_positions.len();
            old
        };
        if state.mute_history.muted(delayed_position) {
            output = 0.0;
        }
        state.mute_history.retire(delayed_position);
        (output, real, failed)
    }

    /// Render a validated native PCM block without callback-size assumptions.
    pub(crate) fn consumer_render(&self, output: &mut [f32]) -> (u64, bool) {
        let mut real_samples = 0_u64;
        let mut adapter_error = false;
        for sample in output {
            let (rendered, real, errored) = self.consumer_render_sample();
            *sample = rendered;
            real_samples += u64::from(real);
            adapter_error |= errored;
        }
        (real_samples, adapter_error)
    }

    /// End the current burst and discard unread/converted pre-edge PCM.
    /// A failed adapter reset leaves rendering silent until a reset succeeds.
    pub(crate) fn consumer_reset(&self) -> Result<(), ()> {
        let written = self.written.load(Ordering::Acquire);
        self.read.store(written, Ordering::Release);
        let state = unsafe { &mut *self.consumer.get() };
        self.mute_mailbox.reset();
        let result = state.reset();
        self.reset_serial.fetch_add(1, Ordering::Release);
        result
    }

    /// Return readable producer samples, bounded to the configured capacity.
    pub(crate) fn available(&self) -> u64 {
        let written = self.written.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        self.cursor_distance(written, read).min(self.capacity_u64)
    }

    /// Return forward distance in the capacity-aligned two-lap cursor domain.
    /// The u32 capacity limit keeps this u64 sum and subtraction in range.
    fn cursor_distance(&self, written: u64, read: u64) -> u64 {
        let modulus = self.capacity_u64 * 2;
        (written + modulus - read) % modulus
    }

    /// Capture an individually current, lock-free diagnostic snapshot.
    pub(crate) fn observe(&self) -> Observation {
        Observation {
            capacity_samples: self.capacity_u64,
            available_samples: self.available(),
            reserve_samples: self.reserve_samples,
            filtered_occupancy_samples: self.filtered_occupancy_milli.load(Ordering::Relaxed)
                / 1000,
            target_samples: self.target_samples,
            ratio_correction_ppm: self.ratio_correction_ppm.load(Ordering::Relaxed),
            discarded_samples: self.discarded.load(Ordering::Relaxed),
            missing_samples: self.missing.load(Ordering::Relaxed),
            consecutive_shortfall_samples: self.consecutive_shortfall.load(Ordering::Relaxed),
            shortfall_average_milli: self
                .shortfall_average_micro
                .load(Ordering::Relaxed)
                .saturating_add(999)
                / 1000,
            adapter_error_count: self.adapter_error_count.load(Ordering::Relaxed),
        }
    }

    /// Update renderer-owned shortfall counters for exactly one output sample.
    fn record_shortfall(&self, missing: bool) {
        if missing {
            saturating_add_counter(&self.missing, 1);
        }
        let consecutive = if !missing {
            self.consecutive_shortfall.store(0, Ordering::Relaxed);
            0
        } else {
            saturating_add_counter(&self.consecutive_shortfall, 1)
        };
        let denominator = u64::from(self.output_rate_hz).saturating_mul(10);
        let weight = 1_u64;
        let average = self.shortfall_average_micro.load(Ordering::Relaxed);
        let measured = consecutive.saturating_mul(1_000_000);
        let adjusted = (average as f64
            + ((measured as f64 - average as f64) * weight as f64 / denominator as f64))
            .clamp(0.0, u64::MAX as f64) as u64;
        self.shortfall_average_micro
            .store(adjusted, Ordering::Relaxed);
    }
}

impl ConsumerState {
    /// Map cached output using the adapter's actual pre/post source-consumption
    /// positions. Its fixed ratio within a bounded process call makes this affine;
    /// compensation is reset for that call, not inferred from requested occupancy.
    /// Both neighboring source samples are covered at a fractional boundary.
    fn cache_position(&self) -> Option<Position> {
        let (generation, offset) = self.source_base?;
        let position = self.cache_start
            + (self.cache_end - self.cache_start) * self.output_offset as i128
                / self.cache_count as i128
            + offset * 1_000_000_000;
        let low = u64::try_from(position.div_euclid(1_000_000_000)).ok()?;
        let high = u64::try_from((position + 999_999_999).div_euclid(1_000_000_000)).ok()?;
        Some(Position {
            generation,
            low,
            high,
        })
    }
    /// Discard every ended-burst cache even if the adapter cannot reset.
    /// Conversion stays blocked after failure until a later reset succeeds.
    fn reset(&mut self) -> Result<(), ()> {
        let result = self.converter.reset();
        self.reset_failed = result.is_err();
        self.input_offset = 0;
        self.input_pending = 0;
        self.output_offset = 0;
        self.output_pending = 0;
        self.output_missing = false;
        self.source_taken = 0;
        self.source_accepted = 0;
        self.gaps.clear();
        self.source_base = None;
        self.cache_count = 0;
        self.mute_history.clear();
        self.mute_fault = false;
        self.plc_positions.fill(None);
        self.plc_position = 0;
        if let Some(plc) = &mut self.plc {
            plc.reset();
        }
        self.zero_progress = false;
        self.occupancy_milli = 0;
        self.ratio = 0.0;
        self.primed = false;
        result
    }

    /// Refill the converted cache with one bounded persistent SRC operation.
    fn refill_output(&mut self, ring: &Ring, target_samples: u64) -> RefillResult {
        if self.reset_failed {
            return RefillResult::AdapterError;
        }
        if self.output_pending != 0 {
            return RefillResult::Ready;
        }
        let quantum = ring.capacity.min(CONVERTER_QUANTUM);
        let desired_input = if self.zero_progress {
            self.input_pending
                .saturating_add(quantum)
                .min(ring.capacity)
        } else {
            quantum
        };
        self.make_input_space(
            desired_input.saturating_sub(self.input_pending),
            ring.capacity,
        );
        while self.input_pending < desired_input {
            let Some(sample) = ring.consumer_take_source_sample() else {
                break;
            };
            if let Some(stamp) = sample.stamp {
                let base = self.source_base.get_or_insert((
                    stamp.generation,
                    i128::from(stamp.source) - self.source_taken as i128,
                ));
                if base.0 != stamp.generation
                    || base.1 + self.source_taken as i128 != i128::from(stamp.source)
                {
                    self.mute_fault = true;
                    return RefillResult::AdapterError;
                }
            }
            if sample.missing {
                if let Some(span) = self
                    .gaps
                    .back_mut()
                    .filter(|span| span.end == self.source_taken)
                {
                    span.end += 1;
                } else {
                    // The preallocated bound covers all live source positions.
                    // Fail closed if a faulty adapter violates its queue bound.
                    if self.gaps.len() == self.gaps.capacity() {
                        return RefillResult::AdapterError;
                    }
                    self.gaps.push_back(GapSpan {
                        start: self.source_taken,
                        end: self.source_taken + 1,
                    });
                }
            }
            self.source_taken += 1;
            self.input[self.input_offset + self.input_pending] = sample.pcm;
            self.input_pending += 1;
        }
        if self.input_pending == 0 && self.zero_progress {
            return RefillResult::Starved;
        }
        let Ok(queued_input) = self.converter.queued_input() else {
            return RefillResult::AdapterError;
        };
        let occupancy = ring
            .available()
            .saturating_add(self.input_pending as u64)
            .saturating_add(u64::from(queued_input))
            .saturating_mul(1000);
        if occupancy != 0 {
            if self.occupancy_milli == 0 {
                self.occupancy_milli = occupancy;
            } else {
                let difference = occupancy as i64 - self.occupancy_milli as i64;
                self.occupancy_milli = (self.occupancy_milli as i64 + difference / 128) as u64;
            }
            ring.filtered_occupancy_milli
                .store(self.occupancy_milli, Ordering::Relaxed);

            let output_rate_hz = ring.output_rate_hz;
            let input_rate_hz = ring.input_rate_hz;
            let nominal = f64::from(output_rate_hz) / f64::from(input_rate_hz);
            let error = if target_samples == 0 {
                0.0
            } else {
                ((self.occupancy_milli as f64 - target_samples as f64 * 1000.0)
                    / (target_samples as f64 * 1000.0))
                    .clamp(-1.0, 1.0)
            };
            let desired = (nominal * (1.0 - error * 0.001)).clamp(MINIMUM_RATIO, MAXIMUM_RATIO);
            self.ratio = if self.ratio == 0.0 {
                nominal
            } else {
                self.ratio + (desired - self.ratio) / 512.0
            }
            .clamp(MINIMUM_RATIO, MAXIMUM_RATIO);
            let ppm = ((self.ratio / nominal - 1.0) * 1_000_000.0)
                .round()
                .clamp(i32::MIN as f64, i32::MAX as f64) as i32;
            ring.ratio_correction_ppm.store(ppm, Ordering::Relaxed);
        } else if self.ratio == 0.0 {
            self.ratio = f64::from(ring.output_rate_hz) / f64::from(ring.input_rate_hz);
        }

        self.output_missing = false;
        if !self.gaps.is_empty() {
            let Ok(delay) = self.converter.input_delay() else {
                return RefillResult::AdapterError;
            };
            // Query before conversion, paired with only actually accepted
            // source frames. The rounded position belongs to the next output;
            // a call producing no output consumes no position. Negative startup
            // positions are zero-extended real history, never a source gap.
            let position = self
                .source_accepted
                .checked_sub((delay as u128).div_ceil(1_000_000_000));
            if let Some(position) = position {
                while self.gaps.front().is_some_and(|span| position >= span.end) {
                    self.gaps.pop_front();
                }
                self.output_missing = self.gaps.front().is_some_and(|span| position >= span.start);
            }
        }
        // The input scan above discovers new spans before SWR can generate
        // their output. Existing cache is drained before entering this method.
        let output_count = if self.gaps.is_empty() { quantum } else { 1 };
        let source_start = if self.source_base.is_some() {
            match self.converter.input_delay() {
                Ok(delay) => self.source_accepted as i128 * 1_000_000_000 - i128::from(delay),
                Err(()) => return RefillResult::AdapterError,
            }
        } else {
            0
        };
        let input = &self.input[self.input_offset..self.input_offset + self.input_pending];
        let output = &mut self.output[..output_count];
        let Ok((input_used, output_generated)) = self.converter.process(input, output, self.ratio)
        else {
            return RefillResult::AdapterError;
        };
        self.input_offset += input_used;
        self.input_pending -= input_used;
        self.source_accepted += input_used as u128;
        if self.source_base.is_some() && output_generated != 0 {
            let Ok(delay) = self.converter.input_delay() else {
                return RefillResult::AdapterError;
            };
            self.cache_start = source_start;
            self.cache_end = self.source_accepted as i128 * 1_000_000_000 - i128::from(delay);
            if self.cache_end < self.cache_start {
                self.mute_fault = true;
                return RefillResult::AdapterError;
            }
            self.cache_count = output_generated;
        }
        if self.input_pending == 0 {
            self.input_offset = 0;
        }
        self.output_offset = 0;
        self.output_pending = output_generated;
        if output_generated != 0 {
            self.zero_progress = false;
            return RefillResult::Ready;
        }
        self.zero_progress = input_used == 0;
        if self.zero_progress && self.input_pending == ring.capacity {
            return RefillResult::AdapterError;
        }
        RefillResult::Starved
    }

    /// Compact unconsumed converter input before appending more source PCM.
    fn make_input_space(&mut self, additional: usize, capacity: usize) {
        if self.input_offset == 0
            || self
                .input_offset
                .saturating_add(self.input_pending)
                .saturating_add(additional)
                <= capacity
        {
            return;
        }
        self.input
            .copy_within(self.input_offset..self.input_offset + self.input_pending, 0);
        self.input_offset = 0;
    }
}

/// Allocate a boxed canonical PCM workspace without panicking on OOM.
pub(crate) fn allocate_samples(length: usize) -> Result<Box<[f32]>, ()> {
    let mut samples = Vec::new();
    if samples.try_reserve_exact(length).is_err() {
        return Err(());
    }
    samples.resize(length, 0.0);
    Ok(samples.into_boxed_slice())
}

/// Reserve every real-time workspace before either audio endpoint starts.
fn allocate_workspaces(capacity: usize) -> Result<Workspaces, ()> {
    Ok(Workspaces {
        storage: allocate_storage(capacity)?,
        input: allocate_samples(capacity)?,
        output: allocate_samples(capacity)?,
    })
}

/// Allocate the SPSC payload cells outside real-time processing.
fn allocate_storage(length: usize) -> Result<Box<[UnsafeCell<SourceSample>]>, ()> {
    let mut storage = Vec::new();
    if storage.try_reserve_exact(length).is_err() {
        return Err(());
    }
    storage.resize_with(length, || {
        UnsafeCell::new(SourceSample {
            pcm: 0.0,
            missing: false,
            stamp: None,
        })
    });
    Ok(storage.into_boxed_slice())
}

/// Add to a single-writer diagnostic counter without allowing a lifetime wrap.
fn saturating_add_counter(counter: &AtomicU64, value: u64) -> u64 {
    let next = counter.load(Ordering::Relaxed).saturating_add(value);
    counter.store(next, Ordering::Relaxed);
    next
}

/// Constrain public PCM to the canonical finite normalized range.
fn canonicalize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
#[path = "../tests/unit/ring_state.rs"]
mod state_tests;
