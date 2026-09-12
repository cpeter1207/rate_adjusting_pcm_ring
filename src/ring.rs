//! Lock-free SPSC PCM storage, rate recovery, and bounded concealment.
//!
//! The producer and consumer share only monotonic atomic cursors and
//! observable counters.  Conversion, occupancy control, and concealment are
//! owned exclusively by the consumer, so neither audio operation allocates,
//! locks, logs, or performs I/O after construction.

use core::cell::UnsafeCell;
use core::ffi::c_int;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

use crate::samplerate_adapter::{AdapterFunctions, Converter};

/// Lowest speech pitch evaluated by the generic concealment algorithm.
const PLC_MIN_PITCH_HZ: u32 = 60;
/// Highest speech pitch evaluated by the generic concealment algorithm.
const PLC_MAX_PITCH_HZ: u32 = 400;
/// Recent waveform span used to compare possible pitch periods.
const PLC_ANALYSIS_MS: u32 = 5;
/// Equal-power waveform transition at loss and recovery boundaries.
const PLC_CROSSFADE_MS: u32 = 8;
/// Full-level continuation period before a sustained loss begins fading.
const PLC_HOLD_MS: u32 = 10;
/// Maximum continuation duration before silence replaces repeated history.
const PLC_FADE_MS: u32 = 60;
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

/// One quality selection shared with the public C ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Quality {
    /// Highest conversion quality offered by the selected adapter.
    Best,
    /// Balanced quality and CPU use offered by the selected adapter.
    Medium,
    /// Lowest-latency quality offered by the selected adapter.
    Fastest,
}

impl Quality {
    /// Parse the stable C ABI numeric representation.
    pub(crate) fn from_ffi(value: u32) -> Result<Self, ()> {
        match value {
            0 => Ok(Self::Best),
            1 => Ok(Self::Medium),
            2 => Ok(Self::Fastest),
            _ => Err(()),
        }
    }

    /// Return the adapter-neutral conversion-quality value.
    const fn as_adapter(self) -> c_int {
        match self {
            Self::Best => 0,
            Self::Medium => 1,
            Self::Fastest => 2,
        }
    }
}

/// Lock-free counters exposed as a best-effort snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observation {
    /// Immutable sample capacity.
    pub(crate) capacity_samples: u64,
    /// Source samples currently readable by the consumer.
    pub(crate) available_samples: u64,
    /// Latest requested retained floor, for diagnostics only.
    pub(crate) reserve_samples: u64,
    /// Low-pass filtered source occupancy.
    pub(crate) filtered_occupancy_samples: u64,
    /// Latest occupancy target used by the slow controller.
    pub(crate) target_samples: u64,
    /// Applied conversion-ratio correction relative to nominal rate.
    pub(crate) ratio_correction_ppm: i32,
    /// Input samples rejected because unread storage was full.
    pub(crate) discarded_samples: u64,
    /// Output samples replaced by concealment.
    pub(crate) missing_samples: u64,
    /// Consecutive output samples currently concealed.
    pub(crate) consecutive_shortfall_samples: u64,
    /// Ten-second moving average of consecutive shortfall in millisamples.
    pub(crate) shortfall_average_milli: u64,
    /// Consumer SRC calls rejected after successful construction.
    pub(crate) adapter_error_count: u64,
}

