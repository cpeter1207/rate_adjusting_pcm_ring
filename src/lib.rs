//! Rust implementation of the rate-adjusting PCM ring ABI major two.
//!
//! The exported C descriptor keeps Rust types and allocation details private.
//! PCM crossing this ABI is mono canonical `f32` in the inclusive normalized
//! range `-1.0..=1.0`; hardware and Asterisk conversions remain outside it.

#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_has_atomic = "64"), allow(dead_code))]

#[cfg(not(target_has_atomic = "64"))]
compile_error!("rate_adjusting_pcm_ring2 requires lock-free 64-bit atomics");

mod legacy_bridge;
mod ring;
mod samplerate_adapter;

use core::ffi::{c_char, c_int};
use core::mem::size_of;
use core::ptr::{self, NonNull};
use std::alloc::{Layout, alloc, dealloc};

use ring::{CreateError, MAXIMUM_CAPACITY, MINIMUM_CAPACITY, Observation, Quality, Ring};

/// ABI major implemented by this opaque descriptor.
const ABI_VERSION: u32 = 2;
/// Successful descriptor-operation result.
const RESULT_OK: c_int = 0;
/// Descriptor-operation result for an invalid caller argument.
const RESULT_INVALID_ARGUMENT: c_int = -1;
/// Descriptor-operation result for a control-plane allocation failure.
const RESULT_NO_MEMORY: c_int = -2;
/// Descriptor-operation result for an unavailable or failed adapter.
const RESULT_ADAPTER_ERROR: c_int = -3;
/// Stable capability name published through the immutable descriptor.
const CAPABILITY_NAME: &[u8] = b"rptadv.rate-adjusting-pcm-ring.f32\0";

/// Opaque C handle containing the Rust-owned rate-adjusting ring.
#[repr(C)]
pub struct Rpcr2Ring {
    ring: Ring,
}

/// Immutable construction settings supplied before real-time use begins.
#[repr(C)]
struct Config {
    struct_size: u32,
    abi_version: u32,
    capacity_samples: u64,
    input_rate_hz: u32,
    output_rate_hz: u32,
    quality: u32,
}

/// C-compatible best-effort ring measurement snapshot.
#[repr(C)]
#[derive(Default)]
struct CObservation {
    struct_size: u32,
    abi_version: u32,
    capacity_samples: u64,
    available_samples: u64,
    reserve_samples: u64,
    filtered_occupancy_samples: u64,
    target_samples: u64,
    ratio_correction_ppm: i32,
    reserved: u32,
    discarded_samples: u64,
    missing_samples: u64,
    consecutive_shortfall_samples: u64,
    shortfall_average_milli: u64,
    adapter_error_count: u64,
}

/// C function type that constructs one stopped ring.
type RingCreate = extern "C" fn(*const Config, *mut *mut Rpcr2Ring) -> c_int;
/// C function type that tears down one stopped ring.
type RingDestroy = extern "C" fn(*mut Rpcr2Ring);
/// C function type that writes one producer sample.
type ProducerPushSample = extern "C" fn(*mut Rpcr2Ring, f32, *mut bool) -> c_int;
/// C function type that writes a chronological producer block.
type ProducerPush = extern "C" fn(*mut Rpcr2Ring, *const f32, u64, *mut u64) -> c_int;
/// C function type that renders one converted output sample.
type ConsumerRenderSample = extern "C" fn(*mut Rpcr2Ring, *mut f32, u64, *mut bool) -> c_int;
/// C function type that renders an arbitrary converted output block.
type ConsumerRender = extern "C" fn(*mut Rpcr2Ring, *mut f32, u64, u64, u64, *mut u64) -> c_int;
/// C function type that copies a diagnostic snapshot.
type Observe = extern "C" fn(*const Rpcr2Ring, *mut CObservation) -> c_int;

/// Versioned C function table exported by the shared object.
#[repr(C)]
pub struct Descriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    ring_create: RingCreate,
    ring_destroy: RingDestroy,
    ring_producer_push_sample: ProducerPushSample,
    ring_producer_push: ProducerPush,
    ring_consumer_render_sample: ConsumerRenderSample,
    ring_consumer_render: ConsumerRender,
    ring_observe: Observe,
}

// The descriptor contains immutable data and function pointers valid for the
// loaded shared object's lifetime.
unsafe impl Sync for Descriptor {}

/// Validate a caller-owned config prefix before copying scalar values.
unsafe fn checked_config(config: *const Config) -> Result<(u64, u32, u32, Quality), c_int> {
    let config = unsafe { config.as_ref() }.ok_or(RESULT_INVALID_ARGUMENT)?;
    if config.struct_size < size_of::<Config>() as u32 || config.abi_version != ABI_VERSION {
        return Err(RESULT_INVALID_ARGUMENT);
    }
    if config.capacity_samples < MINIMUM_CAPACITY as u64
        || config.capacity_samples > MAXIMUM_CAPACITY as u64
        || config.input_rate_hz == 0
        || config.output_rate_hz == 0
    {
        return Err(RESULT_INVALID_ARGUMENT);
    }
    let quality = match Quality::from_ffi(config.quality) {
        Ok(quality) => quality,
        Err(()) => return Err(RESULT_INVALID_ARGUMENT),
    };
    let nominal_ratio = f64::from(config.output_rate_hz) / f64::from(config.input_rate_hz);
    if !(1.0 / 256.0..=256.0).contains(&nominal_ratio) {
        return Err(RESULT_INVALID_ARGUMENT);
    }
    Ok((
        config.capacity_samples,
        config.input_rate_hz,
        config.output_rate_hz,
        quality,
    ))
}

