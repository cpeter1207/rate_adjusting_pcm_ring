//! Frozen ABI-major-one compatibility bridge implemented by the Rust core.
//!
//! The released signed-16 C header embeds a public \`rpcr_ring\` structure, so
//! it cannot become opaque without breaking already-built consumers. The C
//! shared object keeps that layout and forwards through this private
//! descriptor. All ring storage, conversion, recovery, and concealment stay
//! Rust-owned; this module converts only at the legacy S16 boundary.

use core::ffi::{c_char, c_int};
use core::mem::size_of;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::ring::{CreateError, Observation, Quality, Ring};
use crate::samplerate_adapter::{self, AdapterFunctions};

/// Private ABI version used only by the ABI-major-one C forwarding shim.
const ABI_VERSION: u32 = 1;
/// Successful bridge operation.
const OK: c_int = 0;
/// Invalid pointer, rate, quality, or count.
const INVALID_ARGUMENT: c_int = -1;
/// Setup allocation could not reserve persistent state.
const NO_MEMORY: c_int = -2;
/// The mandatory sample-rate adapter rejected setup or rendering.
const ADAPTER_ERROR: c_int = -3;
/// Stable private capability name validated by the C forwarding shim.
const CAPABILITY: &[u8] = b"rptadv.rate-adjusting-pcm-ring.legacy-s16\0";

/// Snapshot copied into the ABI-major-one C structure by its forwarding shim.
#[repr(C)]
pub(crate) struct LegacyObservation {
    pub(crate) capacity_samples: u64,
    pub(crate) available_samples: u64,
    pub(crate) written_samples: u64,
    pub(crate) read_samples: u64,
    pub(crate) reserve_samples: u64,
    pub(crate) filtered_occupancy_samples: u64,
    pub(crate) target_samples: u64,
    pub(crate) ratio_correction_ppm: i32,
    pub(crate) discarded_samples: u64,
    pub(crate) missing_samples: u64,
    pub(crate) consecutive_underruns: u64,
    pub(crate) underrun_average_milli: u64,
}

/// Counter set that retains ABI-major-one shortfall accounting semantics.
struct LegacyStatistics {
    missing: AtomicU64,
    consecutive_underruns: AtomicU64,
    underrun_average_milli: AtomicU64,
}

impl LegacyStatistics {
    /// Create zeroed compatibility counters before either endpoint starts.
    const fn new() -> Self {
        Self {
            missing: AtomicU64::new(0),
            consecutive_underruns: AtomicU64::new(0),
            underrun_average_milli: AtomicU64::new(0),
        }
    }

    /// Preserve the published signed-16 shortfall EWMA semantics.
    fn record(&self, missing: u64, samples: u64, rate_hz: u32) {
        saturating_add(&self.missing, missing);
        let consecutive = if missing == 0 {
            self.consecutive_underruns.store(0, Ordering::Relaxed);
            0
        } else {
            saturating_add(&self.consecutive_underruns, missing)
        };
        let denominator = u64::from(rate_hz).saturating_mul(10_000);
        let weight = samples.saturating_mul(1000).min(denominator);
        let average = self.underrun_average_milli.load(Ordering::Relaxed);
        let measured = consecutive.saturating_mul(1000);
        let adjusted = if denominator == 0 {
            average
        } else {
            // Keep the released integer EWMA's truncation behavior without
            // allowing an intermediate signed multiplication to overflow.
            let difference = i128::from(measured) - i128::from(average);
            let adjustment = difference * i128::from(weight) / i128::from(denominator);
            (i128::from(average) + adjustment).clamp(0, i128::from(u64::MAX)) as u64
        };
        self.underrun_average_milli
            .store(adjusted, Ordering::Relaxed);
    }
}

/// Rust-owned state for one ABI-major-one C \`rpcr_ring\` forwarding handle.
pub(crate) struct LegacyRing {
    ring: Ring,
    statistics: LegacyStatistics,
}

impl LegacyRing {
    /// Construct a legacy handle and persistent Rust ring before audio starts.
    fn create(
        capacity: usize,
        quality: Quality,
        input_rate_hz: u32,
        output_rate_hz: u32,
        adapter: AdapterFunctions,
    ) -> Result<Self, CreateError> {
        Ok(Self {
            ring: Ring::legacy_create(capacity, input_rate_hz, output_rate_hz, quality, adapter)?,
            statistics: LegacyStatistics::new(),
        })
    }

