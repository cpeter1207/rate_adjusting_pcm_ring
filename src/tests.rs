//! Unit coverage for the Rust ring using a deterministic in-process SRC fake.

use crate::ring::{PlcMode, Settings};

#[test]
fn intentional_mute_updates_reach_already_converted_cache_without_loss_or_plc() {
    use crate::mute::Span;
    let handle = fake_handle();
    let ring = unsafe { &(*handle.as_ptr()).ring };
    assert_eq!(ring.producer_push_tagged(&[0.5; 120], 7, 0, &[]), Ok(120));
    assert_eq!(ring.consumer_render_sample(), (0.5, true, false));
    assert_eq!(ring.producer_mute(7, &[Span { start: 10, end: 40 }]), Ok(1));
    for source in 1..120 {
        assert_eq!(
            ring.consumer_render_sample(),
            (
                if (10..40).contains(&source) { 0.0 } else { 0.5 },
                true,
                false
            )
        );
    }
    assert_eq!(ring.observe().missing_samples, 0);
    assert_eq!(ring.observe().adapter_error_count, 0);
}

#[test]
fn intentional_mute_tags_survive_detector_release_plc_lookahead_and_generation_reset() {
    use crate::mute::Span;
    let ring = Ring::create(
        Settings {
            reserve: 40,
            target: 128,
            max_producer: 128,
            max_output: 32,
            plc: PlcMode::G711AppendixI,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    assert_eq!(ring.producer_push_tagged(&[0.5; 128], 9, 0, &[]), Ok(128));
    for _ in 0..20 {
        ring.consumer_render_sample();
    }
    // This interval arrives after its samples entered both SRC and PLC caches.
    assert_eq!(ring.producer_mute(9, &[Span { start: 10, end: 40 }]), Ok(1));
    for output in 20..128 {
        let (sample, _, failed) = ring.consumer_render_sample();
        assert!(!failed);
        let source = output - 30; // G.711 Appendix I retains 3.75 ms at 8 kHz.
        if (10..40).contains(&source) {
            assert_eq!(sample, 0.0, "delayed source {source}");
        }
    }
    assert_eq!(ring.observe().missing_samples, 0);
    ring.consumer_reset().unwrap();
    assert_eq!(ring.producer_push_tagged(&[0.25; 128], 10, 0, &[]), Ok(128));
    assert!(
        ring.producer_mute(9, &[Span { start: 0, end: 128 }])
            .is_err()
    );
    for _ in 0..30 {
        ring.consumer_render_sample();
    }
    for _ in 0..98 {
        assert_eq!(ring.consumer_render_sample(), (0.25, true, false));
    }
}

#[test]
fn intentional_mute_partial_admission_retries_only_its_source_suffix() {
    use crate::mute::Span;
    let handle = fake_handle();
    let ring = unsafe { &(*handle.as_ptr()).ring };
    assert_eq!(ring.producer_push_tagged(&[0.5; 400], 1, 0, &[]), Ok(400));
    assert_eq!(
        ring.producer_push_tagged(
            &[0.5; 200],
            1,
            400,
            &[Span {
                start: 460,
                end: 550
            }]
        ),
        Ok(112)
    );
    assert!(ring.producer_push_tagged(&[0.5; 1], 1, 600, &[]).is_err());
    for _ in 0..100 {
        ring.consumer_render_sample();
    }
    assert_eq!(
        ring.producer_push_tagged(
            &[0.5; 88],
            1,
            512,
            &[Span {
                start: 460,
                end: 550
            }]
        ),
        Ok(88)
    );
    for source in 100..600 {
        assert_eq!(
            ring.consumer_render_sample(),
            (
                if (460..550).contains(&source) {
                    0.0
                } else {
                    0.5
                },
                true,
                false
            )
        );
    }
}

#[test]
fn intentional_mute_actual_converter_preserves_source_edges_with_drift_and_plc() {
    use crate::mute::Span;
    for rate in [8000, 48000] {
        for plc in [PlcMode::Disabled, PlcMode::G711AppendixI] {
            let ring = Ring::create(
                Settings {
                    reserve: 300,
                    target: 600,
                    max_producer: 1200,
                    max_output: 256,
                    plc,
                    ..settings(2048, rate, 48000)
                },
                load_functions().unwrap(),
            )
            .unwrap();
            assert_eq!(ring.producer_push_tagged(&[0.5; 1200], 1, 0, &[]), Ok(1200));
            let factor = (48000 / rate) as usize;
            let delay = ring.output_delay_samples() as usize
                + if plc == PlcMode::G711AppendixI {
                    180
                } else {
                    0
                };
            let start = delay + 700 * factor;
            let end = delay + 740 * factor;
            for _ in 0..start - 20 {
                assert!(!ring.consumer_render_sample().2);
            }
            assert_eq!(
                ring.producer_mute(
                    1,
                    &[Span {
                        start: 700,
                        end: 740
                    }]
                ),
                Ok(1)
            );
            let mut muted = Vec::new();
            for output in start - 20..end + 20 {
                let (sample, _, failed) = ring.consumer_render_sample();
                assert!(!failed);
                if sample == 0.0 {
                    muted.push(output);
                }
            }
            assert!(
                muted.first().unwrap().abs_diff(start) <= factor + 1,
                "rate={rate}, plc={plc:?}, first={:?}, expected={start}",
                muted.first()
            );
            assert!(muted.last().unwrap().abs_diff(end - 1) <= factor + 1);
            assert_eq!(ring.observe().missing_samples, 0);
            assert_ne!(
                ring.observe().ratio_correction_ppm,
                0,
                "exercise actual clock compensation"
            );
        }
    }
}

#[test]
fn intentional_mute_metadata_backpressure_and_audio_are_allocation_free() {
    use crate::mute::Span;
    let handle = fake_handle();
    let ring = unsafe { &(*handle.as_ptr()).ring };
    let before = ALLOCATION_COUNT.get();
    assert_eq!(ring.producer_push_tagged(&[0.5; 10], 1, 0, &[]), Ok(10));
    for _ in 0..128 {
        assert_eq!(ring.producer_mute(1, &[Span { start: 0, end: 10 }]), Ok(1));
    }
    assert_eq!(ring.producer_mute(1, &[Span { start: 0, end: 10 }]), Ok(0));
    assert_eq!(
        ring.producer_push_tagged(&[0.5], 1, 10, &[Span { start: 10, end: 11 }]),
        Ok(0)
    );
    assert_eq!(
        ring.observe().discarded_samples,
        0,
        "metadata backpressure is not audio loss"
    );
    for _ in 0..10 {
        assert_eq!(ring.consumer_render_sample(), (0.0, true, false));
    }
    assert_eq!(ring.producer_mute(1, &[Span { start: 0, end: 10 }]), Ok(1));
    ring.consumer_reset().unwrap();
    assert_eq!(ALLOCATION_COUNT.get(), before);
}
use core::ffi::{CStr, c_char, c_int, c_void};

#[test]
fn intentional_mute_gap_is_real_silence_not_a_second_concealment_request() {
    use crate::mute::Span;
    let handle = fake_handle();
    let ring = unsafe { &(*handle.as_ptr()).ring };
    assert_eq!(ring.producer_push_tagged(&[0.5; 40], 1, 0, &[]), Ok(40));
    assert_eq!(ring.producer_gap(20), 20);
    assert_eq!(ring.producer_push_tagged(&[0.5; 40], 1, 60, &[]), Ok(40));
    assert_eq!(ring.producer_mute(1, &[Span { start: 40, end: 60 }]), Ok(1));
    for source in 0..100 {
        assert_eq!(
            ring.consumer_render_sample(),
            (
                if (40..60).contains(&source) { 0.0 } else { 0.5 },
                true,
                false
            )
        );
    }
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn intentional_mute_descriptor_tail_validates_before_touching_pcm_or_outputs() {
    use crate::mute::Span;
    let handle = fake_handle();
    let table = descriptor();
    assert_eq!(
        core::mem::offset_of!(Descriptor, ring_producer_push_tagged),
        96
    );
    let pointer = handle.as_ptr();
    let push = table.ring_producer_push_tagged;
    let update = table.ring_producer_mute;
    let mut accepted = 99;
    for (ring, input, count, spans, span_count) in [
        (ptr::null_mut(), ptr::null(), 0, ptr::null(), 0),
        (pointer, ptr::dangling(), 513, ptr::null(), 0),
        (pointer, ptr::null(), 1, ptr::null(), 0),
        (pointer, ptr::null(), 0, ptr::null(), 1),
        (pointer, ptr::null(), 0, ptr::dangling(), 65),
    ] {
        assert_eq!(
            push(ring, input, count, 1, 0, spans, span_count, &mut accepted),
            RESULT_INVALID_ARGUMENT
        );
        assert_eq!(accepted, 99);
    }
    assert_eq!(
        push(
            pointer,
            ptr::null(),
            0,
            1,
            0,
            ptr::null(),
            0,
            ptr::null_mut()
        ),
        RESULT_INVALID_ARGUMENT
    );
    let input = [0.5];
    let invalid = Span { start: 2, end: 1 };
    assert_eq!(
        push(pointer, input.as_ptr(), 1, 1, 0, &invalid, 1, &mut accepted),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(accepted, 99);
    assert_eq!(
        push(
            pointer,
            input.as_ptr(),
            1,
            1,
            u64::MAX,
            ptr::null(),
            0,
            &mut accepted
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        push(
            pointer,
            input.as_ptr(),
            1,
            1,
            0,
            ptr::null(),
            0,
            &mut accepted
        ),
        RESULT_OK
    );
    assert_eq!(accepted, 1);
    let span = Span { start: 0, end: 1 };
    let mut intervals = 99;
    assert_eq!(update(pointer, 1, &span, 1, &mut intervals), RESULT_OK);
    assert_eq!(intervals, 1);
    assert_eq!(
        update(ptr::null_mut(), 1, &span, 1, &mut intervals),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        update(pointer, 1, ptr::null(), 1, &mut intervals),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        update(pointer, 1, &span, 1, ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        update(pointer, 2, &span, 1, &mut intervals),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(intervals, 1);
}

#[test]
fn immutable_policy_rejects_insufficient_callback_or_burst_headroom() {
    for (fi, fo, p, n, r, t, c, enabled, valid) in [
        (8000, 48000, 160, 960, 162, 320, 640, true, true),
        (8000, 48000, 160, 960, 161, 320, 640, true, false),
        (8000, 48000, 320, 960, 162, 320, 640, true, true),
        (8000, 48000, 320, 960, 162, 320, 639, true, false),
        (8000, 48000, 4096, 4096, 685, 2080, 6176, true, true),
        (8000, 48000, 4096, 4096, 685, 2080, 6175, true, false),
        (48000, 48000, 4096, 4096, 4102, 12480, 16576, true, true),
        (48000, 48000, 4096, 4096, 0, 0, 14400, false, true),
        (48000, 48000, 4096, 4096, 7200, 7200, 14400, false, true),
        (48000, 48000, 4096, 4096, 7200, 0, 14400, false, true),
        (48000, 48000, 1, 1, 0, 0, 512, true, false),
        (48000, 48000, 0, 1, 100, 100, 512, true, false),
        (48000, 48000, 1, 0, 100, 100, 512, true, false),
        (0, 48000, 1, 1, 0, 0, 512, false, false),
        (48000, 0, 1, 1, 0, 0, 512, false, false),
        (1, 257, 1, 1, 0, 0, 512, false, false),
        (257, 1, 1, 1, 0, 0, 512, false, false),
        (256, 1, 1, 1, 0, 0, 512, false, true),
        (1, 256, 1, 1, 0, 0, 512, false, true),
        (48000, 48000, 1, 1, 513, 0, 512, false, false),
        (48000, 48000, 1, 1, 0, 513, 512, false, false),
        (48000, 48000, 1, 1, 0, 0, 511, false, false),
        (48000, 48000, 1, 1, 100, 99, 512, true, false),
        (48000, 48000, u64::MAX, 1, 100, 100, 512, true, false),
        (48000, 48000, 1, u64::MAX, 100, 100, 512, true, false),
        (256, 1, 1, 1, 257, 257, 512, true, true),
    ] {
        let config = Settings {
            capacity: c,
            input_rate_hz: fi,
            output_rate_hz: fo,
            reserve: r,
            target: t,
            max_producer: p,
            max_output: n,
            plc: if enabled {
                PlcMode::G711AppendixI
            } else {
                PlcMode::Disabled
            },
        };
        assert_eq!(config.validate().is_ok(), valid, "{config:?}");
    }
    assert_eq!(PlcMode::from_ffi(0), Ok(PlcMode::Disabled));
    assert_eq!(PlcMode::from_ffi(1), Ok(PlcMode::G711AppendixI));
    assert_eq!(PlcMode::from_ffi(2), Err(()));
}
use core::mem::size_of;
use core::ptr;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use crate::ring::{CreateError, MAXIMUM_CAPACITY, Ring, allocate_samples};
use crate::samplerate_adapter::{AdapterDescriptor, AdapterFunctions, load_functions};
use crate::{
    ABI_VERSION, CObservation, Config, Descriptor, RESULT_ADAPTER_ERROR, RESULT_INVALID_ARGUMENT,
    RESULT_NO_MEMORY, RESULT_OK, Rpcr3Ring, finish_ring_create, ring_create_with_adapter,
    ring_create_with_adapter_and_allocator, ring_destroy, rpcr3_descriptor,
};

const OK: c_int = 0;
const ERROR: c_int = -1;
const CAPABILITY_NAME: &[u8] = b"rptadv.samplerate\0";
const WRONG_CAPABILITY_NAME: &[u8] = b"test.samplerate-adapter\0";

#[test]
fn counted_gaps_keep_position_between_already_buffered_pcm() {
    let table = descriptor();
    assert!(
        table.struct_size >= 96,
        "counted-gap descriptor tail missing"
    );
    let gap = unsafe {
        (table as *const Descriptor)
            .cast::<u8>()
            .add(88)
            .cast::<extern "C" fn(*mut Rpcr3Ring, u64, *mut u64) -> c_int>()
            .read()
    };
    let handle = handle_with(adapter(&FAKE_DESCRIPTOR));
    let ring = unsafe { &(*handle.as_ptr()).ring };
    assert_eq!(ring.producer_push(&[0.5; 40]), 40);
    let mut accepted = 99;
    assert_eq!(gap(handle.as_ptr(), 17, &mut accepted), RESULT_OK);
    assert_eq!(accepted, 17);
    assert_eq!(ring.producer_push(&[-0.5; 40]), 40);
    for index in 0..97 {
        let (sample, real, failed) = ring.consumer_render_sample();
        assert!(!failed);
        assert_eq!(real, !(40..57).contains(&index), "output {index}");
        assert_eq!(
            sample,
            if index < 40 {
                0.5
            } else if index < 57 {
                0.0
            } else {
                -0.5
            }
        );
    }
    assert_eq!(ring.observe().missing_samples, 17);
}

thread_local! {
    static FAIL_BYTES: Cell<usize> = const { Cell::new(0) };
    static FAIL_AFTER: Cell<usize> = const { Cell::new(1) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn counted_gaps_validate_bounds_and_retry_only_the_unaccepted_suffix() {
    let handle = fake_handle();
    let pointer = handle.as_ptr();
    let ring = unsafe { &(*pointer).ring };
    let gap = descriptor().ring_producer_gap;
    let mut accepted = 99;
    assert_eq!(
        gap(ptr::null_mut(), 1, &mut accepted),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(gap(pointer, 1, ptr::null_mut()), RESULT_INVALID_ARGUMENT);
    assert_eq!(gap(pointer, 513, &mut accepted), RESULT_INVALID_ARGUMENT);
    assert_eq!(accepted, 99);
    assert_eq!(gap(pointer, 0, &mut accepted), RESULT_OK);
    assert_eq!(accepted, 0);
    // Exercise nonempty output cache, partial gap admission, repeated gaps,
    // cursor wrap and metadata reuse over more than two FIFO laps.
    for _ in 0..8 {
        assert_eq!(ring.producer_push(&[0.25; 510]), 510);
        assert_eq!(gap(pointer, 7, &mut accepted), RESULT_OK);
        assert_eq!(accepted, 2);
        assert_eq!(ring.consumer_render_sample(), (0.25, true, false));
        assert_eq!(gap(pointer, 5, &mut accepted), RESULT_OK);
        assert_eq!(accepted, 5);
        assert_eq!(gap(pointer, 3, &mut accepted), RESULT_OK);
        assert_eq!(accepted, 3);
        assert_eq!(ring.producer_push(&[-0.25; 12]), 12);
        for _ in 1..510 {
            assert_eq!(ring.consumer_render_sample(), (0.25, true, false));
        }
        for _ in 0..10 {
            assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
        }
        for _ in 0..12 {
            assert_eq!(ring.consumer_render_sample(), (-0.25, true, false));
        }
    }
    assert_eq!(ring.observe().missing_samples, 80);
    assert_eq!(ring.observe().discarded_samples, 40);
    assert_eq!(ring.producer_gap(30), 30);
    assert_eq!(ring.consumer_reset(), Ok(()));
    assert_eq!(ring.producer_push(&[0.125; 40]), 40);
    for _ in 0..40 {
        assert_eq!(ring.consumer_render_sample(), (0.125, true, false));
    }
}

#[test]
fn counted_gaps_reach_existing_plc_once_without_runtime_allocation() {
    let ring = Ring::create(
        Settings {
            reserve: 40,
            target: 160,
            max_producer: 160,
            max_output: 32,
            plc: PlcMode::G711AppendixI,
            ..settings(2048, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    let mut reference = crate::plc::Plc::new(8000).unwrap();
    let tone: Vec<f32> = (0..160).map(|i| (i as f32 * 0.2).sin() * 0.5).collect();
    let before = ALLOCATION_COUNT.get();
    assert_eq!(ring.producer_push(&tone), 160);
    assert_eq!(ring.producer_gap(80), 80);
    assert_eq!(ring.producer_push(&tone), 160);
    for index in 0..440 {
        let input = match index {
            0..160 => Some(tone[index]),
            240..400 => Some(tone[index - 240]),
            _ => None,
        };
        let expected = reference.process(input);
        let (sample, real, failed) = ring.consumer_render_sample();
        assert!(!failed);
        assert_eq!((sample, real), expected, "output {index}");
    }
    assert_eq!(ALLOCATION_COUNT.get(), before);
    assert_eq!(ring.observe().missing_samples, 120);
}

#[test]
fn live_position_tail_is_required_and_query_failures_are_reported() {
    let missing = AdapterDescriptor {
        converter_input_delay: None,
        ..FAKE_DESCRIPTOR
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing) }.is_err());
    let old_prefix = AdapterDescriptor {
        struct_size: 64,
        ..FAKE_DESCRIPTOR
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&old_prefix) }.is_err());
    unsafe extern "C" fn invalid(_converter: *mut c_void, delay: *mut i64) -> c_int {
        unsafe { *delay = -1 };
        OK
    }
    assert!(matches!(
        Ring::create(
            settings(512, 8000, 8000),
            adapter(&AdapterDescriptor {
                converter_input_delay: Some(invalid),
                ..FAKE_DESCRIPTOR
            })
        ),
        Err(CreateError::Adapter)
    ));
    thread_local! { static FAIL_DELAY: Cell<bool> = const { Cell::new(false) }; }
    unsafe extern "C" fn fail_when_requested(_converter: *mut c_void, delay: *mut i64) -> c_int {
        if FAIL_DELAY.get() {
            ERROR
        } else {
            unsafe { *delay = 0 };
            OK
        }
    }
    let ring = Ring::create(
        settings(512, 8000, 8000),
        adapter(&AdapterDescriptor {
            converter_input_delay: Some(fail_when_requested),
            ..FAKE_DESCRIPTOR
        }),
    )
    .unwrap();
    assert_eq!(ring.producer_gap(2), 2);
    FAIL_DELAY.set(true);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.observe().adapter_error_count, 1);
    FAIL_DELAY.set(false);
    assert_eq!(ring.consumer_reset(), Ok(()));
}

#[test]
fn real_converter_places_successive_gaps_at_nominal_output_boundaries() {
    for rate in [8000, 48000] {
        for partition in [1, 13, 256] {
            let ring =
                Ring::create(settings(2048, rate, 48000), load_functions().unwrap()).unwrap();
            assert_eq!(ring.producer_push(&[0.5; 80]), 80);
            // Generate and retain an output cache before publishing the gap.
            assert!(!ring.consumer_render_sample().2);
            assert_eq!(ring.producer_gap(17), 17);
            assert_eq!(ring.producer_push(&[-0.5; 37]), 37);
            assert_eq!(ring.producer_gap(19), 19);
            assert_eq!(ring.producer_gap(1), 1);
            assert_eq!(ring.producer_push(&[0.25; 900]), 900);
            let factor = 48000 / rate;
            let delay = ring.output_delay_samples() as usize;
            let intervals = [
                (delay + 80 * factor as usize, 17 * factor as usize),
                (delay + 134 * factor as usize, 20 * factor as usize),
            ];
            let mut missing = Vec::new();
            for index in 1..(delay + 220 * factor as usize) {
                let (_, real, failed) = ring.consumer_render_sample();
                assert!(!failed);
                if !real {
                    missing.push(index);
                }
                // Partition the producer's later PCM independently from source
                // spans, keeping following data available through the FIR.
                if index % (factor as usize * 100) == 0 {
                    ring.producer_push(&vec![0.25; partition]);
                }
            }
            assert_eq!(
                missing.len(),
                37 * factor as usize,
                "rate {rate}, partition {partition}"
            );
            let mut offset = 0;
            for (start, length) in intervals {
                assert!(
                    missing[offset].abs_diff(start) <= 1,
                    "{rate}: {:?}",
                    missing
                );
                assert_eq!(missing[offset + length - 1] - missing[offset] + 1, length);
                offset += length;
            }
            assert_eq!(ring.observe().missing_samples, missing.len() as u64);
        }
    }
}
struct Allocator;

fn reject(bytes: usize) -> bool {
    let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get().saturating_add(1)));
    FAIL_BYTES
        .try_with(|slot| {
            if slot.get() == bytes {
                if FAIL_AFTER.get() > 1 {
                    FAIL_AFTER.set(FAIL_AFTER.get() - 1);
                    return false;
                }
                slot.set(0);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

// SAFETY: layouts and live pointers are forwarded unchanged to System. Returning null
// for one selected allocation is explicitly permitted by GlobalAlloc.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject(layout.size()) {
            ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        if reject(bytes) {
            ptr::null_mut()
        } else {
            unsafe { System.realloc(pointer, layout, bytes) }
        }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn fail_allocation<T>(bytes: usize, operation: impl FnOnce() -> T) -> T {
    fail_allocation_at(bytes, 1, operation)
}

pub(crate) fn fail_allocation_at<T>(
    bytes: usize,
    occurrence: usize,
    operation: impl FnOnce() -> T,
) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            FAIL_BYTES.set(0);
            FAIL_AFTER.set(1);
        }
    }
    assert_ne!(bytes, 0);
    assert_eq!(FAIL_BYTES.replace(bytes), 0);
    FAIL_AFTER.set(occurrence);
    let _clear = Clear;
    let result = operation();
    assert_eq!(FAIL_BYTES.get(), 0, "selected allocation was not reached");
    result
}

struct FakeConverter;

unsafe extern "C" fn fake_create(
    input_rate: u32,
    output_rate: u32,
    max_input: u32,
    max_output: u32,
    output: *mut *mut c_void,
) -> c_int {
    if input_rate == 0 || output_rate == 0 || max_input == 0 || max_output == 0 || output.is_null()
    {
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

unsafe extern "C" fn fake_queued_input(_converter: *mut c_void, frames: *mut u32) -> c_int {
    unsafe { *frames = 0 };
    OK
}

unsafe extern "C" fn fake_input_delay(_converter: *mut c_void, delay: *mut i64) -> c_int {
    unsafe { *delay = 0 };
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
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
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
        || output.is_null()
        || input_used.is_null()
        || output_generated.is_null()
        || !ratio.is_finite()
        || ratio <= 0.0
    {
        return ERROR;
    }
    unsafe {
        *input_used = input_frames.min(1);
        *output_generated = 0;
    }
    OK
}

pub(crate) static FAILING_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(failing_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

pub(crate) static ZERO_PROGRESS_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(zero_progress_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static CONSUME_ONE_ZERO_OUTPUT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(consume_one_zero_output_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static WRONG_CAPABILITY_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: WRONG_CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

unsafe extern "C" fn create_fails(
    _input_rate: u32,
    _output_rate: u32,
    _max_input: u32,
    _max_output: u32,
    _output: *mut *mut c_void,
) -> c_int {
    ERROR
}

unsafe extern "C" fn create_without_handle(
    _input_rate: u32,
    _output_rate: u32,
    _max_input: u32,
    _max_output: u32,
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
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(create_fails),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static NULL_HANDLE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(create_without_handle),
    converter_reset: Some(fake_reset),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static RESET_FAILURE_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(reset_fails),
    converter_process: Some(fake_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static INVALID_COUNT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(invalid_count_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

static INVALID_OUTPUT_COUNT_DESCRIPTOR: AdapterDescriptor = AdapterDescriptor {
    struct_size: size_of::<AdapterDescriptor>() as u32,
    abi_version: 2,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    converter_create: Some(fake_create),
    converter_reset: Some(fake_reset),
    converter_process: Some(invalid_output_count_process),
    converter_destroy: Some(fake_destroy),
    converter_queued_input: Some(fake_queued_input),
    converter_output_delay: Some(fake_queued_input),
    converter_input_delay: Some(fake_input_delay),
};

pub(crate) fn adapter(descriptor: &AdapterDescriptor) -> AdapterFunctions {
    unsafe { AdapterFunctions::from_descriptor(descriptor) }.expect("valid test adapter")
}

pub(crate) fn settings(capacity: usize, input_rate_hz: u32, output_rate_hz: u32) -> Settings {
    Settings {
        capacity: capacity as u64,
        input_rate_hz,
        output_rate_hz,
        reserve: 0,
        target: 0,
        max_producer: capacity.max(1) as u64,
        max_output: capacity.max(1) as u64,
        plc: PlcMode::Disabled,
    }
}

fn ring(capacity: usize) -> Ring {
    Ring::create(settings(capacity, 8_000, 8_000), adapter(&FAKE_DESCRIPTOR))
        .expect("test ring allocation")
}

fn valid_config() -> Config {
    Config {
        struct_size: size_of::<Config>() as u32,
        abi_version: ABI_VERSION,
        capacity_samples: 512,
        input_rate_hz: 8_000,
        output_rate_hz: 8_000,
        reserve_samples: 0,
        target_samples: 0,
        max_producer_samples: 512,
        max_output_samples: 512,
        plc_mode: 0,
    }
}

fn descriptor() -> &'static Descriptor {
    let descriptor = rpcr3_descriptor();
    assert!(!descriptor.is_null());
    unsafe { &*descriptor }
}

struct TestHandle(*mut Rpcr3Ring);

impl TestHandle {
    fn as_ptr(&self) -> *mut Rpcr3Ring {
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
fn producer_and_renderer_preserve_canonical_f32() {
    let ring = ring(512);
    assert!(ring.producer_push_sample(2.0));
    assert!(ring.producer_push_sample(f32::NAN));
    assert!(ring.producer_push_sample(-0.25));
    let mut output = [0.0; 3];
    assert_eq!(ring.consumer_render(&mut output).0, 3);
    assert_eq!(output, [1.0, 0.0, -0.25]);
}

#[test]
fn full_producer_rejects_only_new_samples() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&vec![0.1; 512]), 512);
    assert_eq!(ring.producer_push(&[0.2, 0.3]), 0);
    let (_, real, adapter_error) = ring.consumer_render_sample();
    assert!(real);
    assert!(!adapter_error);
    assert_eq!(ring.producer_push(&[0.4]), 1);
    let observation = ring.observe();
    assert_eq!(observation.discarded_samples, 2);
    assert!(observation.available_samples < 512);
}

#[test]
fn rendering_primes_startup_to_reserve() {
    let ring = Ring::create(
        Settings {
            reserve: 12,
            target: 8,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.1, 0.2, 0.3, 0.4]), 4);
    let mut output = [0.0; 4];
    assert_eq!(ring.consumer_render(&mut output).0, 0);
    assert_eq!(output, [0.0; 4]);

    assert_eq!(ring.producer_push(&[0.5; 8]), 8);
    assert_eq!(ring.consumer_render(&mut output).0, 4);
    assert_eq!(output, [0.1, 0.2, 0.3, 0.4]);
    let observation = ring.observe();
    assert_eq!(observation.reserve_samples, 12);
    assert_eq!(observation.target_samples, 8);
    assert_eq!(observation.missing_samples, 0);
}

#[test]
fn sample_rendering_cannot_bypass_an_unprimed_reserve() {
    let ring = Ring::create(
        Settings {
            reserve: 12,
            target: 8,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    ring.producer_push(&[0.5; 4]);
    ring.consumer_render(&mut [0.0; 1]);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.available(), 4);
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn shortfall_without_plc_is_silence_and_resumes_without_repriming() {
    let ring = ring(512);
    ring.producer_push(&[0.5; 512]);
    ring.consumer_render(&mut [0.0; 512]);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.observe().missing_samples, 1);
    ring.producer_push(&[0.3]);
    assert_eq!(ring.consumer_render_sample(), (0.3, true, false));
}

#[test]
fn enabled_ring_distinguishes_priming_lookahead_and_real_shortfall() {
    let ring = Ring::create(
        Settings {
            reserve: 34,
            target: 128,
            max_producer: 128,
            max_output: 32,
            plc: PlcMode::G711AppendixI,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    ring.producer_push(&[0.5; 16]);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.observe().missing_samples, 0);
    ring.producer_push(&[0.5; 112]);
    for _ in 0..30 {
        assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    }
    assert_eq!(ring.observe().missing_samples, 0);
    for _ in 30..128 {
        assert_eq!(ring.consumer_render_sample(), (0.5, true, false));
    }
    // Source loss and delayed real output are separate facts.
    assert_eq!(ring.consumer_render_sample(), (0.5, true, false));
    assert_eq!(ring.observe().missing_samples, 1);
    for _ in 1..510 {
        ring.consumer_render_sample();
    }
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    ring.producer_push(&[0.4]);
    ring.consumer_render_sample();
    assert_eq!(ring.observe().consecutive_shortfall_samples, 0);
    ring.consumer_reset().unwrap();
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
}

#[test]
fn actual_adapter_short_source_fifo_retains_full_output_pitch_history() {
    let ring = Ring::create(
        Settings {
            capacity: 640,
            input_rate_hz: 8000,
            output_rate_hz: 48000,
            reserve: 162,
            target: 320,
            max_producer: 160,
            max_output: 960,
            plc: PlcMode::G711AppendixI,
        },
        load_functions().unwrap(),
    )
    .unwrap();
    let mut output = [0.0; 960];
    for _ in 0..2 {
        ring.producer_push(&[0.5; 160]);
    }
    for _ in 0..12 {
        ring.producer_push(&[0.5; 160]);
        ring.consumer_render(&mut output);
    }
    assert_eq!(ring.observe().missing_samples, 0);
    let mut found = false;
    for _ in 0..10000 {
        let (sample, _, failed) = ring.consumer_render_sample();
        assert!(!failed);
        if ring.observe().consecutive_shortfall_samples == 1621 {
            // 1440 synthesized samples plus the 180-sample lookahead: 30 ms loss.
            assert!((sample - 0.3).abs() < 0.002, "{sample}");
            found = true;
            break;
        }
    }
    assert!(found);
}

#[test]
fn actual_adapter_partitioning_preserves_loss_recovery_and_reset() {
    let config = Settings {
        capacity: 640,
        input_rate_hz: 8000,
        output_rate_hz: 48000,
        reserve: 162,
        target: 320,
        max_producer: 160,
        max_output: 960,
        plc: PlcMode::G711AppendixI,
    };
    let a = Ring::create(config, load_functions().unwrap()).unwrap();
    let b = Ring::create(config, load_functions().unwrap()).unwrap();
    for block in 0..40 {
        if block % 9 != 5 {
            let input = core::array::from_fn::<_, 160, _>(|i| {
                (((block * 160 + i) as f32) * 0.17).sin() * 0.5
            });
            assert_eq!(a.producer_push(&input), b.producer_push(&input));
        }
        let mut expected = [0.0; 960];
        let (expected_real, expected_error) = a.consumer_render(&mut expected);
        let mut actual = [0.0; 960];
        let mut offset = 0;
        let mut real = 0;
        for count in [1, 7, 32, 128, 256, 536] {
            let (produced, failed) = b.consumer_render(&mut actual[offset..offset + count]);
            assert!(!failed);
            real += produced;
            offset += count;
        }
        assert!(!expected_error);
        assert_eq!(real, expected_real);
        assert_eq!(actual, expected);
        assert_eq!(a.observe().missing_samples, b.observe().missing_samples);
        assert_eq!(
            a.observe().ratio_correction_ppm,
            b.observe().ratio_correction_ppm
        );
        if block % 11 == 10 {
            a.consumer_reset().unwrap();
            b.consumer_reset().unwrap();
        }
    }
}

#[test]
fn actual_adapter_reset_removes_all_previous_burst_pcm() {
    let ring = Ring::create(settings(1024, 8000, 48000), load_functions().unwrap()).unwrap();
    ring.producer_push(&[0.75; 512]);
    let mut output = [0.0; 960];
    assert_eq!(ring.consumer_render(&mut output), (960, false));
    ring.consumer_reset().unwrap();
    ring.producer_push(&[0.0; 512]);
    assert_eq!(ring.consumer_render(&mut output), (960, false));
    assert_eq!(output, [0.0; 960]);
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn actual_adapter_reported_delay_aligns_the_impulse_peak() {
    for (input_rate, output_rate) in [
        (8000, 48000),
        (48000, 8000),
        (48000, 48000),
        (44100, 48000),
        (48000, 44100),
    ] {
        let ring = Ring::create(
            settings(4096, input_rate, output_rate),
            load_functions().unwrap(),
        )
        .unwrap();
        let mut input = [0.0; 4096];
        input[0] = 0.5;
        assert_eq!(ring.producer_push(&input), 4096);
        let mut output = [0.0; 2048];
        let (_, failed) = ring.consumer_render(&mut output);
        assert!(!failed);
        let peak = output
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .unwrap()
            .0;
        assert!(
            peak.abs_diff(ring.output_delay_samples() as usize) <= 1,
            "{input_rate}->{output_rate}: peak={peak}, delay={}",
            ring.output_delay_samples()
        );
    }
}

#[test]
fn callback_partitioning_does_not_change_priming_plc_or_recovery() {
    let config = Settings {
        reserve: 40,
        target: 128,
        max_producer: 128,
        max_output: 32,
        plc: PlcMode::G711AppendixI,
        ..settings(512, 8000, 8000)
    };
    let a = Ring::create(config, adapter(&FAKE_DESCRIPTOR)).unwrap();
    let b = Ring::create(config, adapter(&FAKE_DESCRIPTOR)).unwrap();
    for burst in 0..8 {
        if burst % 3 != 1 {
            a.producer_push(&[0.25; 128]);
            b.producer_push(&[0.25; 128]);
        }
        for count in [1, 7, 32, 16, 8, 32, 32] {
            let mut output = [0.0; 32];
            let (real, failed) = b.consumer_render(&mut output[..count]);
            let mut expected_real = 0;
            for &sample in &output[..count] {
                let (expected, is_real, error) = a.consumer_render_sample();
                assert_eq!(sample, expected);
                assert!(!error);
                expected_real += u64::from(is_real);
            }
            assert_eq!((real, failed), (expected_real, false));
        }
    }
    assert_eq!(a.observe().missing_samples, b.observe().missing_samples);
}

#[test]
fn rendering_drains_adapter_pcm_after_the_source_fifo_is_empty() {
    unsafe extern "C" fn create(
        _input_rate: u32,
        _output_rate: u32,
        _max_input: u32,
        _max_output: u32,
        output: *mut *mut c_void,
    ) -> c_int {
        unsafe { *output = Box::into_raw(Box::new(0_u32)).cast() };
        OK
    }
    unsafe extern "C" fn destroy(handle: *mut c_void) {
        unsafe { drop(Box::from_raw(handle.cast::<u32>())) };
    }
    unsafe extern "C" fn process(
        handle: *mut c_void,
        _input: *const f32,
        input_frames: u32,
        output: *mut f32,
        output_capacity: u32,
        _ratio: f64,
        used: *mut u32,
        generated: *mut u32,
    ) -> c_int {
        let pending = unsafe { &mut *handle.cast::<u32>() };
        *pending += input_frames;
        let count = if input_frames == 0 {
            *pending
        } else {
            *pending / 2
        }
        .min(output_capacity);
        unsafe {
            core::slice::from_raw_parts_mut(output, count as usize).fill(0.25);
            *used = input_frames;
            *generated = count;
        }
        *pending -= count;
        OK
    }
    let descriptor = AdapterDescriptor {
        converter_create: Some(create),
        converter_process: Some(process),
        converter_destroy: Some(destroy),
        ..FAKE_DESCRIPTOR
    };
    let ring = Ring::create(settings(512, 8000, 8000), adapter(&descriptor)).unwrap();
    assert_eq!(ring.producer_push(&[0.25; 128]), 128);
    let mut output = [0.0; 128];
    assert_eq!(ring.consumer_render(&mut output), (128, false));
    assert_eq!(output, [0.25; 128]);
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn producer_consumer_and_burst_reset_do_not_allocate() {
    for plc in [PlcMode::Disabled, PlcMode::G711AppendixI] {
        let ring = Ring::create(
            Settings {
                reserve: 40,
                target: 128,
                max_producer: 128,
                max_output: 32,
                plc,
                ..settings(512, 8000, 8000)
            },
            adapter(&FAKE_DESCRIPTOR),
        )
        .unwrap();
        let mut output = [0.0; 32];
        let before = ALLOCATION_COUNT.get();
        for i in 0..100 {
            if i % 3 != 1 {
                ring.producer_push(&[0.5; 128]);
            }
            for _ in 0..4 {
                ring.consumer_render(&mut output);
            }
        }
        ring.consumer_reset().unwrap();
        assert_eq!(ALLOCATION_COUNT.get(), before);
    }
}

#[test]
fn clock_controller_changes_ratio_only_after_an_observation_update() {
    let ring = Ring::create(
        Settings {
            target: 1,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    let input = [0.1; 16];
    assert_eq!(ring.producer_push(&input), 16);
    let mut first = [0.0; 8];
    assert_eq!(ring.consumer_render(&mut first).0, 8);
    assert_eq!(ring.producer_push(&input), 16);
    let mut second = [0.0; 16];
    assert_eq!(ring.consumer_render(&mut second).0, 16);
    assert!(ring.observe().ratio_correction_ppm < 0);
}

#[test]
fn construction_rejects_invalid_parameters() {
    let first_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(settings(0, 8_000, 8_000), first_adapter).is_err());
    let second_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(settings(512, 0, 8_000), second_adapter).is_err());
    let third_adapter = adapter(&FAKE_DESCRIPTOR);
    assert!(Ring::create(settings(512, 8_000, 0), third_adapter).is_err());
}

#[test]
fn construction_handles_control_plane_allocation_failure_without_panicking() {
    let too_large = Ring::create(
        settings(usize::MAX, 8_000, 8_000),
        adapter(&FAKE_DESCRIPTOR),
    );
    assert!(matches!(too_large, Err(CreateError::Invalid)));
    assert!(allocate_samples(usize::MAX).is_err());

    for occurrence in 1..=2 {
        let no_memory = fail_allocation_at(512 * size_of::<f32>(), occurrence, || {
            Ring::create(settings(512, 8_000, 8_000), adapter(&FAKE_DESCRIPTOR))
        });
        assert!(matches!(no_memory, Err(CreateError::NoMemory)));
    }

    let no_plc_memory = fail_allocation(390 * size_of::<f32>(), || {
        Ring::create(
            Settings {
                reserve: 40,
                target: 128,
                max_producer: 128,
                max_output: 32,
                plc: PlcMode::G711AppendixI,
                ..settings(512, 8000, 8000)
            },
            adapter(&FAKE_DESCRIPTOR),
        )
    });
    assert!(matches!(no_plc_memory, Err(CreateError::NoMemory)));

    let ring = ring(512);
    assert!(crate::allocate_ring_handle_with(ring, null_allocator).is_err());
}

#[test]
fn descriptor_rejects_an_unselected_adapter_capability() {
    assert!(unsafe { AdapterFunctions::from_descriptor(&WRONG_CAPABILITY_DESCRIPTOR) }.is_err());
}

#[test]
fn converter_failure_is_visible_without_skipping_concealment() {
    let ring = Ring::create(settings(512, 8_000, 8_000), adapter(&FAILING_DESCRIPTOR))
        .expect("test ring allocation");
    assert_eq!(ring.producer_push(&[0.4]), 1);
    let (_, real, adapter_error) = ring.consumer_render_sample();
    assert!(!real);
    assert!(adapter_error);
    assert_eq!(ring.observe().adapter_error_count, 1);
    assert_eq!(ring.observe().missing_samples, 1);
}

#[test]
fn zero_progress_conversion_gathers_more_then_reports_a_fault() {
    let ring = Ring::create(
        settings(512, 8_000, 8_000),
        adapter(&ZERO_PROGRESS_DESCRIPTOR),
    )
    .expect("test ring allocation");
    assert_eq!(ring.producer_push(&vec![0.2; 512]), 512);
    let (_, first_real, first_error) = ring.consumer_render_sample();
    assert!(!first_real);
    assert!(!first_error);
    let (_, second_real, second_error) = ring.consumer_render_sample();
    assert!(!second_real);
    assert!(second_error);
    assert_eq!(ring.observe().adapter_error_count, 1);
}

#[test]
fn controller_clamps_the_adapter_ratio_at_its_documented_boundary() {
    let ring = Ring::create(settings(512, 1, 256), adapter(&FAKE_DESCRIPTOR))
        .expect("boundary-rate ring allocation");
    assert_eq!(ring.producer_push(&vec![0.1; 512]), 512);
    let mut output = [0.0; 2];
    assert_eq!(ring.consumer_render(&mut output).0, 2);
    assert_eq!(ring.observe().adapter_error_count, 0);
    assert!(ring.observe().ratio_correction_ppm <= 0);
}

#[test]
fn partial_block_acceptance_preserves_the_chronological_prefix() {
    let ring = ring(512);
    assert_eq!(ring.producer_push(&vec![0.1; 510]), 510);
    assert_eq!(ring.producer_push(&[0.2, 0.3, 0.4, 0.5]), 2);
    let mut output = vec![0.0; 512];
    assert_eq!(ring.consumer_render(&mut output).0, 512);
    assert!(output[..510].iter().all(|sample| *sample == 0.1));
    assert_eq!(&output[510..], &[0.2, 0.3]);
    assert_eq!(ring.observe().discarded_samples, 2);
}

#[test]
fn refill_compacts_pending_input_after_partial_zero_output_conversion() {
    let ring = Ring::create(
        settings(512, 8_000, 8_000),
        adapter(&CONSUME_ONE_ZERO_OUTPUT_DESCRIPTOR),
    )
    .expect("test ring allocation");
    assert_eq!(ring.producer_push(&vec![0.2; 512]), 512);
    for _ in 0..258 {
        let (_, real, adapter_error) = ring.consumer_render_sample();
        assert!(!real);
        assert!(!adapter_error);
    }
    assert_eq!(ring.available(), 0);
    assert_eq!(ring.observe().adapter_error_count, 0);
}

#[test]
fn controller_clamps_at_both_documented_ratio_limits() {
    let lower = Ring::create(
        Settings {
            target: 1,
            ..settings(512, 256, 1)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .expect("lower-bound ring allocation");
    assert_eq!(lower.producer_push(&vec![0.2; 512]), 512);
    let mut lower_first = vec![0.0; 256];
    assert_eq!(lower.consumer_render(&mut lower_first).0, 256);
    assert_eq!(lower.producer_push(&vec![0.2; 256]), 256);
    let (_, lower_real, lower_error) = lower.consumer_render_sample();
    assert!(lower_real);
    assert!(!lower_error);
    assert_eq!(lower.observe().ratio_correction_ppm, 0);

    let upper = Ring::create(
        Settings {
            target: 512,
            ..settings(512, 1, 256)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .expect("upper-bound ring allocation");
    assert_eq!(upper.producer_push(&vec![0.2; 512]), 512);
    let mut upper_first = vec![0.0; 256];
    assert_eq!(upper.consumer_render(&mut upper_first).0, 256);
    assert_eq!(upper.producer_push(&vec![0.2; 256]), 256);
    let (_, upper_real, upper_error) = upper.consumer_render_sample();
    assert!(upper_real);
    assert!(!upper_error);
    assert_eq!(upper.observe().ratio_correction_ppm, 0);
}

#[test]
fn adapter_validation_rejects_each_required_descriptor_component() {
    assert!(unsafe { AdapterFunctions::from_descriptor(ptr::null()) }.is_err());

    let undersized = AdapterDescriptor {
        struct_size: 0,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&undersized) }.is_err());

    let incompatible_abi = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 1,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&incompatible_abi) }.is_err());

    let missing_capability = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: ptr::null(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_capability) }.is_err());

    let missing_create = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: None,
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_create) }.is_err());

    let missing_reset = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: None,
        converter_process: Some(fake_process),
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_reset) }.is_err());

    let missing_process = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: None,
        converter_destroy: Some(fake_destroy),
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_process) }.is_err());

    let missing_destroy = AdapterDescriptor {
        struct_size: size_of::<AdapterDescriptor>() as u32,
        abi_version: 2,
        capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
        converter_create: Some(fake_create),
        converter_reset: Some(fake_reset),
        converter_process: Some(fake_process),
        converter_destroy: None,
        converter_queued_input: Some(fake_queued_input),
        converter_output_delay: Some(fake_queued_input),
        converter_input_delay: Some(fake_input_delay),
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_destroy) }.is_err());

    let missing_queue = AdapterDescriptor {
        converter_queued_input: None,
        ..FAKE_DESCRIPTOR
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_queue) }.is_err());
    let missing_delay = AdapterDescriptor {
        converter_output_delay: None,
        ..FAKE_DESCRIPTOR
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&missing_delay) }.is_err());
}

