//! Unit coverage for the frozen ABI-major-one Rust compatibility bridge.

use core::ffi::{c_char, c_int, c_void};
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{
    ABI_VERSION, ADAPTER_ERROR, CAPABILITY, CreateError, INVALID_ARGUMENT, LegacyDescriptor,
    LegacyObservation, LegacyRing, LegacyStatistics, NO_MEMORY, OK, conceal, create,
    create_error_result, create_with_adapter, destroy, observe, pop_sample, push, push_sample,
    quality_from_ffi, reconfigure, record_shortfall, render, render_sample, saturating_add,
};
use crate::ring::Quality;
use crate::samplerate_adapter::AdapterDescriptor;
use crate::tests::{CREATE_FAILURE_DESCRIPTOR, FAILING_DESCRIPTOR, FAKE_DESCRIPTOR, adapter};

const TEST_ADAPTER_CAPABILITY: &[u8] = b"rptadv.samplerate\0";
const TEST_OK: c_int = 0;
const TEST_ERROR: c_int = -1;

struct ResetTestConverter;

static RESET_FAILS_AFTER_CREATION: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn reset_test_create(
    quality: c_int,
    channels: u32,
    output: *mut *mut c_void,
) -> c_int {
    if quality > 2 || channels != 1 || output.is_null() {
        return TEST_ERROR;
    }
    let converter = Box::new(ResetTestConverter);
    unsafe {
        *output = Box::into_raw(converter).cast::<c_void>();
    }
    TEST_OK
}

unsafe extern "C" fn reset_test_reset(converter: *mut c_void) -> c_int {
    if converter.is_null() || RESET_FAILS_AFTER_CREATION.load(Ordering::Relaxed) {
        TEST_ERROR
    } else {
        TEST_OK
    }
}

unsafe extern "C" fn reset_test_process(
    _converter: *mut c_void,
    _input: *const f32,
    _input_frames: u32,
    _output: *mut f32,
    _output_capacity: u32,
    _ratio: f64,
    _input_used: *mut u32,
    _output_generated: *mut u32,
) -> c_int {
    TEST_ERROR
}

unsafe extern "C" fn reset_test_destroy(converter: *mut c_void) {
    if !converter.is_null() {
        unsafe {
            drop(Box::from_raw(converter.cast::<ResetTestConverter>()));
        }
    }
}

static RESET_AFTER_CREATION_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 1,
    capability_name: TEST_ADAPTER_CAPABILITY.as_ptr().cast::<c_char>(),
    converter_create: Some(reset_test_create),
    converter_reset: Some(reset_test_reset),
    converter_process: Some(reset_test_process),
    converter_destroy: Some(reset_test_destroy),
};

fn handle_with(bridge: LegacyRing) -> *mut LegacyRing {
    Box::into_raw(Box::new(bridge))
}

fn fake_bridge(capacity: usize) -> LegacyRing {
    LegacyRing::create(
        capacity,
        Quality::Best,
        8_000,
        8_000,
        adapter(&FAKE_DESCRIPTOR),
    )
    .expect("fake compatibility bridge")
}

#[test]
fn legacy_bridge_retains_s16_boundary_and_small_legacy_capacities() {
    let ring = fake_bridge(1);
    assert!(ring.push_sample(i16::MIN));
    assert!(!ring.push_sample(i16::MAX));
    assert_eq!(ring.pop_sample(), Some(i16::MIN));
    assert_eq!(ring.pop_sample(), None);

    assert_eq!(ring.push(&[i16::MAX, 1, 2]), 1);
    let observation = ring.observe();
    assert_eq!(observation.capacity_samples, 1);
    assert_eq!(observation.available_samples, 1);
    assert_eq!(observation.discarded_samples, 3);
    assert_eq!(ring.pop_sample(), Some(i16::MAX));

    assert_eq!(LegacyRing::output_to_i16(-1.0), i16::MIN);
    assert_eq!(LegacyRing::output_to_i16(1.0), i16::MAX);
    assert_eq!(LegacyRing::output_to_i16(f32::NAN), 0);
    assert_eq!(LegacyRing::input_from_i16(i16::MIN), -1.0);
    assert!(LegacyRing::input_from_i16(i16::MAX) < 1.0);
}