    /// Replace a stopped immutable-rate ring while preserving its C handle.
    fn reconfigure(&self, input_rate_hz: u32, output_rate_hz: u32) -> Result<(), CreateError> {
        self.ring.legacy_reconfigure(input_rate_hz, output_rate_hz)
    }

    /// Return the canonical F32 value for one signed-16 boundary sample.
    fn input_from_i16(sample: i16) -> f32 {
        f32::from(sample) / 32_768.0
    }

    /// Quantize one canonical F32 value at the frozen signed-16 boundary.
    fn output_to_i16(sample: f32) -> i16 {
        let scaled = (sample.clamp(-1.0, 1.0) * 32_768.0).round();
        scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
    }

    /// Push one signed-16 source sample without waiting or allocating.
    fn push_sample(&self, sample: i16) -> bool {
        self.ring.producer_push_sample(Self::input_from_i16(sample))
    }

    /// Push a chronological signed-16 block without a per-sample FFI crossing.
    fn push(&self, input: &[i16]) -> u64 {
        let mut accepted = 0_u64;
        for &sample in input {
            if !self.push_sample(sample) {
                break;
            }
            accepted = accepted.saturating_add(1);
        }
        if accepted < input.len() as u64 {
            let rejected_tail = input.len() as u64 - accepted;
            saturating_add(
                self.ring.legacy_discarded_counter(),
                rejected_tail.saturating_sub(1),
            );
        }
        accepted
    }

    /// Consume one raw legacy source sample, preserving the historical API.
    fn pop_sample(&self) -> Option<i16> {
        self.ring
            .legacy_consumer_pop_source_sample()
            .map(Self::output_to_i16)
    }

    /// Render one signed-16 output sample through the Rust-owned converter.
    fn render_sample(&self, target_samples: u64) -> (i16, bool, bool) {
        let (sample, real, adapter_error) = self.ring.consumer_render_sample(target_samples);
        self.statistics
            .record(u64::from(!real), 1, self.ring.output_rate_hz());
        (Self::output_to_i16(sample), real, adapter_error)
    }

    /// Render a variable block without allocating a temporary F32 buffer.
    fn render(&self, output: &mut [i16], reserve_samples: u64, target_samples: u64) -> (u64, bool) {
        self.ring.legacy_set_reserve(reserve_samples);
        let mut real_samples = 0_u64;
        let mut adapter_error = false;
        for sample in output {
            let (rendered, real, errored) = self.render_sample(target_samples);
            *sample = rendered;
            if real {
                real_samples = real_samples.saturating_add(1);
            }
            adapter_error |= errored;
        }
        (real_samples, adapter_error)
    }

    /// Advance legacy PLC state without consuming queued source PCM.
    fn conceal(&self, output: &mut [i16]) {
        for sample in output {
            *sample = Self::output_to_i16(self.ring.legacy_conceal_sample());
        }
    }

    /// Copy ring state and frozen legacy shortfall measurements.
    fn observe(&self) -> LegacyObservation {
        let observation: Observation = self.ring.observe();
        let (written_samples, read_samples) = self.ring.legacy_cursor_positions();
        LegacyObservation {
            capacity_samples: observation.capacity_samples,
            available_samples: observation.available_samples,
            written_samples,
            read_samples,
            reserve_samples: observation.reserve_samples,
            filtered_occupancy_samples: observation.filtered_occupancy_samples,
            target_samples: observation.target_samples,
            ratio_correction_ppm: observation.ratio_correction_ppm,
            discarded_samples: observation.discarded_samples,
            missing_samples: self.statistics.missing.load(Ordering::Relaxed),
            consecutive_underruns: self
                .statistics
                .consecutive_underruns
                .load(Ordering::Relaxed),
            underrun_average_milli: self
                .statistics
                .underrun_average_milli
                .load(Ordering::Relaxed),
        }
    }
}

