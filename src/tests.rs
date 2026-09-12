//! Unit coverage for the Rust ring using a deterministic in-process SRC fake.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::size_of;
use core::ptr;
use std::alloc::Layout;

use crate::ring::{
    CreateError, MAXIMUM_CAPACITY, Quality, Ring, allocate_samples, attenuate_concealment,
};
use crate::samplerate_adapter::{AdapterDescriptor, AdapterFunctions, load_functions};
use crate::{
    ABI_VERSION, CObservation, Config, Descriptor, RESULT_ADAPTER_ERROR, RESULT_INVALID_ARGUMENT,
    RESULT_NO_MEMORY, RESULT_OK, Rpcr2Ring, finish_ring_create, ring_create_with_adapter,
    ring_create_with_adapter_and_allocator, ring_destroy, rpcr2_descriptor,
};

const OK: c_int = 0;
const ERROR: c_int = -1;
const CAPABILITY_NAME: &[u8] = b"rptadv.samplerate\0";
const WRONG_CAPABILITY_NAME: &[u8] = b"test.samplerate-adapter\0";

struct FakeConverter;

unsafe extern "C" fn fake_create(quality: c_int, channels: u32, output: *mut *mut c_void) -> c_int {
    if quality > 2 || channels != 1 || output.is_null() {
        return ERROR;
    }
    let converter = Box::new(FakeConverter);
    unsafe {
        *output = Box::into_raw(converter).cast::<c_void>();
    }
    OK
}

unsafe extern "C" fn fake_reset(converter: *mut c_void) -> c_int {
    if converter.is_null() { ERROR } else { OK }
}

unsafe extern "C" fn fake_process(
    converter: *mut c_void,
    input: *const f32,
    input_frames: u32,
    output: *mut f32,
    output_capacity: u32,
    ratio: f64,
    input_used: *mut u32,
    output_generated: *mut u32,
) -> c_int {
    if converter.is_null()
        || input.is_null()
        || output.is_null()
        || input_used.is_null()
        || output_generated.is_null()
        || !ratio.is_finite()
        || ratio <= 0.0
    {
        return ERROR;
    }
    let count = input_frames.min(output_capacity) as usize;
    let input = unsafe { core::slice::from_raw_parts(input, count) };
    let output = unsafe { core::slice::from_raw_parts_mut(output, count) };
    output.copy_from_slice(input);
    unsafe {
        *input_used = count as u32;
        *output_generated = count as u32;
    }
    OK
}

unsafe extern "C" fn fake_destroy(converter: *mut c_void) {
    if !converter.is_null() {
        unsafe {
            drop(Box::from_raw(converter.cast::<FakeConverter>()));
        }
    }
}

pub(crate) static FAKE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
};

unsafe extern "C" fn failing_process(
    _converter: *mut c_void,
    _input: *const f32,
    _input_frames: u32,
    _output: *mut f32,
    _output_capacity: u32,
    _ratio: f64,
    _input_used: *mut u32,
    _output_generated: *mut u32,
) -> c_int {
    ERROR
}

unsafe extern "C" fn zero_progress_process(
    converter: *mut c_void,
    input: *const f32,
    _input_frames: u32,
    output: *mut f32,
    _output_capacity: u32,
    ratio: f64,
    input_used: *mut u32,
    output_generated: *mut u32,
) -> c_int {
    if converter.is_null()
        || input.is_null()
        || output.is_null()
        || input_used.is_null()
        || output_generated.is_null()
        || !ratio.is_finite()
    {
        return ERROR;
    }
    unsafe {
        *input_used = 0;
        *output_generated = 0;
    }
    OK
}

unsafe extern "C" fn consume_one_zero_output_process(
    converter: *mut c_void,
    input: *const f32,
    input_frames: u32,
    output: *mut f32,
    _output_capacity: u32,
    ratio: f64,
    input_used: *mut u32,
    output_generated: *mut u32,
) -> c_int {
    if converter.is_null()
        || input.is_null()
        || input_frames == 0
        || output.is_null()
        || input_used.is_null()
        || output_generated.is_null()
        || !ratio.is_finite()
        || ratio <= 0.0
    {
        return ERROR;
    }
    unsafe {
        *input_used = 1;
        *output_generated = 0;
    }
    OK
}