/// Convert a nonzero C sample count into one immutable input slice.
unsafe fn checked_input<'a>(input: *const f32, samples: u64) -> Result<&'a [f32], c_int> {
    if samples == 0 {
        return Ok(&[]);
    }
    if samples > isize::MAX as u64 / size_of::<f32>() as u64 {
        return Err(RESULT_INVALID_ARGUMENT);
    }
    let length = samples as usize;
    let input = NonNull::new(input.cast_mut()).ok_or(RESULT_INVALID_ARGUMENT)?;
    Ok(unsafe { core::slice::from_raw_parts(input.as_ptr(), length) })
}

/// Convert a nonzero C sample count into one mutable output slice.
unsafe fn checked_output<'a>(output: *mut f32, samples: u64) -> Result<&'a mut [f32], c_int> {
    if samples == 0 {
        return Ok(&mut []);
    }
    if samples > isize::MAX as u64 / size_of::<f32>() as u64 {
        return Err(RESULT_INVALID_ARGUMENT);
    }
    let length = samples as usize;
    let output = NonNull::new(output).ok_or(RESULT_INVALID_ARGUMENT)?;
    Ok(unsafe { core::slice::from_raw_parts_mut(output.as_ptr(), length) })
}

/// Recover a live opaque ring pointer without taking ownership.
unsafe fn checked_ring<'a>(ring: *mut Rpcr2Ring) -> Result<&'a Rpcr2Ring, c_int> {
    unsafe { ring.as_ref() }.ok_or(RESULT_INVALID_ARGUMENT)
}

/// Allocate the opaque handle through one control-plane allocator callback.
fn allocate_ring_handle_with(
    ring: Ring,
    allocator: unsafe fn(Layout) -> *mut u8,
) -> Result<*mut Rpcr2Ring, ()> {
    let layout = Layout::new::<Rpcr2Ring>();
    let handle = unsafe { allocator(layout).cast::<Rpcr2Ring>() };
    let Some(handle) = NonNull::new(handle) else {
        return Err(());
    };
    unsafe {
        handle.as_ptr().write(Rpcr2Ring { ring });
    }
    Ok(handle.as_ptr())
}

/// Apply a resolved dynamic adapter to a validated public construction request.
fn ring_create_with_adapter(
    config: *const Config,
    output: *mut *mut Rpcr2Ring,
    adapter: Result<samplerate_adapter::AdapterFunctions, ()>,
) -> c_int {
    ring_create_with_adapter_and_allocator(config, output, adapter, alloc)
}

/// Apply a resolved dynamic adapter through one control-plane handle allocator.
fn ring_create_with_adapter_and_allocator(
    config: *const Config,
    output: *mut *mut Rpcr2Ring,
    adapter: Result<samplerate_adapter::AdapterFunctions, ()>,
    allocator: unsafe fn(Layout) -> *mut u8,
) -> c_int {
    let Some(output) = NonNull::new(output) else {
        return RESULT_INVALID_ARGUMENT;
    };
    unsafe {
        *output.as_ptr() = ptr::null_mut();
    }
    let (capacity, input_rate, output_rate, quality) = match unsafe { checked_config(config) } {
        Ok(values) => values,
        Err(error) => return error,
    };
    let adapter = match adapter {
        Ok(adapter) => adapter,
        Err(()) => return RESULT_ADAPTER_ERROR,
    };
    finish_ring_create(
        output,
        Ring::create(capacity as usize, input_rate, output_rate, quality, adapter),
        allocator,
    )
}

/// Translate construction failures and publish a fully initialized opaque handle.
fn finish_ring_create(
    output: NonNull<*mut Rpcr2Ring>,
    ring: Result<Ring, CreateError>,
    allocator: unsafe fn(Layout) -> *mut u8,
) -> c_int {
    let ring = match ring {
        Ok(ring) => ring,
        Err(CreateError::NoMemory) => return RESULT_NO_MEMORY,
        Err(CreateError::Adapter) => return RESULT_ADAPTER_ERROR,
    };
    let ring = match allocate_ring_handle_with(ring, allocator) {
        Ok(ring) => ring,
        Err(()) => return RESULT_NO_MEMORY,
    };
    unsafe {
        *output.as_ptr() = ring;
    }
    RESULT_OK
}

/// Construct a ring and its required dynamic converter before audio starts.
extern "C" fn ring_create(config: *const Config, output: *mut *mut Rpcr2Ring) -> c_int {
    ring_create_with_adapter(config, output, samplerate_adapter::load_functions())
}