#[test]
fn legacy_bridge_renders_records_and_reconfigures() {
    let ring = fake_bridge(8);
    assert_eq!(ring.push(&[100, 200, 300, 400]), 4);

    let mut output = [0_i16; 4];
    let (real, adapter_error) = ring.render(&mut output, 2, 3);
    assert_eq!(real, 4);
    assert!(!adapter_error);
    assert_eq!(output, [100, 200, 300, 400]);

    let (_missing, real, adapter_error) = ring.render_sample(3);
    assert!(!real);
    assert!(!adapter_error);
    let observation = ring.observe();
    assert_eq!(observation.reserve_samples, 2);
    assert_eq!(observation.target_samples, 3);
    assert_eq!(observation.missing_samples, 1);
    assert_eq!(observation.consecutive_underruns, 1);
    assert_eq!(observation.underrun_average_milli, 0);

    ring.statistics.record(0, 160, 8_000);
    assert_eq!(ring.observe().consecutive_underruns, 0);
    ring.statistics.record(1, 90_000, 8_000);
    assert!(ring.observe().underrun_average_milli > 0);
    ring.statistics.record(1, 1, 0);
    assert!(ring.observe().underrun_average_milli > 0);

    assert!(ring.reconfigure(0, 8_000).is_err());
    assert!(ring.reconfigure(8_000, 0).is_err());
    assert_eq!(ring.push(&[777]), 1);
    ring.reconfigure(16_000, 8_000)
        .expect("valid reconfiguration");
    assert_eq!(ring.pop_sample(), Some(777));
    assert_eq!(ring.push(&[123, -123]), 2);
    let mut converted = [0_i16; 2];
    assert_eq!(ring.render(&mut converted, 0, 1).0, 2);

    RESET_FAILS_AFTER_CREATION.store(false, Ordering::Relaxed);
    let reset_failure = LegacyRing::create(
        8,
        Quality::Best,
        8_000,
        8_000,
        adapter(&RESET_AFTER_CREATION_DESCRIPTOR),
    )
    .expect("bridge with initially usable reset");
    RESET_FAILS_AFTER_CREATION.store(true, Ordering::Relaxed);
    assert!(matches!(
        reset_failure.reconfigure(8_000, 8_000),
        Err(CreateError::Adapter)
    ));
    RESET_FAILS_AFTER_CREATION.store(false, Ordering::Relaxed);
}

#[test]
fn legacy_statistics_and_descriptor_metadata_cover_control_boundaries() {
    let statistics = LegacyStatistics::new();
    statistics.record(0, 1, 8_000);
    statistics.record(1, 160, 8_000);
    assert_eq!(statistics.missing.load(Ordering::Relaxed), 1);
    assert_eq!(statistics.consecutive_underruns.load(Ordering::Relaxed), 1);
    assert_eq!(statistics.underrun_average_milli.load(Ordering::Relaxed), 2);
    statistics.record(1, 90_000, 8_000);
    assert_eq!(
        statistics.underrun_average_milli.load(Ordering::Relaxed),
        2_000
    );

    let truncation = LegacyStatistics::new();
    truncation.record(1, 1, 3);
    assert_eq!(
        truncation.underrun_average_milli.load(Ordering::Relaxed),
        33
    );
    truncation.record(0, 1, 3);
    assert_eq!(
        truncation.underrun_average_milli.load(Ordering::Relaxed),
        32
    );
    truncation.record(1, 1, 0);
    assert_eq!(
        truncation.underrun_average_milli.load(Ordering::Relaxed),
        32
    );
    assert_eq!(truncation.missing.load(Ordering::Relaxed), 2);
    statistics.record(1, 1, 0);
    assert_eq!(
        statistics.underrun_average_milli.load(Ordering::Relaxed),
        2_000
    );

    let counter = AtomicU64::new(u64::MAX);
    assert_eq!(saturating_add(&counter, 1), u64::MAX);
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);

    assert_eq!(quality_from_ffi(0), Ok(Quality::Best));
    assert_eq!(quality_from_ffi(1), Ok(Quality::Medium));
    assert_eq!(quality_from_ffi(2), Ok(Quality::Fastest));
    assert_eq!(quality_from_ffi(3), Err(()));
    assert_eq!(create_error_result(CreateError::NoMemory), NO_MEMORY);
    assert_eq!(create_error_result(CreateError::Adapter), ADAPTER_ERROR);

    let descriptor: &LegacyDescriptor = unsafe { &*super::rpcr1_bridge_descriptor() };
    assert_eq!(descriptor.struct_size, size_of::<LegacyDescriptor>() as u32);
    assert_eq!(descriptor.abi_version, ABI_VERSION);
    assert_eq!(
        unsafe { core::ffi::CStr::from_ptr(descriptor.capability_name) }.to_bytes_with_nul(),
        CAPABILITY
    );
    assert!(descriptor.create as usize != 0);
    assert!(descriptor.destroy as usize != 0);
    assert!(descriptor.reconfigure as usize != 0);
    assert!(descriptor.push_sample as usize != 0);
    assert!(descriptor.push as usize != 0);
    assert!(descriptor.pop_sample as usize != 0);
    assert!(descriptor.render_sample as usize != 0);
    assert!(descriptor.render as usize != 0);
    assert!(descriptor.conceal as usize != 0);
    assert!(descriptor.observe as usize != 0);
    assert!(descriptor.record_shortfall as usize != 0);
}