pub(crate) static FAILING_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(failing_process),
    converter_destroy: Some(fake_destroy),
};

pub(crate) static ZERO_PROGRESS_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(zero_progress_process),
    converter_destroy: Some(fake_destroy),
};

static CONSUME_ONE_ZERO_OUTPUT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(consume_one_zero_output_process),
    converter_destroy: Some(fake_destroy),
};

static WRONG_CAPABILITY_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: WRONG_CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
};

unsafe extern "C" fn create_fails(
    _quality: c_int,
    _channels: u32,
    _output: *mut *mut c_void,
) -> c_int {
    ERROR
}

unsafe extern "C" fn create_without_handle(
    _quality: c_int,
    _channels: u32,
    _output: *mut *mut c_void,
) -> c_int {
    OK
}

unsafe extern "C" fn reset_fails(_converter: *mut c_void) -> c_int {
    ERROR
}

unsafe extern "C" fn invalid_count_process(
    converter: *mut c_void,
    _input: *const f32,
    input_frames: u32,
    _output: *mut f32,
    _output_capacity: u32,
    _ratio: f64,
    input_used: *mut u32,
    output_generated: *mut u32,
) -> c_int {
    if converter.is_null() || input_used.is_null() || output_generated.is_null() {
        return ERROR;
    }
    unsafe {
        *input_used = input_frames.saturating_add(1);
        *output_generated = 0;
    }
    OK
}

unsafe extern "C" fn invalid_output_count_process(
    converter: *mut c_void,
    _input: *const f32,
    _input_frames: u32,
    _output: *mut f32,
    output_capacity: u32,
    _ratio: f64,
    input_used: *mut u32,
    output_generated: *mut u32,
) -> c_int {
    if converter.is_null() || input_used.is_null() || output_generated.is_null() {
        return ERROR;
    }
    unsafe {
        *input_used = 0;
        *output_generated = output_capacity.saturating_add(1);
    }
    OK
}

pub(crate) static CREATE_FAILURE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(create_fails),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
};

static NULL_HANDLE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(create_without_handle),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
};

static RESET_FAILURE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(reset_fails),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
};

static INVALID_COUNT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(invalid_count_process),
    converter_destroy: Some(fake_destroy),
};

static INVALID_OUTPUT_COUNT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(invalid_output_count_process),
    converter_destroy: Some(fake_destroy),
};

pub(crate) fn adapter(descriptor: &AdapterDescriptor) -> AdapterFunctions {
    unsafe { AdapterFunctions::from_descriptor(descriptor) }.expect("valid test adapter")
}

fn ring(capacity: usize) -> Ring {
    Ring::create(
        capacity,
        8_000,
        8_000,
        Quality::Best,
        adapter(&FAKE_DESCRIPTOR),
    )
    .expect("test ring allocation")
}

fn valid_config() -> Config {
    Config {
        struct_size: size_of::<Config>() as u32,
        abi_version: ABI_VERSION,
        capacity_samples: 512,
        input_rate_hz: 8_000,
        output_rate_hz: 8_000,
        quality: 0,
    }
}

fn descriptor() -> &'static Descriptor {
    let descriptor = rpcr2_descriptor();
    assert!(!descriptor.is_null());
    unsafe { &*descriptor }
}

struct TestHandle(*mut Rpcr2Ring);

impl TestHandle {
    fn as_ptr(&self) -> *mut Rpcr2Ring {
        self.0
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        ring_destroy(self.0);
    }
}

fn fake_handle() -> TestHandle {
    handle_with(adapter(&FAKE_DESCRIPTOR))
}

fn handle_with(adapter_functions: AdapterFunctions) -> TestHandle {
    handle_with_config(valid_config(), adapter_functions)
}

fn handle_with_config(config: Config, adapter_functions: AdapterFunctions) -> TestHandle {
    let mut handle = ptr::null_mut();
    assert_eq!(
        ring_create_with_adapter(&config, &mut handle, Ok(adapter_functions)),
        RESULT_OK
    );
    assert!(!handle.is_null());
    TestHandle(handle)
}

unsafe fn null_allocator(_layout: Layout) -> *mut u8 {
    ptr::null_mut()
}