/// Control-plane creation failure classified for the stable C ABI.
#[derive(Debug)]
pub(crate) enum CreateError {
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

/// Persistent state touched exclusively by the consumer endpoint.
struct ConsumerState {
    converter: Converter,
    input: Box<[f32]>,
    output: Box<[f32]>,
    history: Box<[f32]>,
    plc_waveform: Box<[f32]>,
    input_offset: usize,
    input_pending: usize,
    output_offset: usize,
    output_pending: usize,
    history_length: usize,
    history_next: usize,
    plc_period: usize,
    plc_samples: usize,
    recovery_samples: usize,
    zero_progress: bool,
    occupancy_milli: u64,
    ratio: f64,
}

/// All preallocated payload and consumer workspaces owned by one ring.
struct Workspaces {
    storage: Box<[UnsafeCell<f32>]>,
    input: Box<[f32]>,
    output: Box<[f32]>,
    history: Box<[f32]>,
    plc_waveform: Box<[f32]>,
}

/// Lock-free producer/consumer state and consumer-only DSP workspace.
///
/// `UnsafeCell` stores the payload because one producer writes a slot before
/// its release-store advances `written`, while one consumer reads that slot
/// only after acquiring `written`.  The SPSC contract is part of the public
/// ABI.  Every other shared field is an atomic diagnostic or cursor.
pub(crate) struct Ring {
    storage: Box<[UnsafeCell<f32>]>,
    capacity: usize,
    capacity_u64: u64,
    written: AtomicU64,
    read: AtomicU64,
    discarded: AtomicU64,
    missing: AtomicU64,
    consecutive_shortfall: AtomicU64,
    /// Internal precision prevents a one-sample EMA update rounding to zero.
    shortfall_average_micro: AtomicU64,
    reserve_samples: AtomicU64,
    target_samples: AtomicU64,
    filtered_occupancy_milli: AtomicU64,
    ratio_correction_ppm: AtomicI32,
    adapter_error_count: AtomicU64,
    input_rate_hz: AtomicU32,
    output_rate_hz: AtomicU32,
    consumer: UnsafeCell<ConsumerState>,
}

// The only non-atomic sharing is protected by the documented one-producer,
// one-consumer cursor protocol described above.  The sole consumer owns the
// `ConsumerState` behind its UnsafeCell for the lifetime of a running ring.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Allocate a ring and its converter before either real-time endpoint runs.
    pub(crate) fn create(
        capacity: usize,
        input_rate_hz: u32,
        output_rate_hz: u32,
        quality: Quality,
        adapter: AdapterFunctions,
    ) -> Result<Self, CreateError> {
        Self::create_with_minimum(
            capacity,
            input_rate_hz,
            output_rate_hz,
            quality,
            adapter,
            MINIMUM_CAPACITY,
        )
    }

    /// Construct the frozen ABI-major-one facade with its historic small sizes.
    ///
    /// ABI major one permitted every nonzero capacity. New F32 callers use the
    /// safer documented minimum through the canonical F32 constructor instead.
    pub(crate) fn legacy_create(
        capacity: usize,
        input_rate_hz: u32,
        output_rate_hz: u32,
        quality: Quality,
        adapter: AdapterFunctions,
    ) -> Result<Self, CreateError> {
        Self::create_with_minimum(capacity, input_rate_hz, output_rate_hz, quality, adapter, 1)
    }

    /// Reset converter-only state for a stopped ABI-major-one rate change.
    ///
    /// The released S16 interface retained unread source PCM and diagnostics
    /// across `rpcr_set_rates()`. Its compatibility facade therefore resets
    /// only persistent conversion and controller state instead of replacing
    /// the Rust ring and discarding queued samples.
    pub(crate) fn legacy_reconfigure(
        &self,
        input_rate_hz: u32,
        output_rate_hz: u32,
    ) -> Result<(), CreateError> {
        if input_rate_hz == 0 || output_rate_hz == 0 {
            return Err(CreateError::NoMemory);
        }
        // The ABI-major-one contract requires both endpoints to be stopped,
        // so its compatibility bridge may reset the consumer-owned converter.
        let state = unsafe { &mut *self.consumer.get() };
        state.converter.reset().map_err(|()| CreateError::Adapter)?;
        state.reset_after_rate_change();
        self.input_rate_hz.store(input_rate_hz, Ordering::Relaxed);
        self.output_rate_hz.store(output_rate_hz, Ordering::Relaxed);
        Ok(())
    }