/// Add to one diagnostic counter without wrapping a public value.
fn saturating_add(counter: &AtomicU64, value: u64) -> u64 {
    let next = counter.load(Ordering::Relaxed).saturating_add(value);
    counter.store(next, Ordering::Relaxed);
    next
}

/// C function type that creates one frozen ABI-major-one compatibility handle.
type Create = extern "C" fn(u64, u32, u32, u32, *mut *mut LegacyRing) -> c_int;
/// C function type that destroys a stopped compatibility handle.
type Destroy = extern "C" fn(*mut LegacyRing);
/// C function type that changes fixed rates while both endpoints are stopped.
type Reconfigure = extern "C" fn(*mut LegacyRing, u32, u32) -> c_int;
/// C function type that publishes one signed-16 input sample.
type PushSample = extern "C" fn(*mut LegacyRing, i16, *mut bool, *mut LegacyObservation) -> c_int;
/// C function type that publishes a chronological signed-16 input block.
type Push = extern "C" fn(*mut LegacyRing, *const i16, u64, *mut u64) -> c_int;
/// C function type that consumes one raw signed-16 input sample.
type PopSample = extern "C" fn(*mut LegacyRing, *mut i16, *mut bool) -> c_int;
/// C function type that renders one signed-16 output sample.
type RenderSample =
    extern "C" fn(*mut LegacyRing, *mut i16, u64, *mut bool, *mut LegacyObservation) -> c_int;
/// C function type that renders one signed-16 output block.
type Render = extern "C" fn(*mut LegacyRing, *mut i16, u64, u64, u64, *mut u64) -> c_int;
/// C function type that advances legacy concealment without consuming source.
type Conceal = extern "C" fn(*mut LegacyRing, *mut i16, u64) -> c_int;
/// C function type that copies a compatibility observation.
type Observe = extern "C" fn(*const LegacyRing, *mut LegacyObservation) -> c_int;
/// C function type that adds an explicitly observed legacy shortfall.
type RecordShortfall = extern "C" fn(*mut LegacyRing, u64, u64, u32) -> c_int;

/// Private descriptor used only by the separately built ABI-major-one shim.
#[repr(C)]
pub(crate) struct LegacyDescriptor {
    pub(crate) struct_size: u32,
    pub(crate) abi_version: u32,
    pub(crate) capability_name: *const c_char,
    pub(crate) create: Create,
    pub(crate) destroy: Destroy,
    pub(crate) reconfigure: Reconfigure,
    pub(crate) push_sample: PushSample,
    pub(crate) push: Push,
    pub(crate) pop_sample: PopSample,
    pub(crate) render_sample: RenderSample,
    pub(crate) render: Render,
    pub(crate) conceal: Conceal,
    pub(crate) observe: Observe,
    pub(crate) record_shortfall: RecordShortfall,
}

unsafe impl Sync for LegacyDescriptor {}

/// Borrow one valid legacy handle for a real-time operation.
unsafe fn checked_ring<'a>(ring: *mut LegacyRing) -> Result<&'a LegacyRing, c_int> {
    unsafe { ring.as_ref() }.ok_or(INVALID_ARGUMENT)
}

/// Borrow one mutable stopped legacy handle for reconfiguration.
unsafe fn checked_ring_mut<'a>(ring: *mut LegacyRing) -> Result<&'a mut LegacyRing, c_int> {
    unsafe { ring.as_mut() }.ok_or(INVALID_ARGUMENT)
}

/// Convert the numeric legacy quality to the Rust core selection.
fn quality_from_ffi(value: u32) -> Result<Quality, ()> {
    Quality::from_ffi(value)
}

/// Preserve control-plane allocation and adapter errors across the C bridge.
fn create_error_result(error: CreateError) -> c_int {
    match error {
        CreateError::NoMemory => NO_MEMORY,
        CreateError::Adapter => ADAPTER_ERROR,
    }
}