#[test]
fn quality_accepts_only_published_values() {
    assert_eq!(Quality::from_ffi(0), Ok(Quality::Best));
    assert_eq!(Quality::from_ffi(1), Ok(Quality::Medium));
    assert_eq!(Quality::from_ffi(2), Ok(Quality::Fastest));
    assert_eq!(Quality::from_ffi(3), Err(()));
}

#[test]
fn producer_and_renderer_preserve_canonical_f32() {
    let ring = ring(512);
    assert!(ring.producer_push_sample(2.0));
    assert!(ring.producer_push_sample(f32::NAN));
    assert!(ring.producer_push_sample(-0.25));
    let mut output = [0.0; 3];
    assert_eq!(ring.consumer_render(&mut output, 0, 0).0, 3);
    assert_eq!(output, [1.0, 0.0, -0.25]);
}

#[test]
fn full_producer_rejects_only_new_samples() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&vec![0.1; 512]), 512);
    assert_eq!(ring.producer_push(&[0.2, 0.3]), 0);
    let (_, real, adapter_error) = ring.consumer_render_sample(0);
    assert!(real);
    assert!(!adapter_error);
    assert_eq!(ring.producer_push(&[0.4]), 1);
    let observation = ring.observe();
    assert_eq!(observation.discarded_samples, 2);
    assert!(observation.available_samples < 512);
}

#[test]
fn rendering_does_not_gate_startup_on_reserve() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&[0.1, 0.2, 0.3, 0.4]), 4);
    let mut output = [0.0; 4];
    assert_eq!(ring.consumer_render(&mut output, 12, 8).0, 4);
    assert_eq!(output, [0.1, 0.2, 0.3, 0.4]);
    let observation = ring.observe();
    assert_eq!(observation.reserve_samples, 12);
    assert_eq!(observation.target_samples, 8);
    assert_eq!(observation.missing_samples, 0);
}

#[test]
fn clock_controller_changes_ratio_only_after_an_observation_update() {
    let ring = ring(512);
    let input = [0.1; 16];
    assert_eq!(ring.producer_push(&input), 16);
    let mut first = [0.0; 8];
    assert_eq!(ring.consumer_render(&mut first, 0, 1).0, 8);
    assert_eq!(ring.producer_push(&input), 16);
    let mut second = [0.0; 16];
    assert_eq!(ring.consumer_render(&mut second, 0, 1).0, 16);
    assert!(ring.observe().ratio_correction_ppm < 0);
}

#[test]
fn renderer_conceals_shortfall_once_and_recovers() {
    let ring = ring(512);
    let input: Vec<f32> = (0..512)
        .map(|index| ((index as f32 * 0.11).sin() * 0.7).clamp(-1.0, 1.0))
        .collect();
    assert_eq!(ring.producer_push(&input), 512);
    let mut real = vec![0.0; 512];
    assert_eq!(ring.consumer_render(&mut real, 0, 32).0, 512);
    let (concealed, is_real, adapter_error) = ring.consumer_render_sample(32);
    assert!(!is_real);
    assert!(!adapter_error);
    assert_ne!(concealed, 0.0);
    let observation = ring.observe();
    assert_eq!(observation.missing_samples, 1);
    assert_eq!(observation.consecutive_shortfall_samples, 1);

    assert_eq!(ring.producer_push(&[0.6, 0.5]), 2);
    let (_, recovered, adapter_error) = ring.consumer_render_sample(32);
    assert!(recovered);
    assert!(!adapter_error);
    assert_eq!(ring.observe().consecutive_shortfall_samples, 0);
}

#[test]
fn construction_rejects_invalid_parameters() {
    let first_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(0, 8_000, 8_000, Quality::Best, first_adapter).is_err());
    let second_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(512, 0, 8_000, Quality::Best, second_adapter).is_err());
    let third_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(512, 8_000, 0, Quality::Best, third_adapter).is_err());
}

#[test]
fn construction_handles_control_plane_allocation_failure_without_panicking() {
    let too_large = Ring::create(
        usize::MAX,
        8_000,
        8_000,
        Quality::Best,
        adapter(&FAKE_DESCRIPTOR),
    );
    assert!(matches!(too_large, Err(CreateError::NoMemory)));
    assert!(allocate_samples(usize::MAX).is_err());

    let ring = ring(512);
    assert!(crate::allocate_ring_handle_with(ring, null_allocator).is_err());
}