#[test]
fn bridge_create_validates_arguments_and_adapter_setup() {
    let mut handle = ptr::NonNull::<LegacyRing>::dangling().as_ptr();
    assert_eq!(
        create_with_adapter(
            1,
            0,
            8_000,
            8_000,
            ptr::null_mut(),
            Ok(adapter(&FAKE_DESCRIPTOR))
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        create_with_adapter(
            0,
            0,
            8_000,
            8_000,
            &mut handle,
            Ok(adapter(&FAKE_DESCRIPTOR))
        ),
        INVALID_ARGUMENT
    );
    assert!(handle.is_null());
    assert_eq!(
        create_with_adapter(
            u64::from(u32::MAX) + 1,
            0,
            8_000,
            8_000,
            &mut handle,
            Ok(adapter(&FAKE_DESCRIPTOR))
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        create_with_adapter(
            1,
            3,
            8_000,
            8_000,
            &mut handle,
            Ok(adapter(&FAKE_DESCRIPTOR))
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        create_with_adapter(1, 0, 0, 8_000, &mut handle, Ok(adapter(&FAKE_DESCRIPTOR))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        create_with_adapter(1, 0, 8_000, 0, &mut handle, Ok(adapter(&FAKE_DESCRIPTOR))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        create_with_adapter(1, 0, 8_000, 8_000, &mut handle, Err(())),
        ADAPTER_ERROR
    );
    assert_eq!(
        create_with_adapter(
            1,
            0,
            8_000,
            8_000,
            &mut handle,
            Ok(adapter(&CREATE_FAILURE_DESCRIPTOR))
        ),
        ADAPTER_ERROR
    );
    assert!(handle.is_null());

    assert_eq!(
        create_with_adapter(
            1,
            0,
            8_000,
            8_000,
            &mut handle,
            Ok(adapter(&FAKE_DESCRIPTOR))
        ),
        OK
    );
    assert!(!handle.is_null());
    destroy(handle);
    destroy(ptr::null_mut());

    assert_eq!(create(1, 0, 8_000, 8_000, &mut handle), OK);
    assert!(!handle.is_null());
    destroy(handle);
}

#[test]
fn bridge_ffi_operations_validate_inputs_and_forward_results() {
    let mut accepted = false;
    let mut accepted_count = 0_u64;
    let mut output = 0_i16;
    let mut available = false;
    let mut real = false;
    let mut real_count = 0_u64;
    let mut observation = LegacyObservation {
        capacity_samples: 0,
        available_samples: 0,
        written_samples: 0,
        read_samples: 0,
        reserve_samples: 0,
        filtered_occupancy_samples: 0,
        target_samples: 0,
        ratio_correction_ppm: 0,
        discarded_samples: 0,
        missing_samples: 0,
        consecutive_underruns: 0,
        underrun_average_milli: 0,
    };
    let handle = handle_with(fake_bridge(8));
    let one = [100_i16];

    assert_eq!(reconfigure(ptr::null_mut(), 8_000, 8_000), INVALID_ARGUMENT);
    assert_eq!(reconfigure(handle, 0, 8_000), INVALID_ARGUMENT);
    assert_eq!(reconfigure(handle, 8_000, 0), INVALID_ARGUMENT);
    assert_eq!(reconfigure(handle, 16_000, 8_000), OK);

    assert_eq!(
        push_sample(ptr::null_mut(), 1, &mut accepted, &mut observation),
        INVALID_ARGUMENT
    );
    assert_eq!(
        push_sample(handle, 1, ptr::null_mut(), &mut observation),
        INVALID_ARGUMENT
    );
    assert_eq!(
        push_sample(handle, 1, &mut accepted, ptr::null_mut()),
        INVALID_ARGUMENT
    );
    assert_eq!(push_sample(handle, 1, &mut accepted, &mut observation), OK);
    assert!(accepted);

    assert_eq!(
        push(ptr::null_mut(), one.as_ptr(), 1, &mut accepted_count),
        INVALID_ARGUMENT
    );
    assert_eq!(
        push(handle, ptr::null(), 1, &mut accepted_count),
        INVALID_ARGUMENT
    );
    assert_eq!(
        push(handle, ptr::null(), 0, ptr::null_mut()),
        INVALID_ARGUMENT
    );
    assert_eq!(push(handle, ptr::null(), 0, &mut accepted_count), OK);
    assert_eq!(accepted_count, 0);
    assert_eq!(
        push(
            handle,
            one.as_ptr(),
            isize::MAX as u64 / size_of::<i16>() as u64 + 1,
            &mut accepted_count
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(push(handle, one.as_ptr(), 1, &mut accepted_count), OK);
    assert_eq!(accepted_count, 1);

    assert_eq!(
        pop_sample(ptr::null_mut(), &mut output, &mut available),
        INVALID_ARGUMENT
    );
    assert_eq!(
        pop_sample(handle, ptr::null_mut(), &mut available),
        INVALID_ARGUMENT
    );
    assert_eq!(
        pop_sample(handle, &mut output, ptr::null_mut()),
        INVALID_ARGUMENT
    );
    assert_eq!(pop_sample(handle, &mut output, &mut available), OK);
    assert!(available);
    assert_eq!(output, 1);
    output = 123;
    assert_eq!(pop_sample(handle, &mut output, &mut available), OK);
    assert!(available);
    assert_eq!(output, 100);
    output = 123;
    assert_eq!(pop_sample(handle, &mut output, &mut available), OK);
    assert!(!available);
    assert_eq!(output, 123);

    assert_eq!(
        render_sample(ptr::null_mut(), &mut output, 1, &mut real, &mut observation),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render_sample(handle, ptr::null_mut(), 1, &mut real, &mut observation),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render_sample(handle, &mut output, 1, ptr::null_mut(), &mut observation),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render_sample(handle, &mut output, 1, &mut real, ptr::null_mut()),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render_sample(handle, &mut output, 1, &mut real, &mut observation),
        OK
    );

    assert_eq!(conceal(ptr::null_mut(), &mut output, 1), INVALID_ARGUMENT);
    assert_eq!(conceal(handle, ptr::null_mut(), 1), INVALID_ARGUMENT);
    output = 123;
    assert_eq!(conceal(handle, &mut output, 1), OK);

    assert_eq!(
        render(ptr::null_mut(), &mut output, 1, 0, 1, &mut real_count),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render(handle, ptr::null_mut(), 1, 0, 1, &mut real_count),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render(handle, ptr::null_mut(), 0, 0, 1, ptr::null_mut()),
        INVALID_ARGUMENT
    );
    assert_eq!(
        render(handle, ptr::null_mut(), 0, 0, 1, &mut real_count),
        OK
    );
    assert_eq!(real_count, 0);
    assert_eq!(
        render(
            handle,
            &mut output,
            isize::MAX as u64 / size_of::<i16>() as u64 + 1,
            0,
            1,
            &mut real_count
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(render(handle, &mut output, 1, 2, 3, &mut real_count), OK);

    assert_eq!(observe(ptr::null(), &mut observation), INVALID_ARGUMENT);
    assert_eq!(observe(handle, ptr::null_mut()), INVALID_ARGUMENT);
    assert_eq!(observe(handle, &mut observation), OK);
    assert_eq!(observation.capacity_samples, 8);

    assert_eq!(
        record_shortfall(ptr::null_mut(), 1, 1, 8_000),
        INVALID_ARGUMENT
    );
    assert_eq!(record_shortfall(handle, 1, 1, 8_000), OK);
    assert!(observe(handle, &mut observation) == OK);
    assert!(observation.missing_samples >= 1);

    destroy(handle);
}

#[test]
fn bridge_reports_adapter_failures_without_skipping_concealment() {
    let handle = handle_with(
        LegacyRing::create(8, Quality::Best, 8_000, 8_000, adapter(&FAILING_DESCRIPTOR))
            .expect("failing-process bridge"),
    );
    let mut accepted = false;
    let mut output = 1_i16;
    let mut real = true;
    let mut real_count = 1_u64;

    let mut observation = LegacyObservation {
        capacity_samples: 0,
        available_samples: 0,
        written_samples: 0,
        read_samples: 0,
        reserve_samples: 0,
        filtered_occupancy_samples: 0,
        target_samples: 0,
        ratio_correction_ppm: 0,
        discarded_samples: 0,
        missing_samples: 0,
        consecutive_underruns: 0,
        underrun_average_milli: 0,
    };
    assert_eq!(
        push_sample(handle, 123, &mut accepted, &mut observation),
        OK
    );
    assert!(accepted);
    assert_eq!(
        render_sample(handle, &mut output, 1, &mut real, &mut observation),
        OK
    );
    assert!(!real);
    assert_eq!(output, 0);
    assert_eq!(render(handle, &mut output, 1, 0, 1, &mut real_count), OK);
    assert_eq!(real_count, 0);
    destroy(handle);
}