/// Create a stopped handle through one resolved mandatory converter adapter.
fn create_with_adapter(
    capacity: u64,
    quality: u32,
    input_rate_hz: u32,
    output_rate_hz: u32,
    out_ring: *mut *mut LegacyRing,
    adapter: Result<AdapterFunctions, ()>,
) -> c_int {
    let Some(out_ring) = NonNull::new(out_ring) else {
        return INVALID_ARGUMENT;
    };
    unsafe {
        *out_ring.as_ptr() = ptr::null_mut();
    }
    if capacity == 0 || capacity > u64::from(u32::MAX) || input_rate_hz == 0 || output_rate_hz == 0
    {
        return INVALID_ARGUMENT;
    }
    let quality = match quality_from_ffi(quality) {
        Ok(quality) => quality,
        Err(()) => return INVALID_ARGUMENT,
    };
    let adapter = match adapter {
        Ok(adapter) => adapter,
        Err(()) => return ADAPTER_ERROR,
    };
    let ring = match LegacyRing::create(
        capacity as usize,
        quality,
        input_rate_hz,
        output_rate_hz,
        adapter,
    ) {
        Ok(ring) => ring,
        Err(error) => return create_error_result(error),
    };
    unsafe {
        *out_ring.as_ptr() = Box::into_raw(Box::new(ring));
    }
    OK
}

/// Create a stopped handle and all persistent Rust state.
extern "C" fn create(
    capacity: u64,
    quality: u32,
    input_rate_hz: u32,
    output_rate_hz: u32,
    out_ring: *mut *mut LegacyRing,
) -> c_int {
    create_with_adapter(
        capacity,
        quality,
        input_rate_hz,
        output_rate_hz,
        out_ring,
        samplerate_adapter::load_functions(),
    )
}

/// Destroy one stopped legacy handle.
extern "C" fn destroy(ring: *mut LegacyRing) {
    let Some(ring) = NonNull::new(ring) else {
        return;
    };
    unsafe {
        drop(Box::from_raw(ring.as_ptr()));
    }
}