#[test]
fn every_published_converter_quality_constructs_a_ring() {
    let medium = Ring::create(
        512,
        8_000,
        8_000,
        Quality::Medium,
        adapter(&FAKE_DESCRIPTOR),
    );
    assert!(medium.is_ok());
    let fastest = Ring::create(
        512,
        8_000,
        8_000,
        Quality::Fastest,
        adapter(&FAKE_DESCRIPTOR),
    );
    assert!(fastest.is_ok());
}

#[test]
fn descriptor_rejects_an_unselected_adapter_capability() {
    assert!(unsafe { AdapterFunctions::from_descriptor(&WRONG_CAPABILITY_DESCRIPTOR) }.is_err());
}

#[test]
fn converter_failure_is_visible_without_skipping_concealment() {
    let ring = Ring::create(
        512,
        8_000,
        8_000,
        Quality::Best,
        adapter(&FAILING_DESCRIPTOR),
    )
    .expect("test ring allocation");
    assert_eq!(ring.producer_push(&[0.4]), 1);
    let (_, real, adapter_error) = ring.consumer_render_sample(4);
    assert!(!real);
    assert!(adapter_error);
    assert_eq!(ring.observe().adapter_error_count, 1);
    assert_eq!(ring.observe().missing_samples, 1);
}

#[test]
fn zero_progress_conversion_gathers_more_then_reports_a_fault() {
    let ring = Ring::create(
        512,
        8_000,
        8_000,
        Quality::Best,
        adapter(&ZERO_PROGRESS_DESCRIPTOR),
    )
    .expect("test ring allocation");
    assert_eq!(ring.producer_push(&vec![0.2; 512]), 512);
    let (_, first_real, first_error) = ring.consumer_render_sample(8);
    assert!(!first_real);
    assert!(!first_error);
    let (_, second_real, second_error) = ring.consumer_render_sample(8);
    assert!(!second_real);
    assert!(second_error);
    assert_eq!(ring.observe().adapter_error_count, 1);
}

#[test]
fn controller_clamps_the_adapter_ratio_at_its_documented_boundary() {
    let ring = Ring::create(512, 1, 256, Quality::Best, adapter(&FAKE_DESCRIPTOR))
        .expect("boundary-rate ring allocation");
    assert_eq!(ring.producer_push(&vec![0.1; 512]), 512);
    let mut output = [0.0; 2];
    assert_eq!(ring.consumer_render(&mut output, 0, 1).0, 2);
    assert_eq!(ring.observe().adapter_error_count, 0);
    assert!(ring.observe().ratio_correction_ppm <= 0);
}

#[test]
fn partial_block_acceptance_preserves_the_chronological_prefix() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&vec![0.1; 510]), 510);
    assert_eq!(ring.producer_push(&[0.2, 0.3, 0.4, 0.5]), 2);
    let mut output = vec![0.0; 512];
    assert_eq!(ring.consumer_render(&mut output, 0, 0).0, 512);
    assert!(output[..510].iter().all(|sample| *sample == 0.1));
    assert_eq!(&output[510..], &[0.2, 0.3]);
    assert_eq!(ring.observe().discarded_samples, 2);
}

#[test]
fn refill_compacts_pending_input_after_partial_zero_output_conversion() {
    let ring = Ring::create(
        512,
        8_000,
        8_000,
        Quality::Best,
        adapter(&CONSUME_ONE_ZERO_OUTPUT_DESCRIPTOR),
    )
    .expect("test ring allocation");
    assert_eq!(ring.producer_push(&vec![0.2; 512]), 512);
    for _ in 0..258 {
        let (_, real, adapter_error) = ring.consumer_render_sample(8);
        assert!(!real);
        assert!(!adapter_error);
    }
    assert_eq!(ring.available(), 0);
    assert_eq!(ring.observe().adapter_error_count, 0);
}