#[test]
fn adapter_binding_accepts_the_nominal_rate_abi() {
    let descriptor = AdapterDescriptor {
        abi_version: 2,
        ..FAKE_DESCRIPTOR
    };
    assert!(unsafe { AdapterFunctions::from_descriptor(&descriptor) }.is_ok());
}

#[test]
fn descriptor_exposes_intrinsic_output_delay_for_finite_media() {
    assert!(descriptor().struct_size >= 88);
    unsafe extern "C" fn delay(_handle: *mut c_void, frames: *mut u32) -> c_int {
        unsafe { *frames = 77 };
        OK
    }
    let handle = handle_with(adapter(&AdapterDescriptor {
        converter_output_delay: Some(delay),
        ..FAKE_DESCRIPTOR
    }));
    let mut samples = 123;
    let query = descriptor().ring_output_delay;
    assert_eq!(query(ptr::null(), &mut samples), RESULT_INVALID_ARGUMENT);
    assert_eq!(samples, 123);
    assert_eq!(
        query(handle.as_ptr(), ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(query(handle.as_ptr(), &mut samples), RESULT_OK);
    assert_eq!(samples, 77);
    assert_eq!(
        (descriptor().ring_consumer_reset)(handle.as_ptr()),
        RESULT_OK
    );
    assert_eq!(query(handle.as_ptr(), &mut samples), RESULT_OK);
    assert_eq!(samples, 77);
}

#[test]
fn output_delay_failure_rejects_ring_creation() {
    unsafe extern "C" fn delay(_handle: *mut c_void, _frames: *mut u32) -> c_int {
        ERROR
    }
    assert!(matches!(
        Ring::create(
            settings(512, 8000, 8000),
            adapter(&AdapterDescriptor {
                converter_output_delay: Some(delay),
                ..FAKE_DESCRIPTOR
            })
        ),
        Err(CreateError::Adapter)
    ));
}

#[test]
fn controller_counts_adapter_backlog_in_input_samples() {
    unsafe extern "C" fn queued(_handle: *mut c_void, frames: *mut u32) -> c_int {
        unsafe { *frames = 128 };
        OK
    }
    let descriptor = AdapterDescriptor {
        converter_queued_input: Some(queued),
        ..FAKE_DESCRIPTOR
    };
    let ring = Ring::create(settings(512, 8000, 8000), adapter(&descriptor)).unwrap();
    ring.producer_push(&[0.25]);
    assert_eq!(ring.consumer_render_sample(), (0.25, true, false));
    assert_eq!(ring.observe().filtered_occupancy_samples, 129);
    assert_eq!(ring.observe().available_samples, 0);
}

#[test]
fn empty_playout_does_not_accelerate_the_clock_controller() {
    let ring = Ring::create(
        Settings {
            target: 128,
            ..settings(512, 8000, 8000)
        },
        adapter(&FAKE_DESCRIPTOR),
    )
    .unwrap();
    let mut output = [0.0; 256];
    ring.producer_push(&[0.25; 512]);
    ring.consumer_render(&mut output);
    ring.consumer_render(&mut output);
    let before = ring.observe();
    for _ in 0..20 {
        assert_eq!(ring.consumer_render(&mut output), (0, false));
    }
    let after = ring.observe();
    assert_eq!(after.ratio_correction_ppm, before.ratio_correction_ppm);
    assert_eq!(
        after.filtered_occupancy_samples,
        before.filtered_occupancy_samples
    );
}

#[test]
fn adapter_queue_failure_is_reported_as_a_converter_fault() {
    unsafe extern "C" fn queued(_handle: *mut c_void, _frames: *mut u32) -> c_int {
        ERROR
    }
    let descriptor = AdapterDescriptor {
        converter_queued_input: Some(queued),
        ..FAKE_DESCRIPTOR
    };
    let ring = Ring::create(settings(512, 8000, 8000), adapter(&descriptor)).unwrap();
    ring.producer_push(&[0.25]);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.observe().adapter_error_count, 1);
}

#[test]
fn ring_supplies_nominal_rates_and_internal_workspace_bounds() {
    unsafe extern "C" fn create(
        input_rate: u32,
        output_rate: u32,
        max_input: u32,
        max_output: u32,
        handle: *mut *mut c_void,
    ) -> c_int {
        if (input_rate, output_rate, max_input, max_output) != (8000, 48000, 640, 256) {
            return ERROR;
        }
        unsafe { fake_create(input_rate, output_rate, max_input, max_output, handle) }
    }
    let descriptor = AdapterDescriptor {
        converter_create: Some(create),
        ..FAKE_DESCRIPTOR
    };
    assert!(Ring::create(settings(640, 8000, 48000), adapter(&descriptor)).is_ok());
}

#[test]
fn adapter_converter_lifecycle_validates_callback_results() {
    let mut converter = adapter(&FAKE_DESCRIPTOR)
        .create_converter(8000, 8000, 512, 512)
        .expect("fake converter creation");
    let mut output = [0.0; 2];
    assert_eq!(
        converter.process(&[0.1, -0.2], &mut output, 1.0),
        Ok((2, 2))
    );
    assert_eq!(output, [0.1, -0.2]);
    assert!(converter.reset().is_ok());
    assert_eq!(converter.process(&[], &mut output, 1.0), Ok((0, 0)));
    assert!(converter.process(&[0.1], &mut [], 1.0).is_err());
    assert!(converter.process(&[0.1], &mut output, 0.0).is_err());
    assert!(converter.process(&[0.1], &mut output, f64::NAN).is_err());
    assert!(
        adapter(&CREATE_FAILURE_DESCRIPTOR)
            .create_converter(8000, 8000, 512, 512)
            .is_err()
    );
    assert!(
        adapter(&NULL_HANDLE_DESCRIPTOR)
            .create_converter(8000, 8000, 512, 512)
            .is_err()
    );
    assert!(
        adapter(&RESET_FAILURE_DESCRIPTOR)
            .create_converter(8000, 8000, 512, 512)
            .is_err()
    );

    let mut invalid_counts = adapter(&INVALID_COUNT_DESCRIPTOR)
        .create_converter(8000, 8000, 512, 512)
        .expect("invalid-count converter creation");
    assert!(invalid_counts.process(&[0.1], &mut output, 1.0).is_err());

    let mut invalid_output_counts = adapter(&INVALID_OUTPUT_COUNT_DESCRIPTOR)
        .create_converter(8000, 8000, 512, 512)
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
        .create_converter(8000, 8000, 512, 512)
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
fn public_descriptor_exposes_the_complete_abi_v3_function_table() {
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
    assert!(descriptor.ring_consumer_reset as usize != 0);
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
    let mut output = ptr::NonNull::<Rpcr3Ring>::dangling().as_ptr();
    assert_eq!(
        ring_create_with_adapter(ptr::null(), &mut output, Ok(adapter(&FAKE_DESCRIPTOR))),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(output, ptr::NonNull::<Rpcr3Ring>::dangling().as_ptr());
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
    invalid.plc_mode = 3;
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

    // Validated construction clears the output before entering this helper.
    output = ptr::null_mut();
    assert_eq!(
        finish_ring_create(
            ptr::NonNull::from(&mut output),
            Err(CreateError::Invalid),
            std::alloc::alloc
        ),
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
fn descriptor_reset_validates_handles_and_reports_adapter_failure() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static FAIL: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn reset(_converter: *mut c_void) -> c_int {
        if FAIL.swap(false, Ordering::Relaxed) {
            -1
        } else {
            0
        }
    }
    let descriptor = descriptor();
    let handle = handle_with(adapter(&AdapterDescriptor {
        converter_reset: Some(reset),
        ..FAKE_DESCRIPTOR
    }));
    assert_eq!(
        (descriptor.ring_consumer_reset)(ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!((descriptor.ring_consumer_reset)(handle.as_ptr()), RESULT_OK);
    FAIL.store(true, Ordering::Relaxed);
    assert_eq!(
        (descriptor.ring_consumer_reset)(handle.as_ptr()),
        RESULT_ADAPTER_ERROR
    );
    assert_eq!((descriptor.ring_consumer_reset)(handle.as_ptr()), RESULT_OK);
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
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), ptr::null_mut(), &mut real),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, ptr::null_mut()),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(ptr::null_mut(), &mut sample, &mut real),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, &mut real),
        RESULT_OK
    );
    assert!(!real);
    assert_eq!(
        (descriptor.ring_consumer_render)(handle.as_ptr(), ptr::null_mut(), 1, &mut real_samples),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(ptr::null_mut(), one.as_mut_ptr(), 1, &mut real_samples),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(
            handle.as_ptr(),
            one.as_mut_ptr(),
            too_large,
            &mut real_samples
        ),
        RESULT_INVALID_ARGUMENT
    );
    assert_eq!(
        (descriptor.ring_consumer_render)(handle.as_ptr(), ptr::null_mut(), 0, &mut real_samples),
        RESULT_OK
    );
    assert_eq!(real_samples, 0);
    assert_eq!(
        (descriptor.ring_consumer_render)(handle.as_ptr(), one.as_mut_ptr(), 1, ptr::null_mut()),
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
        (descriptor.ring_consumer_render_sample)(handle.as_ptr(), &mut sample, &mut real),
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
            &mut real_samples
        ),
        RESULT_ADAPTER_ERROR
    );
    assert_eq!(real_samples, 0);
    assert!(output.into_iter().all(f32::is_finite));
}