/// Destroy a stopped ring and all control-plane allocations it owns.
extern "C" fn ring_destroy(ring: *mut Rpcr2Ring) {
    let Some(ring) = NonNull::new(ring) else {
        return;
    };
    unsafe {
        ptr::drop_in_place(ring.as_ptr());
        dealloc(ring.as_ptr().cast::<u8>(), Layout::new::<Rpcr2Ring>());
    }
}

/// Publish one producer sample and report whether unread storage accepted it.
extern "C" fn ring_producer_push_sample(
    ring: *mut Rpcr2Ring,
    sample: f32,
    accepted: *mut bool,
) -> c_int {
    let Some(accepted) = NonNull::new(accepted) else {
        return RESULT_INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    unsafe {
        *accepted.as_ptr() = ring.ring.producer_push_sample(sample);
    }
    RESULT_OK
}

/// Publish a producer block and report the chronological prefix accepted.
extern "C" fn ring_producer_push(
    ring: *mut Rpcr2Ring,
    input: *const f32,
    samples: u64,
    accepted: *mut u64,
) -> c_int {
    let Some(accepted) = NonNull::new(accepted) else {
        return RESULT_INVALID_ARGUMENT;
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
        *accepted.as_ptr() = ring.ring.producer_push(input);
    }
    RESULT_OK
}

/// Render one output sample and report whether it came from converted source PCM.
extern "C" fn ring_consumer_render_sample(
    ring: *mut Rpcr2Ring,
    sample: *mut f32,
    target_samples: u64,
    real: *mut bool,
) -> c_int {
    let Some(sample) = NonNull::new(sample) else {
        return RESULT_INVALID_ARGUMENT;
    };
    let Some(real) = NonNull::new(real) else {
        return RESULT_INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let (rendered, is_real, adapter_error) = ring.ring.consumer_render_sample(target_samples);
    unsafe {
        *sample.as_ptr() = rendered;
        *real.as_ptr() = is_real;
    }
    if adapter_error {
        RESULT_ADAPTER_ERROR
    } else {
        RESULT_OK
    }
}

/// Render a variable-size output block without a fixed callback-duration assumption.
extern "C" fn ring_consumer_render(
    ring: *mut Rpcr2Ring,
    output: *mut f32,
    samples: u64,
    reserve_samples: u64,
    target_samples: u64,
    real_samples: *mut u64,
) -> c_int {
    let Some(real_samples) = NonNull::new(real_samples) else {
        return RESULT_INVALID_ARGUMENT;
    };
    let ring = match unsafe { checked_ring(ring) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let output = match unsafe { checked_output(output, samples) } {
        Ok(output) => output,
        Err(error) => return error,
    };
    unsafe {
        let (rendered, adapter_error) =
            ring.ring
                .consumer_render(output, reserve_samples, target_samples);
        *real_samples.as_ptr() = rendered;
        if adapter_error {
            return RESULT_ADAPTER_ERROR;
        }
    }
    RESULT_OK
}

/// Copy a lock-free best-effort diagnostic snapshot.
extern "C" fn ring_observe(ring: *const Rpcr2Ring, observation: *mut CObservation) -> c_int {
    let ring = match unsafe { checked_ring(ring.cast_mut()) } {
        Ok(ring) => ring,
        Err(error) => return error,
    };
    let Some(mut observation) = NonNull::new(observation) else {
        return RESULT_INVALID_ARGUMENT;
    };
    let observation = unsafe { observation.as_mut() };
    if observation.struct_size < size_of::<CObservation>() as u32 {
        return RESULT_INVALID_ARGUMENT;
    }
    let values: Observation = ring.ring.observe();
    *observation = CObservation {
        struct_size: size_of::<CObservation>() as u32,
        abi_version: ABI_VERSION,
        capacity_samples: values.capacity_samples,
        available_samples: values.available_samples,
        reserve_samples: values.reserve_samples,
        filtered_occupancy_samples: values.filtered_occupancy_samples,
        target_samples: values.target_samples,
        ratio_correction_ppm: values.ratio_correction_ppm,
        reserved: 0,
        discarded_samples: values.discarded_samples,
        missing_samples: values.missing_samples,
        consecutive_shortfall_samples: values.consecutive_shortfall_samples,
        shortfall_average_milli: values.shortfall_average_milli,
        adapter_error_count: values.adapter_error_count,
    };
    RESULT_OK
}

/// Process-lifetime immutable public function table.
static DESCRIPTOR: Descriptor = Descriptor {
    struct_size: size_of::<Descriptor>() as u32,
    abi_version: ABI_VERSION,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    ring_create,
    ring_destroy,
    ring_producer_push_sample,
    ring_producer_push,
    ring_consumer_render_sample,
    ring_consumer_render,
    ring_observe,
};

/// Return the immutable ABI-major-2 descriptor for the Rust ring.
#[unsafe(no_mangle)]
pub extern "C" fn rpcr2_descriptor() -> *const Descriptor {
    &DESCRIPTOR
}

#[cfg(test)]
mod tests;