#[test]
fn controller_clamps_at_both_documented_ratio_limits() {
    let lower = Ring::create(512, 256, 1, Quality::Best, adapter(&FAKE_DESCRIPTOR))
        .expect("lower-bound ring allocation");
    assert_eq!(lower.producer_push(&vec![0.2; 512]), 512);
    let mut lower_first = vec![0.0; 256];
    assert_eq!(lower.consumer_render(&mut lower_first, 0, 1).0, 256);
    assert_eq!(lower.producer_push(&vec![0.2; 256]), 256);
    let (_, lower_real, lower_error) = lower.consumer_render_sample(1);
    assert!(lower_real);
    assert!(!lower_error);
    assert_eq!(lower.observe().ratio_correction_ppm, 0);

    let upper = Ring::create(512, 1, 256, Quality::Best, adapter(&FAKE_DESCRIPTOR))
        .expect("upper-bound ring allocation");
    assert_eq!(upper.producer_push(&vec![0.2; 512]), 512);
    let mut upper_first = vec![0.0; 256];
    assert_eq!(upper.consumer_render(&mut upper_first, 0, u64::MAX).0, 256);
    assert_eq!(upper.producer_push(&vec![0.2; 256]), 256);
    let (_, upper_real, upper_error) = upper.consumer_render_sample(u64::MAX);
    assert!(upper_real);
    assert!(!upper_error);
    assert_eq!(upper.observe().ratio_correction_ppm, 0);
}

#[test]
fn concealment_fades_and_recovers_at_its_sample_boundaries() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&vec![0.6; 512]), 512);
    let mut source = vec![0.0; 512];
    assert_eq!(ring.consumer_render(&mut source, 0, 32).0, 512);

    let mut concealed = Vec::new();
    for _ in 0..481 {
        let (sample, real, adapter_error) = ring.consumer_render_sample(32);
        assert!(!real);
        assert!(!adapter_error);
        concealed.push(sample);
    }
    assert_ne!(concealed[0], 0.0);
    assert_ne!(concealed[80], 0.0);
    assert_ne!(concealed[479], 0.0);
    assert_eq!(concealed[480], 0.0);

    assert_eq!(ring.producer_push(&vec![0.4; 512]), 512);
    for _ in 0..64 {
        let (_, real, adapter_error) = ring.consumer_render_sample(32);
        assert!(real);
        assert!(!adapter_error);
    }
    let (recovered, real, adapter_error) = ring.consumer_render_sample(32);
    assert!(real);
    assert!(!adapter_error);
    assert_eq!(recovered, 0.4);
}

#[test]
fn recovery_uses_the_frozen_concealment_waveform_phase() {
    let ring = ring(512);
    let source: Vec<f32> = (0..512)
        .map(|index| (core::f32::consts::TAU * index as f32 / 80.0).sin())
        .collect();
    assert_eq!(ring.producer_push(&source), source.len() as u64);
    let mut played = vec![0.0; source.len()];
    assert_eq!(
        ring.consumer_render(&mut played, 0, 0).0,
        source.len() as u64
    );

    for _ in 0..2 {
        let (_, real, adapter_error) = ring.consumer_render_sample(0);
        assert!(!real);
        assert!(!adapter_error);
    }
    assert_eq!(ring.producer_push(&[0.0, 0.0]), 2);

    let crossfade = 64.0_f32;
    let (first, first_real, first_error) = ring.consumer_render_sample(0);
    assert!(first_real);
    assert!(!first_error);
    let expected_first = ((1.0 - 1.0 / (crossfade + 1.0)).sqrt()) * source[434];
    assert!((first - expected_first).abs() < 0.000_001);

    let (second, second_real, second_error) = ring.consumer_render_sample(0);
    assert!(second_real);
    assert!(!second_error);
    let expected_second = ((1.0 - 2.0 / (crossfade + 1.0)).sqrt()) * source[435];
    assert!((second - expected_second).abs() < 0.000_001);
}

#[test]
fn low_rate_plc_uses_safe_pitch_and_zero_crossfade_fallbacks() {
    let ring = Ring::create(512, 1, 1, Quality::Best, adapter(&FAKE_DESCRIPTOR))
        .expect("low-rate ring allocation");
    assert_eq!(ring.producer_push(&vec![0.6; 512]), 512);
    let mut source = vec![0.0; 512];
    assert_eq!(ring.consumer_render(&mut source, 0, 0).0, 512);

    let (concealed, real, adapter_error) = ring.consumer_render_sample(0);
    assert_eq!(concealed, 0.0);
    assert!(!real);
    assert!(!adapter_error);

    assert!(ring.producer_push_sample(0.4));
    let (recovered, real, adapter_error) = ring.consumer_render_sample(0);
    assert_eq!(recovered, 0.4);
    assert!(real);
    assert!(!adapter_error);

    let high_rate = Ring::create(
        512,
        48_000,
        48_000,
        Quality::Best,
        adapter(&FAKE_DESCRIPTOR),
    )
    .expect("high-rate ring allocation");
    let (_, high_rate_real, high_rate_error) = high_rate.consumer_render_sample(0);
    assert!(!high_rate_real);
    assert!(!high_rate_error);

    assert_eq!(attenuate_concealment(0.5, 1, 1), 0.5);
    assert_eq!(attenuate_concealment(0.5, 0, 8_000), 0.5);
}