    /// Allocate one immutable-rate ring using the selected ABI's capacity rule.
    fn create_with_minimum(
        capacity: usize,
        input_rate_hz: u32,
        output_rate_hz: u32,
        quality: Quality,
        adapter: AdapterFunctions,
        minimum_capacity: usize,
    ) -> Result<Self, CreateError> {
        if capacity < minimum_capacity || input_rate_hz == 0 || output_rate_hz == 0 {
            return Err(CreateError::NoMemory);
        }

        let Workspaces {
            storage,
            input,
            output,
            history,
            plc_waveform,
        } = match allocate_workspaces(capacity) {
            Ok(workspaces) => workspaces,
            Err(()) => return Err(CreateError::NoMemory),
        };
        let converter = match adapter.create_converter(quality.as_adapter()) {
            Ok(converter) => converter,
            Err(()) => return Err(CreateError::Adapter),
        };
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
            reserve_samples: AtomicU64::new(0),
            target_samples: AtomicU64::new(0),
            filtered_occupancy_milli: AtomicU64::new(0),
            ratio_correction_ppm: AtomicI32::new(0),
            adapter_error_count: AtomicU64::new(0),
            input_rate_hz: AtomicU32::new(input_rate_hz),
            output_rate_hz: AtomicU32::new(output_rate_hz),
            consumer: UnsafeCell::new(ConsumerState {
                converter,
                input,
                output,
                history,
                plc_waveform,
                input_offset: 0,
                input_pending: 0,
                output_offset: 0,
                output_pending: 0,
                history_length: 0,
                history_next: 0,
                plc_period: 0,
                plc_samples: 0,
                recovery_samples: 0,
                zero_progress: false,
                occupancy_milli: 0,
                ratio: 0.0,
            }),
        })
    }

    /// Publish one canonical input sample without waiting or allocating.
    pub(crate) fn producer_push_sample(&self, sample: f32) -> bool {
        let written = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if written.wrapping_sub(read) >= self.capacity_u64 {
            saturating_add_counter(&self.discarded, 1);
            return false;
        }
        let index = (written % self.capacity_u64) as usize;
        // The producer owns this slot until the release store below publishes
        // it.  The sole consumer acquires `written` before reading it.
        unsafe {
            *self.storage[index].get() = canonicalize(sample);
        }
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
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

    /// Consume one source sample for the renderer's persistent converter.
    fn consumer_take_source_sample(&self) -> Option<f32> {
        let read = self.read.load(Ordering::Relaxed);
        let written = self.written.load(Ordering::Acquire);
        if read == written {
            return None;
        }
        let index = (read % self.capacity_u64) as usize;
        // The producer's release publication above makes this slot readable;
        // the consumer alone advances `read`, so no second consumer can race.
        let sample = unsafe { *self.storage[index].get() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(sample)
    }

    /// Consume one unconverted source sample for the frozen ABI-major-one shim.
    ///
    /// ABI major two deliberately exposes rendering only, so every new user
    /// receives rate recovery. The old public ABI included a raw-pop entry
    /// point, however, and its small Rust compatibility bridge must retain
    /// that published behavior until ABI-major-one consumers migrate.
    pub(crate) fn legacy_consumer_pop_source_sample(&self) -> Option<f32> {
        self.consumer_take_source_sample()
    }

    /// Publish the ABI-major-one reserve diagnostic before sample rendering.
    ///
    /// Reserve has never gated playout; it is retained for observability and
    /// source compatibility with the signed-16 interface.
    pub(crate) fn legacy_set_reserve(&self, reserve_samples: u64) {
        self.reserve_samples
            .store(reserve_samples, Ordering::Relaxed);
    }

    /// Produce one compatibility concealment sample without consuming source.
    ///
    /// The released ABI-major-one block renderer uses this only when a caller
    /// requests more output samples than its fixed workspace can render. That
    /// historical exceptional path advanced PLC state without reading source
    /// PCM or accounting a normal renderer shortfall.
    pub(crate) fn legacy_conceal_sample(&self) -> f32 {
        let output_rate_hz = self.output_rate_hz.load(Ordering::Relaxed);
        // The legacy contract assigns this operation to the sole consumer.
        let state = unsafe { &mut *self.consumer.get() };
        state.conceal_one(output_rate_hz, self.capacity)
    }

    /// Return the immutable consumer sample rate for a legacy statistics shim.
    pub(crate) fn output_rate_hz(&self) -> u32 {
        self.output_rate_hz.load(Ordering::Relaxed)
    }

    /// Return the ABI-major-one source cursor snapshots without exposing them
    /// through the new opaque F32 interface.
    pub(crate) fn legacy_cursor_positions(&self) -> (u64, u64) {
        (
            self.written.load(Ordering::Acquire),
            self.read.load(Ordering::Acquire),
        )
    }

    /// Return the private discarded counter for the ABI-major-one block shim.
    pub(crate) fn legacy_discarded_counter(&self) -> &AtomicU64 {
        &self.discarded
    }

    /// Render one hardware-paced sample through persistent conversion.
    pub(crate) fn consumer_render_sample(&self, target_samples: u64) -> (f32, bool, bool) {
        self.target_samples.store(target_samples, Ordering::Relaxed);
        let output_rate_hz = self.output_rate_hz.load(Ordering::Relaxed);
        // This mutable state is exclusively consumer-owned by the public SPSC
        // contract.  Producer calls cannot access it.
        let state = unsafe { &mut *self.consumer.get() };
        let refill = state.refill_output(self, target_samples);
        if !matches!(refill, RefillResult::Ready) {
            let output = state.conceal_one(output_rate_hz, self.capacity);
            self.record_shortfall(true);
            let adapter_error = matches!(refill, RefillResult::AdapterError);
            if adapter_error {
                saturating_add_counter(&self.adapter_error_count, 1);
            }
            return (output, false, adapter_error);
        }

        let sample = state.output[state.output_offset];
        state.output_offset += 1;
        state.output_pending -= 1;
        if state.output_pending == 0 {
            state.output_offset = 0;
        }
        let output = state.recover_one(sample, output_rate_hz);
        state.remember_output(output, self.capacity);
        self.record_shortfall(false);
        (output, true, false)
    }

    /// Render an arbitrary native PCM block without a fixed callback duration.
    pub(crate) fn consumer_render(
        &self,
        output: &mut [f32],
        reserve_samples: u64,
        target_samples: u64,
    ) -> (u64, bool) {
        self.reserve_samples
            .store(reserve_samples, Ordering::Relaxed);
        let mut real_samples = 0_u64;
        let mut adapter_error = false;
        for sample in output {
            let (rendered, real, errored) = self.consumer_render_sample(target_samples);
            *sample = rendered;
            if real {
                real_samples = real_samples.saturating_add(1);
            }
            adapter_error |= errored;
        }
        (real_samples, adapter_error)
    }

    /// Return readable producer samples, bounded to the configured capacity.
    pub(crate) fn available(&self) -> u64 {
        let written = self.written.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        written.wrapping_sub(read).min(self.capacity_u64)
    }

    /// Capture an individually current, lock-free diagnostic snapshot.
    pub(crate) fn observe(&self) -> Observation {
        Observation {
            capacity_samples: self.capacity_u64,
            available_samples: self.available(),
            reserve_samples: self.reserve_samples.load(Ordering::Relaxed),
            filtered_occupancy_samples: self.filtered_occupancy_milli.load(Ordering::Relaxed)
                / 1000,
            target_samples: self.target_samples.load(Ordering::Relaxed),
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
        let denominator = u64::from(self.output_rate_hz.load(Ordering::Relaxed)).saturating_mul(10);
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
    /// Refill the converted cache with one bounded persistent SRC operation.
    fn refill_output(&mut self, ring: &Ring, target_samples: u64) -> RefillResult {
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
            self.input[self.input_offset + self.input_pending] = sample;
            self.input_pending += 1;
        }
        if self.input_pending == 0 {
            return RefillResult::Starved;
        }

        let occupancy = ring
            .available()
            .saturating_add(self.input_pending as u64)
            .saturating_mul(1000);
        if self.occupancy_milli == 0 {
            self.occupancy_milli = occupancy;
        } else {
            let difference = occupancy as i64 - self.occupancy_milli as i64;
            self.occupancy_milli = (self.occupancy_milli as i64 + difference / 128) as u64;
        }
        ring.filtered_occupancy_milli
            .store(self.occupancy_milli, Ordering::Relaxed);

        let output_rate_hz = ring.output_rate_hz.load(Ordering::Relaxed);
        let input_rate_hz = ring.input_rate_hz.load(Ordering::Relaxed);
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

        let input = &self.input[self.input_offset..self.input_offset + self.input_pending];
        let output = &mut self.output[..quantum];
        let Ok((input_used, output_generated)) = self.converter.process(input, output, self.ratio)
        else {
            return RefillResult::AdapterError;
        };
        self.input_offset += input_used;
        self.input_pending -= input_used;
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

    /// Clear only converter-derived state after a stopped legacy rate change.
    fn reset_after_rate_change(&mut self) {
        self.input_offset = 0;
        self.input_pending = 0;
        self.output_offset = 0;
        self.output_pending = 0;
        self.plc_period = 0;
        self.plc_samples = 0;
        self.recovery_samples = 0;
        self.zero_progress = false;
        self.occupancy_milli = 0;
        self.ratio = 0.0;
    }

    /// Retain actual playout for bounded later pitch-period continuation.
    fn remember_output(&mut self, sample: f32, capacity: usize) {
        self.history[self.history_next] = sample;
        self.history_next = (self.history_next + 1) % capacity;
        self.history_length = self.history_length.saturating_add(1).min(capacity);
    }

    /// Read one historical waveform sample before the next insertion cursor.
    fn history_back(&self, distance: usize, capacity: usize) -> f32 {
        let offset = distance % capacity;
        self.history[(self.history_next + capacity - offset) % capacity]
    }

    /// Detect the least-error voiced period in recent real playout history.
    fn detect_pitch(&self, output_rate_hz: u32, capacity: usize) -> usize {
        let mut minimum = (output_rate_hz / PLC_MAX_PITCH_HZ) as usize;
        let maximum = (output_rate_hz / PLC_MIN_PITCH_HZ) as usize;
        let mut window = milliseconds_to_samples(output_rate_hz, PLC_ANALYSIS_MS);
        let mut resolution = (output_rate_hz / 48_000) as usize;
        if minimum == 0 {
            minimum = 1;
        }
        if window == 0 {
            window = 1;
        }
        if resolution == 0 {
            resolution = 1;
        }
        if maximum < minimum || self.history_length < maximum.saturating_add(window) {
            return 0;
        }
        let mut best_error = f64::INFINITY;
        let mut best_period = 0;
        let mut period = minimum;
        while period <= maximum {
            let mut error = 0.0_f64;
            let mut index = 0;
            while index < window {
                let difference = self.history_back(index + 1, capacity)
                    - self.history_back(index + period + 1, capacity);
                error += f64::from(difference.abs());
                index = index.saturating_add(resolution);
            }
            if error < best_error {
                best_error = error;
                best_period = period;
            }
            period = period.saturating_add(resolution);
        }
        best_period
    }

    /// Render one pitch-continuation sample, including bounded decay.
    fn concealed_sample(&self, position: usize, output_rate_hz: u32) -> f32 {
        if self.plc_period == 0 {
            return 0.0;
        }
        let phase = position % self.plc_period;
        let sample = self.plc_waveform[phase];
        attenuate_concealment(sample, position, output_rate_hz)
    }

    /// Produce one concealed sample after an input shortfall.
    fn conceal_one(&mut self, output_rate_hz: u32, capacity: usize) -> f32 {
        if self.plc_samples == 0 {
            self.plc_period = self.detect_pitch(output_rate_hz, capacity);
            self.capture_plc_waveform(capacity);
        }
        self.recovery_samples = 0;
        let position = self.plc_samples;
        let mut sample = self.concealed_sample(position, output_rate_hz);
        let crossfade = milliseconds_to_samples(output_rate_hz, PLC_CROSSFADE_MS);
        if crossfade != 0 && position < crossfade {
            let previous = if self.history_length == 0 {
                0.0
            } else {
                self.history_back(1, capacity)
            };
            sample = equal_power_mix(previous, sample, position, crossfade);
        }
        self.plc_samples = self.plc_samples.saturating_add(1);
        canonicalize(sample)
    }

    /// Snapshot the detected period before real output mutates history again.
    fn capture_plc_waveform(&mut self, capacity: usize) {
        for phase in 0..self.plc_period {
            self.plc_waveform[phase] = self.history_back(self.plc_period - phase, capacity);
        }
    }

    /// Smooth one real converted sample back from an active concealment run.
    fn recover_one(&mut self, sample: f32, output_rate_hz: u32) -> f32 {
        if self.plc_samples == 0 {
            return canonicalize(sample);
        }
        let crossfade = milliseconds_to_samples(output_rate_hz, PLC_CROSSFADE_MS);
        if crossfade == 0 {
            self.plc_period = 0;
            self.plc_samples = 0;
            return canonicalize(sample);
        }
        if self.recovery_samples == 0 {
            self.recovery_samples = crossfade;
        }
        let index = crossfade - self.recovery_samples;
        let continued = self.concealed_sample(self.plc_samples + index, output_rate_hz);
        let mixed = equal_power_mix(continued, sample, index, crossfade);
        self.recovery_samples -= 1;
        if self.recovery_samples == 0 {
            self.plc_period = 0;
            self.plc_samples = 0;
        }
        canonicalize(mixed)
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
        history: allocate_samples(capacity)?,
        plc_waveform: allocate_samples(capacity)?,
    })
}

/// Allocate the SPSC payload cells outside real-time processing.
fn allocate_storage(length: usize) -> Result<Box<[UnsafeCell<f32>]>, ()> {
    let mut storage = Vec::new();
    if storage.try_reserve_exact(length).is_err() {
        return Err(());
    }
    storage.resize_with(length, || UnsafeCell::new(0.0));
    Ok(storage.into_boxed_slice())
}

/// Add to a single-writer diagnostic counter without allowing a lifetime wrap.
fn saturating_add_counter(counter: &AtomicU64, value: u64) -> u64 {
    let next = counter.load(Ordering::Relaxed).saturating_add(value);
    counter.store(next, Ordering::Relaxed);
    next
}

/// Convert a duration to output-rate samples without a callback-size assumption.
fn milliseconds_to_samples(rate_hz: u32, milliseconds: u32) -> usize {
    (u64::from(rate_hz).saturating_mul(u64::from(milliseconds)) / 1000) as usize
}

/// Constrain public PCM to the canonical finite normalized range.
fn canonicalize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Apply the bounded continuation envelope with no integer PCM conversion.
pub(crate) fn attenuate_concealment(sample: f32, position: usize, output_rate_hz: u32) -> f32 {
    let hold = milliseconds_to_samples(output_rate_hz, PLC_HOLD_MS);
    let fade = milliseconds_to_samples(output_rate_hz, PLC_FADE_MS);
    if fade == 0 || position <= hold {
        return sample;
    }
    if position >= fade {
        return 0.0;
    }
    sample * (fade - position) as f32 / (fade - hold) as f32
}

/// Blend waveform segments with equal-power f64 control math.
fn equal_power_mix(outgoing: f32, incoming: f32, index: usize, samples: usize) -> f32 {
    let progress = (index as f64 + 1.0) / (samples as f64 + 1.0);
    let mixed =
        (1.0 - progress).sqrt() * f64::from(outgoing) + progress.sqrt() * f64::from(incoming);
    canonicalize(mixed as f32)
}