/// Replace a stopped handle's immutable source and destination rates.
extern "C" fn reconfigure(ring: *mut LegacyRing, input_rate_hz: u32, output_rate_hz: u32) -> c_int {
    if input_rate_hz == 0 || output_rate_hz == 0 {
        return INVALID_ARGUMENT;
    }
    let ring = match unsafe { checked_ring_mut(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    ring.reconfigure(input_rate_hz, output_rate_hz)
        .map_or_else(create_error_result, |_| OK)
}

/// Publish one signed-16 source sample.
extern "C" fn push_sample(
    ring: *mut LegacyRing,
    sample: i16,
    accepted: *mut bool,
    observation: *mut LegacyObservation,
) -> c_int {
    let Some(accepted) = NonNull::new(accepted) else {
        return INVALID_ARGUMENT;
    };
    let Some(observation) = NonNull::new(observation) else {
        return INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    unsafe {
        *accepted.as_ptr() = ring.push_sample(sample);
        *observation.as_ptr() = ring.observe();
    }
    OK
}

/// Convert a bounded signed-16 C input block into one immutable slice.
unsafe fn checked_input<'a>(input: *const i16, samples: u64) -> Result<&'a [i16], c_int> {
    if samples == 0 {
        return Ok(&[]);
    }
    if samples > isize::MAX as u64 / size_of::<i16>() as u64 {
        return Err(INVALID_ARGUMENT);
    }
    let input = NonNull::new(input.cast_mut()).ok_or(INVALID_ARGUMENT)?;
    Ok(unsafe { core::slice::from_raw_parts(input.as_ptr(), samples as usize) })
}

/// Convert a bounded signed-16 C output block into one mutable slice.
unsafe fn checked_output<'a>(output: *mut i16, samples: u64) -> Result<&'a mut [i16], c_int> {
    if samples == 0 {
        return Ok(&mut []);
    }
    if samples > isize::MAX as u64 / size_of::<i16>() as u64 {
        return Err(INVALID_ARGUMENT);
    }
    let output = NonNull::new(output).ok_or(INVALID_ARGUMENT)?;
    Ok(unsafe { core::slice::from_raw_parts_mut(output.as_ptr(), samples as usize) })
}

/// Publish one chronological signed-16 source block.
extern "C" fn push(
    ring: *mut LegacyRing,
    input: *const i16,
    samples: u64,
    accepted: *mut u64,
) -> c_int {
    let Some(accepted) = NonNull::new(accepted) else {
        return INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let input = match unsafe { checked_input(input, samples) } {
        Ok(input) => input,
        Err(error) => return error,
    };
    unsafe {
        *accepted.as_ptr() = ring.push(input);
    }
    OK
}

/// Consume one raw signed-16 source sample.
extern "C" fn pop_sample(ring: *mut LegacyRing, output: *mut i16, available: *mut bool) -> c_int {
    let Some(output) = NonNull::new(output) else {
        return INVALID_ARGUMENT;
    };
    let Some(available) = NonNull::new(available) else {
        return INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let sample = ring.pop_sample();
    unsafe {
        *available.as_ptr() = sample.is_some();
        if let Some(sample) = sample {
            *output.as_ptr() = sample;
        }
    }
    OK
}

/// Render one signed-16 hardware-paced output sample.
extern "C" fn render_sample(
    ring: *mut LegacyRing,
    output: *mut i16,
    target_samples: u64,
    real: *mut bool,
    observation: *mut LegacyObservation,
) -> c_int {
    let Some(output) = NonNull::new(output) else {
        return INVALID_ARGUMENT;
    };
    let Some(real) = NonNull::new(real) else {
        return INVALID_ARGUMENT;
    };
    let Some(observation) = NonNull::new(observation) else {
        return INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let (sample, is_real, _) = ring.render_sample(target_samples);
    unsafe {
        *output.as_ptr() = sample;
        *real.as_ptr() = is_real;
        *observation.as_ptr() = ring.observe();
    }
    // ABI major one reports only whether output was real. Rust has already
    // produced bounded concealment for an adapter fault, so preserve that PCM
    // and expose the fault only through the shared observation counters.
    OK
}

/// Render one arbitrary signed-16 output block.
extern "C" fn render(
    ring: *mut LegacyRing,
    output: *mut i16,
    samples: u64,
    reserve_samples: u64,
    target_samples: u64,
    real_samples: *mut u64,
) -> c_int {
    let Some(real_samples) = NonNull::new(real_samples) else {
        return INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let output = match unsafe { checked_output(output, samples) } {
        Ok(output) => output,
        Err(error) => return error,
    };
    let (rendered, _) = ring.render(output, reserve_samples, target_samples);
    unsafe {
        *real_samples.as_ptr() = rendered;
    }
    OK
}

/// Fill an exceptional oversized ABI-major-one request with PLC only.
extern "C" fn conceal(ring: *mut LegacyRing, output: *mut i16, samples: u64) -> c_int {
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let output = match unsafe { checked_output(output, samples) } {
        Ok(output) => output,
        Err(error) => return error,
    };
    ring.conceal(output);
    OK
}

/// Copy compatibility observations without stopping producer or consumer.
extern "C" fn observe(ring: *const LegacyRing, observation: *mut LegacyObservation) -> c_int {
    let ring = match unsafe { checked_ring(ring.cast_mut()) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let Some(observation) = NonNull::new(observation) else {
        return INVALID_ARGUMENT;
    };
    unsafe {
        *observation.as_ptr() = ring.observe();
    }
    OK
}

/// Account for a legacy callback's explicitly observed shortfall.
extern "C" fn record_shortfall(
    ring: *mut LegacyRing,
    missing: u64,
    samples: u64,
    rate_hz: u32,
) -> c_int {
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    ring.statistics.record(missing, samples, rate_hz);
    OK
}

/// Immutable compatibility table resolved once by the C ABI-major-one shim.
static DESCRIPTOR: LegacyDescriptor = LegacyDescriptor {
    struct_size: size_of::<LegacyDescriptor>() as u32,
    abi_version: ABI_VERSION,
    capability_name: CAPABILITY.as_ptr().cast::<c_char>(),
    create,
    destroy,
    reconfigure,
    push_sample,
    push,
    pop_sample,
    render_sample,
    render,
    conceal,
    observe,
    record_shortfall,
};

/// Return the immutable private bridge used by ABI-major-one C forwarding.
#[unsafe(no_mangle)]
pub extern "C" fn rpcr1_bridge_descriptor() -> *const LegacyDescriptor {
    &DESCRIPTOR
}

#[cfg(test)]
mod tests;