#[test]
fn adapter_validation_rejects_each_required_descriptor_component() {
    assert!(unsafe { AdapterFunctions::from_descriptor(ptr::null()) }.is_err());

    let undersized = AdapterDescriptor {
        struct_size: 0,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&undersized) }.is_err());

    let incompatible_abi = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&incompatible_abi) }.is_err());

    let missing_capability = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: ptr::null(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_capability) }.is_err());

    let missing_create = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: None,
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_create) }.is_err());

    let missing_reset = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: None,
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_reset) }.is_err());

    let missing_process = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: None,
        converter_destroy: Some(fake_destroy),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_process) }.is_err());

    let missing_destroy = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: None,
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_destroy) }.is_err());
}

#[test]
fn adapter_converter_lifecycle_validates_callback_results() {
    let mut converter = adapter(&FAKE_DESCRIPTOR)
        .create_converter(0)
        .expect("fake converter creation");
    let mut output = [0.0; 2];
    assert_eq!(
        converter.process(&[0.1, -0.2], &mut output, 1.0),
        Ok((2, 2))
    );
    assert_eq!(output, [0.1, -0.2]);
    assert!(converter.reset().is_ok());
    assert!(converter.process(&[], &mut output, 1.0).is_err());
    assert!(converter.process(&[0.1], &mut [], 1.0).is_err());
    assert!(converter.process(&[0.1], &mut output, 0.0).is_err());
    assert!(converter.process(&[0.1], &mut output, f64::NAN).is_err());
    assert!(
        adapter(&CREATE_FAILURE_DESCRIPTOR)
            .create_converter(0)
            .is_err()
    );
    assert!(
        adapter(&NULL_HANDLE_DESCRIPTOR)
            .create_converter(0)
            .is_err()
    );
    assert!(
        adapter(&RESET_FAILURE_DESCRIPTOR)
            .create_converter(0)
            .is_err()
    );

    let mut invalid_counts = adapter(&INVALID_COUNT_DESCRIPTOR)
        .create_converter(0)
        .expect("invalid-count converter creation");
    assert!(invalid_counts.process(&[0.1], &mut output, 1.0).is_err());

    let mut invalid_output_counts = adapter(&INVALID_OUTPUT_COUNT_DESCRIPTOR)
        .create_converter(0)
        .expect("invalid-output-count converter creation");
    assert!(
        invalid_output_counts
            .process(&[0.1], &mut output, 1.0)
            .is_err()
    );
}

#[test]
fn dynamic_adapter_descriptor_is_usable_at_ring_construction_time() {
    let functions = load_functions().expect("installed dynamic adapter descriptor");
    let mut converter = functions
        .create_converter(0)
        .expect("installed dynamic converter creation");
    let input = [0.1; 512];
    let mut output = [0.0; 512];
    let (used, generated) = converter
        .process(&input, &mut output, 1.0)
        .expect("installed dynamic converter processing");
    assert!(used <= input.len());
    assert!(generated <= output.len());
}

#[test]
fn public_descriptor_exposes_the_complete_abi_v2_function_table() {
    let descriptor = descriptor();
    assert_eq!(descriptor.struct_size, size_of::<Descriptor>() as u32);
    assert_eq!(descriptor.abi_version, ABI_VERSION);
    assert!(!descriptor.capability_name.is_null());
    let capability = unsafe { CStr::from_ptr(descriptor.capability_name) };
    assert_eq!(
        capability.to_bytes_with_nul(),
        b"rptadv.rate-adjusting-pcm-ring.f32\0"
    );
    assert!(descriptor.ring_create as usize != 0);
    assert!(descriptor.ring_destroy as usize != 0);
    assert!(descriptor.ring_producer_push_sample as usize != 0);
    assert!(descriptor.ring_producer_push as usize != 0);
    assert!(descriptor.ring_consumer_render_sample as usize != 0);
    assert!(descriptor.ring_consumer_render as usize != 0);
    assert!(descriptor.ring_observe as usize != 0);
}

#[test]
fn public_descriptor_constructs_with_the_installed_dynamic_adapter() {
    let descriptor = descriptor();
    let config = valid_config();
    let mut handle = ptr::null_mut();
    assert_eq!((descriptor.ring_create)(&config, &mut handle), RESULT_OK);
    assert!(!handle.is_null());
    (descriptor.ring_destroy)(handle);
    (descriptor.ring_destroy)(ptr::null_mut());
}

#[test]
fn ring_create_classifies_invalid_configurations_and_adapter_failures() {
    let config = valid_config();
    let mut output = ptr::NonNull::<Rpcr2Ring>::dangling().as_ptr();
    assert_eq!(
        ring_create_with_adapter(ptr::null(), &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    assert!(output.is_null());
    assert_eq!(
        ring_create_with_adapter(&config, ptr::null_mut(), Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );

    let mut invalid = valid_config();
    invalid.struct_size = 0;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.abi_version = ABI_VERSION + 1;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.capacity_samples = 511;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.capacity_samples = MAXIMUM_CAPACITY as u64 + 1;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.input_rate_hz = 0;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.output_rate_hz = 0;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.quality = 3;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid = valid_config();
    invalid.output_rate_hz = 257;
    invalid.input_rate_hz = 1;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    invalid.output_rate_hz = 1;
    invalid.input_rate_hz = 257;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );

    assert_eq!(
        finish_ring_create(
            ptr::NonNull::from(&mut output),
            Err(CreateError::NoMemory),
            std::alloc::alloc,
        ),
        RESULT_NO_MEMORY
    );
    assert!(output.is_null());

    invalid = valid_config();
    invalid.capacity_samples = MAXIMUM_CAPACITY as u64 + 1;
    assert_eq!(
        ring_create_with_adapter(&invalid, &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    assert!(output.is_null());

    assert_eq!(
        ring_create_with_adapter_and_allocator(
            &config,
            &mut output,
            Ok(adapter(&FAKE_DESCRIPTOR)),
            null_allocator
        ),
        RESULT_NO_MEMORY
    );
    assert!(output.is_null());

    assert_eq!(
        ring_create_with_adapter(&config, &mut output, Err(())),
        RESULT_ADAPTER_ERROR
    );
    assert!(output.is_null());
    assert_eq!(
        ring_create_with_adapter(
            &config,
            &mut output,
            Ok(adapter(&CREATE_FAILURE_DESCRIPTOR))
        ),
        RESULT_ADAPTER_ERROR
    );
    assert_eq!(
        ring_create_with_adapter(&config, &mut output, Ok(adapter(&RESET_FAILURE_DESCRIPTOR))),
        RESULT_ADAPTER_ERROR
    );
}

#[test]
fn descriptor_operations_validate_pointers_and_report_complete_results() {
    let descriptor = descriptor();
    let handle = fake_handle();
    let mut accepted = false;
    let mut accepted_count = 0_u64;
    let mut sample = 0.0_f32;
    let mut real = false;
    let mut real_samples = 0_u64;
    let mut observation = CObservation::default();

    assert_eq!(
        (descriptor.ring_producer_push_sample)(ptr::null_mut(), 0.1, &mut accepted),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_producer_push_sample)(handle.as_ptr(), 0.1, ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_producer_push)(handle.as_ptr(), ptr::null(), 1, &mut accepted_count),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_producer_push)(handle.as_ptr(), ptr::null(), 0, ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_producer_push)(ptr::null_mut(), ptr::null(), 0, &mut accepted_count),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_producer_push)(handle.as_ptr(), ptr::null(), 0, &mut accepted_count),
        RESULT_OK
    );
    assert_eq!(accepted_count, 0);
    let mut one = [0.1];
    let too_large = isize::MAX as u64 / size_of::<f32>() as u64 + 1;
    assert_eq!(
        (descriptor.ring_producer_push)(
            handle.as_ptr(),
            one.as_ptr(),
            too_large,
            &mut accepted_count
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), ptr::null_mut(), 0, &mut real),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, 0, ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(ptr::null_mut(), &mut sample, 0, &mut real),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, 0, &mut real),
        RESULT_OK
    );
    assert!(!real);
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            ptr::null_mut(),
            1,
            0,
            0,
            &mut real_samples
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(
            ptr::null_mut(),
            one.as_mut_ptr(),
            1,
            0,
            0,
            &mut real_samples
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            one.as_mut_ptr(),
            too_large,
            0,
            0,
            &mut real_samples
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            ptr::null_mut(),
            0,
            0,
            0,
            &mut real_samples
        ),
        RESULT_OK
    );
    assert_eq!(real_samples, 0);
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            one.as_mut_ptr(),
            1,
            0,
            0,
            ptr::null_mut()
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_observe)(ptr::null(), &mut observation),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_observe)(handle.as_ptr(), ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_observe)(handle.as_ptr(), &mut observation),
        RESULT_INVALID_ARGUMENT
    );
    observation.struct_size = size_of::<CObservation>() as u32;
    assert_eq!(
        (descriptor.ring_observe)(handle.as_ptr(), &mut observation),
        RESULT_OK
    );
    assert_eq!(observation.abi_version, ABI_VERSION);
    assert_eq!(observation.capacity_samples, 512);
}

#[test]
fn descriptor_operations_preserve_f32_and_account_for_renderer_shortfalls() {
    let descriptor = descriptor();
    let handle = fake_handle();
    let mut accepted = false;
    assert_eq!(
        (descriptor.ring_producer_push_sample)(handle.as_ptr(), f32::NAN, &mut accepted),
        RESULT_OK
    );
    assert!(accepted);
    let mut output = [0.0; 1];
    let mut real_samples = 0_u64;
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u64,
            0,
            0,
            &mut real_samples
        ),
        RESULT_OK
    );
    assert_eq!(real_samples, 1);
    assert_eq!(output, [0.0]);

    let input = [2.0, -0.25, 0.5];
    let mut accepted_count = 0_u64;
    assert_eq!(
        (descriptor.ring_producer_push)(
            handle.as_ptr(),
            input.as_ptr(),
            input.len() as u64,
            &mut accepted_count
        ),
        RESULT_OK
    );
    assert_eq!(accepted_count, input.len() as u64);
    let mut output = [0.0; 3];
    real_samples = 0;
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u64,
            12,
            8,
            &mut real_samples
        ),
        RESULT_OK
    );
    assert_eq!(real_samples, output.len() as u64);
    assert_eq!(output, [1.0, -0.25, 0.5]);

    let empty_handle = fake_handle();
    let mut concealed = [1.0; 3];
    real_samples = 1;
    assert_eq!(
        (descriptor.ring_consumer_render)(
            empty_handle.as_ptr(),
            concealed.as_mut_ptr(),
            concealed.len() as u64,
            0,
            0,
            &mut real_samples
        ),
        RESULT_OK
    );
    assert_eq!(real_samples, 0);
    let mut observation = CObservation {
        struct_size: size_of::<CObservation>() as u32,
        ..CObservation::default()
    };
    assert_eq!(
        (descriptor.ring_observe)(empty_handle.as_ptr(), &mut observation),
        RESULT_OK
    );
    assert_eq!(observation.missing_samples, 3);
    assert_eq!(observation.consecutive_shortfall_samples, 3);
    assert!(observation.shortfall_average_milli > 0);
}

#[test]
fn descriptor_reports_adapter_faults_after_safe_concealment() {
    let descriptor = descriptor();
    let handle = handle_with(adapter(&FAILING_DESCRIPTOR));
    let mut accepted = false;
    assert_eq!(
        (descriptor.ring_producer_push_sample)(handle.as_ptr(), 0.4, &mut accepted),
        RESULT_OK
    );
    assert!(accepted);

    let mut sample = 0.0;
    let mut real = true;
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, 8, &mut real),
        RESULT_ADAPTER_ERROR
    );
    assert!(!real);
    assert!(sample.is_finite());

    let mut output = [0.0; 2];
    let mut real_samples = 0_u64;
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u64,
            0,
            8,
            &mut real_samples
        ),
        RESULT_ADAPTER_ERROR
    );
    assert_eq!(real_samples, 0);
    assert!(output.into_iter().all(f32::is_finite));
}
