use super::*;
use crate::samplerate_adapter::AdapterDescriptor;
use crate::tests::FAKE_DESCRIPTOR;
use core::ffi::{c_int, c_void};
use core::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

fn ring(capacity: usize) -> Ring {
    let adapter = unsafe { AdapterFunctions::from_descriptor(&FAKE_DESCRIPTOR) }.unwrap();
    Ring::create(crate::tests::settings(capacity, 8_000, 8_000), adapter).unwrap()
}

#[test]
fn finite_short_burst_drains_below_reserve_without_loss_then_reprimes() {
    let ring = Ring::create(
        Settings {
            reserve: 40,
            ..crate::tests::settings(512, 8000, 8000)
        },
        unsafe { AdapterFunctions::from_descriptor(&FAKE_DESCRIPTOR) }.unwrap(),
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.2, 0.4, 0.6]), 3);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    let token = ring.producer_finish().unwrap();
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Pending));
    assert!(!ring.producer_push_sample(0.8));
    assert_eq!(ring.producer_push(&[0.8; 3]), 0);
    assert_eq!(ring.producer_gap(3), 0);
    assert_eq!(
        ring.observe().discarded_samples,
        0,
        "finite-boundary backpressure is not loss"
    );
    assert!(ring.producer_finish().is_err());
    for sample in [0.2, 0.4, 0.6] {
        assert_eq!(ring.consumer_render_sample(), (sample, true, false));
    }
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Drained));
    assert_eq!(ring.observe().missing_samples, 0);
    for _ in 0..20 {
        assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    }
    assert_eq!(ring.observe().missing_samples, 0);
    assert_eq!(ring.producer_push(&[0.7]), 1);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    let next = ring.producer_finish().unwrap();
    assert!(next > token);
    assert!(ring.finish_status(token).is_err());
    assert_eq!(ring.consumer_render_sample(), (0.7, true, false));
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(next), Ok(FinishStatus::Drained));
}

#[test]
fn finite_tail_keeps_plc_lookahead_and_late_mute_without_inventing_loss() {
    let ring = Ring::create(
        Settings {
            reserve: 40,
            target: 128,
            max_producer: 128,
            max_output: 32,
            plc: PlcMode::G711AppendixI,
            ..crate::tests::settings(512, 8000, 8000)
        },
        unsafe { AdapterFunctions::from_descriptor(&FAKE_DESCRIPTOR) }.unwrap(),
    )
    .unwrap();
    assert_eq!(
        ring.producer_push_tagged(&[0.2, 0.4, 0.6], 9, 0, &[]),
        Ok(3)
    );
    let token = ring.producer_finish().unwrap();
    for _ in 0..3 {
        assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    }
    assert_eq!(ring.producer_mute(9, &[Span { start: 1, end: 2 }]), Ok(1));
    for _ in 3..30 {
        assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    }
    for expected in [0.2, 0.0, 0.6] {
        assert_eq!(ring.consumer_render_sample(), (expected, true, false));
    }
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Pending));
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Drained));
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn finite_reset_cancels_exact_token_and_tokens_never_wrap() {
    let ring = ring(513);
    assert_eq!(ring.producer_push(&[0.5; 10]), 10);
    let token = ring.producer_finish().unwrap();
    ring.consumer_reset().unwrap();
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Cancelled));
    assert_eq!(ring.available(), 0);
    let empty = ring.producer_finish().unwrap();
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(empty), Ok(FinishStatus::Drained));
    ring.finish.store(u64::MAX, Ordering::Relaxed);
    assert!(ring.producer_finish().is_err());
    assert!(ring.finish_status(0).is_err());
}

#[test]
fn finite_real_src_matches_continuing_zero_extension_across_rates_and_drift() {
    for rate in [8000, 48000] {
        for correction in [0.999, 1.0, 1.001] {
            for count in [1, 17, 640] {
                let config = Settings {
                    reserve: 700,
                    ..crate::tests::settings(2048, rate, 48000)
                };
                let ring =
                    Ring::create(config, crate::samplerate_adapter::load_functions().unwrap())
                        .unwrap();
                // Compare against the released converter directly. A second
                // ring would steer its occupancy correction while the finite
                // ring correctly freezes that ratio during zero extension.
                let mut reference = crate::samplerate_adapter::load_functions()
                    .unwrap()
                    .create_converter(rate, 48000, 2048, 256)
                    .unwrap();
                let mut reference_output = [0.0; 256];
                let mut reference_input = 0;
                let mut reference_offset = 0;
                let mut reference_count = 0;
                let mut input = [0.0; 2048];
                for (index, sample) in input[..count].iter_mut().enumerate() {
                    *sample = ((index as f32 + 1.0) * 0.17).sin() * 0.5;
                }
                assert_eq!(ring.producer_push(&input[..count]), count as u64);
                let token = ring.producer_finish().unwrap();
                let ratio = 48000.0 / f64::from(rate) * correction;
                let mut played = 0;
                while ring.finish_status(token) == Ok(FinishStatus::Pending) {
                    assert!(played < 4500, "finite converter never drained");
                    unsafe {
                        (*ring.consumer.get()).ratio = ratio;
                    }
                    let (actual, _, failed) = ring.consumer_render_sample();
                    assert!(!failed, "rate={rate}, count={count}, sample={played}");
                    if ring.finish_status(token) == Ok(FinishStatus::Pending) {
                        while reference_offset == reference_count {
                            let end = (reference_input + 256).min(input.len());
                            let (used, generated) = reference
                                .process(&input[reference_input..end], &mut reference_output, ratio)
                                .unwrap();
                            reference_input += used;
                            reference_count = generated;
                            reference_offset = 0;
                            assert!(used != 0 || generated != 0);
                        }
                        let expected = reference_output[reference_offset];
                        reference_offset += 1;
                        assert!(
                            (actual - expected).abs() < 0.00001,
                            "rate={rate}, count={count}, correction={correction}, sample={played}: {actual} != {expected}"
                        );
                        played += 1;
                    }
                }
                let expected = count as f64 * ratio + ring.output_delay_samples() as f64;
                assert!(
                    (played as f64 - expected).abs() <= 3.0,
                    "rate={rate}, count={count}: played={played}, expected={expected}"
                );
                assert_eq!(ring.observe().missing_samples, 0);
                assert_eq!(ring.observe().discarded_samples, 0);
            }
        }
    }
}

#[test]
fn finite_real_src_keeps_a_final_packet_gap_in_its_original_time_slot() {
    let ring = Ring::create(
        Settings {
            reserve: 700,
            ..crate::tests::settings(2048, 8000, 48000)
        },
        crate::samplerate_adapter::load_functions().unwrap(),
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.3; 600]), 600);
    assert_eq!(ring.producer_gap(40), 40);
    let token = ring.producer_finish().unwrap();
    let mut played = 0;
    while ring.finish_status(token) == Ok(FinishStatus::Pending) {
        assert!(played < 4500);
        assert!(!ring.consumer_render_sample().2);
        played += 1;
    }
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Drained));
    assert!(
        ring.observe().missing_samples.abs_diff(240) <= 1,
        "{}",
        ring.observe().missing_samples
    );
}

#[test]
fn finite_adapter_stall_reports_failed_completion_without_hanging() {
    let ring = Ring::create(
        crate::tests::settings(512, 8000, 8000),
        unsafe { AdapterFunctions::from_descriptor(&crate::tests::ZERO_PROGRESS_DESCRIPTOR) }
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.5; 3]), 3);
    let token = ring.producer_finish().unwrap();
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::AdapterError));
    assert_eq!(ring.observe().adapter_error_count, 1);
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn failed_finite_conversion_discards_the_whole_old_burst_before_reuse() {
    static FAIL_NEXT: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn process(
        converter: *mut c_void,
        input: *const f32,
        input_frames: u32,
        output: *mut f32,
        output_capacity: u32,
        ratio: f64,
        input_used: *mut u32,
        output_generated: *mut u32,
    ) -> c_int {
        if FAIL_NEXT.swap(false, Ordering::Relaxed) {
            return -1;
        }
        unsafe {
            FAKE_DESCRIPTOR.converter_process.unwrap()(
                converter,
                input,
                input_frames,
                output,
                output_capacity,
                ratio,
                input_used,
                output_generated,
            )
        }
    }
    let descriptor = AdapterDescriptor {
        converter_process: Some(process),
        ..FAKE_DESCRIPTOR
    };
    let ring = Ring::create(
        crate::tests::settings(1024, 8000, 8000),
        unsafe { AdapterFunctions::from_descriptor(&descriptor) }.unwrap(),
    )
    .unwrap();
    assert_eq!(ring.producer_push_tagged(&[0.2; 640], 1, 0, &[]), Ok(640));
    let failed = ring.producer_finish().unwrap();
    FAIL_NEXT.store(true, Ordering::Relaxed);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.finish_status(failed), Ok(FinishStatus::AdapterError));
    assert_eq!(
        ring.available(),
        0,
        "failed-burst FIFO suffix must retire before acknowledgement"
    );
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.producer_push_tagged(&[0.9; 3], 2, 0, &[]), Ok(3));
    let next = ring.producer_finish().unwrap();
    for _ in 0..3 {
        assert_eq!(ring.consumer_render_sample(), (0.9, true, false));
    }
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(next), Ok(FinishStatus::Drained));
    assert_eq!(ring.observe().adapter_error_count, 1);
    assert_eq!(ring.observe().missing_samples, 0);
}

#[test]
fn tagged_source_and_gap_metadata_allocation_failures_are_recoverable() {
    for bytes in [512 * size_of::<SourceSample>(), 1026 * size_of::<GapSpan>()] {
        let result = crate::tests::fail_allocation_at(bytes, 1, || {
            Ring::create(
                crate::tests::settings(512, 8000, 8000),
                unsafe { AdapterFunctions::from_descriptor(&FAKE_DESCRIPTOR) }.unwrap(),
            )
        });
        assert!(matches!(result, Err(CreateError::NoMemory)));
    }
}

#[test]
fn coverage_prepared_mute_and_plc_workspace_allocation_failures() {
    for bytes in [
        1026 * size_of::<crate::mute::Interval>(),
        mute::MAX_SPANS * 2 * size_of::<crate::mute::Interval>(),
        30 * size_of::<Option<Position>>(),
    ] {
        let result = crate::tests::fail_allocation_at(bytes, 1, || {
            Ring::create(
                Settings {
                    reserve: 40,
                    target: 128,
                    max_output: 32,
                    max_producer: 128,
                    plc: PlcMode::G711AppendixI,
                    ..crate::tests::settings(512, 8000, 8000)
                },
                unsafe { AdapterFunctions::from_descriptor(&FAKE_DESCRIPTOR) }.unwrap(),
            )
        });
        assert!(matches!(result, Err(CreateError::NoMemory)));
    }
}

#[test]
fn coverage_tagged_bounds_and_exhausted_source_positions_fail_without_publication() {
    let ring = ring(1024);
    let token = ring.producer_finish().unwrap();
    assert_eq!(ring.producer_push_tagged(&[0.2], 1, 0, &[]), Ok(0));
    assert_eq!(ring.finish_status(token), Ok(FinishStatus::Pending));
    ring.consumer_reset().unwrap();
    assert!(ring.producer_push_tagged(&[0.2; 1025], 1, 0, &[]).is_err());
    let too_many = [Span { start: 0, end: 1 }; mute::MAX_SPANS + 1];
    assert!(ring.producer_push_tagged(&[0.2], 1, 0, &too_many).is_err());
    assert!(ring.producer_push_tagged(&[0.2], 1, u64::MAX, &[]).is_err());
    assert!(
        ring.producer_push_tagged(&[0.2], 1, 0, &[Span { start: 0, end: 2 }])
            .is_err()
    );
    assert!(ring.producer_mute(1, &[]).is_err());
    assert!(
        ring.producer_push_tagged(&[0.2], 1, 2048, &[Span { start: 0, end: 1 }])
            .is_err()
    );
    assert_eq!(ring.producer_push_tagged(&[0.2], 1, 2048, &[]), Ok(1));
    for (generation, spans) in [
        (2, vec![]),
        (1, too_many.to_vec()),
        (
            1,
            vec![Span {
                start: 2048,
                end: 2048,
            }],
        ),
        (
            1,
            vec![Span {
                start: 2048,
                end: 2050,
            }],
        ),
        (1, vec![Span { start: 0, end: 1 }]),
    ] {
        assert!(ring.producer_mute(generation, &spans).is_err());
    }
    unsafe {
        *ring.producer_stamp.get() = Some(Stamp {
            generation: 1,
            source: u64::MAX,
        })
    };
    assert!(!ring.producer_push_sample(0.3));
    assert_eq!(ring.available(), 1);
}

#[test]
fn coverage_mute_history_exhaustion_and_source_fences_fail_closed() {
    let exhausted = ring(512);
    unsafe { (*exhausted.consumer.get()).mute_history = History::new(0).unwrap() };
    assert_eq!(
        exhausted.producer_push_tagged(&[0.2], 1, 0, &[Span { start: 0, end: 1 }]),
        Ok(1)
    );
    assert_eq!(exhausted.consumer_render_sample(), (0.0, false, true));
    assert_eq!(exhausted.consumer_reset(), Ok(()));
    assert!(exhausted.producer_push_sample(0.9));
    assert_eq!(exhausted.consumer_render_sample(), (0.9, true, false));
    for base in [(2, 0), (1, 100)] {
        let fenced = ring(512);
        unsafe { (*fenced.consumer.get()).source_base = Some(base) };
        assert_eq!(fenced.producer_push_tagged(&[0.2], 1, 0, &[]), Ok(1));
        assert_eq!(fenced.consumer_render_sample(), (0.0, false, true));
    }
    let changed = ring(512);
    assert_eq!(changed.producer_push_tagged(&[0.2], 1, 0, &[]), Ok(1));
    assert_eq!(changed.producer_push_tagged(&[0.3], 2, 0, &[]), Ok(1));
    assert_eq!(changed.consumer_render_sample(), (0.0, false, true));
}

#[test]
fn coverage_source_support_rejects_fractional_u64_overflow() {
    let ring = ring(512);
    let state = unsafe { &mut *ring.consumer.get() };
    state.source_base = Some((1, 0));
    state.cache_count = 1;
    state.cache_start = i128::from(u64::MAX) * 1_000_000_000 + 1;
    state.cache_end = state.cache_start;
    assert!(state.cache_position().is_none());
    state.cache_start = 0;
    state.cache_end = 0;
    assert_eq!(
        state
            .cache_position()
            .map(|p| (p.generation, p.low, p.high)),
        Some((1, 0, 0))
    );
}

#[test]
fn coverage_finite_reset_failure_is_not_a_drained_or_cancelled_acknowledgement() {
    static FAIL_RESET: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn reset(converter: *mut c_void) -> c_int {
        if FAIL_RESET.swap(false, Ordering::Relaxed) {
            return -1;
        }
        unsafe { FAKE_DESCRIPTOR.converter_reset.unwrap()(converter) }
    }
    let descriptor = AdapterDescriptor {
        converter_reset: Some(reset),
        ..FAKE_DESCRIPTOR
    };
    let ring = Ring::create(
        crate::tests::settings(512, 8000, 8000),
        unsafe { AdapterFunctions::from_descriptor(&descriptor) }.unwrap(),
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.2]), 1);
    let failed = ring.producer_finish().unwrap();
    assert_eq!(ring.consumer_render_sample(), (0.2, true, false));
    FAIL_RESET.store(true, Ordering::Relaxed);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.finish_status(failed), Ok(FinishStatus::AdapterError));
    assert_eq!(ring.consumer_reset(), Ok(()));
    assert_eq!(ring.finish_status(failed), Ok(FinishStatus::AdapterError));
    assert_eq!(ring.producer_push(&[0.9]), 1);
    let next = ring.producer_finish().unwrap();
    assert_eq!(ring.consumer_render_sample(), (0.9, true, false));
    assert_eq!(ring.consumer_render_sample(), (0.0, false, false));
    assert_eq!(ring.finish_status(next), Ok(FinishStatus::Drained));
    assert_eq!(ring.consumer_reset(), Ok(()));
    assert_eq!(ring.finish_status(next), Ok(FinishStatus::Drained));
}

#[test]
fn coverage_converter_delay_faults_and_backwards_positions_fail_closed() {
    thread_local! { static FAULT: std::cell::Cell<(u32, usize)> = const { std::cell::Cell::new((0, 0)) }; }
    unsafe extern "C" fn delay(_: *mut c_void, output: *mut i64) -> c_int {
        let (mode, call) = FAULT.with(|fault| {
            let current = fault.get();
            fault.set((current.0, current.1 + 1));
            current
        });
        if (mode == 1 && call == 0) || (mode == 2 && call == 1) {
            return -1;
        }
        unsafe {
            *output = if mode == 3 && call == 1 {
                999_000_000_000
            } else {
                0
            }
        };
        0
    }
    for mode in 1..=3 {
        FAULT.with(|fault| fault.set((0, 0)));
        let descriptor = AdapterDescriptor {
            converter_input_delay: Some(delay),
            ..FAKE_DESCRIPTOR
        };
        let ring = Ring::create(
            crate::tests::settings(512, 8000, 8000),
            unsafe { AdapterFunctions::from_descriptor(&descriptor) }.unwrap(),
        )
        .unwrap();
        assert!(ring.producer_push_sample(0.2));
        FAULT.with(|fault| fault.set((mode, 0)));
        assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
        FAULT.with(|fault| fault.set((0, 0)));
        assert_eq!(ring.consumer_reset(), Ok(()));
    }
}

#[test]
fn exhausted_gap_metadata_reports_a_fault_without_allocating() {
    let ring = ring(512);
    {
        let state = unsafe { &mut *ring.consumer.get() };
        // A provider retaining beyond its declared queue bound must not make
        // the consumer grow its metadata storage on an audio callback.
        for index in 0..state.gaps.capacity() {
            state.gaps.push_back(GapSpan {
                start: index as u128 * 2,
                end: index as u128 * 2 + 1,
            });
        }
    }
    assert_eq!(ring.producer_gap(1), 1);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.consumer_reset(), Ok(()));
    assert!(ring.producer_push_sample(0.5));
    assert_eq!(ring.consumer_render_sample(), (0.5, true, false));
}

#[test]
fn non_power_of_two_cursor_wrap_preserves_unread_pcm() {
    let ring = ring(513);
    ring.written.store(1_024, Ordering::Relaxed);
    ring.read.store(1_024, Ordering::Relaxed);
    let input: Vec<f32> = (0..513).map(|index| index as f32 / 513.0).collect();
    for _ in 0..4 {
        assert_eq!(ring.producer_push(&input), 513);
        assert_eq!(ring.available(), 513);
        assert!(!ring.producer_push_sample(-0.5));
        assert!(ring.written.load(Ordering::Relaxed) < 1_026);
        for &expected in &input {
            assert_eq!(
                ring.consumer_take_source_sample(),
                Some(SourceSample {
                    pcm: expected,
                    missing: false,
                    stamp: None,
                })
            );
        }
        assert_eq!(ring.consumer_take_source_sample(), None);
        assert_eq!(ring.available(), 0);
        assert!(ring.read.load(Ordering::Relaxed) < 1_026);
    }
}

#[test]
fn real_gap_mapping_survives_partial_adapter_input_wrap_and_ppm_changes() {
    for rate in [8000, 48000] {
        for correction in [0.999, 1.0, 1.001] {
            let ring = Ring::create(
                crate::tests::settings(513, rate, 48000),
                crate::samplerate_adapter::load_functions().unwrap(),
            )
            .unwrap();
            let intervals = [(80, 97), (480, 540), (900, 905), (1024, 1104)];
            let mut source = 0;
            let mut missing = 0;
            let mut saw_pending = false;
            let mut saw_converter_backlog = false;
            let nominal = 48000.0 / f64::from(rate);
            let frames = (1500.0 * nominal) as usize;
            for index in 0..frames {
                // Keep following PCM available; producer publications wrap the
                // non-power-of-two FIFO while SWR retains accepted history.
                while source < 2000 && ring.available() < 513 {
                    let is_gap = intervals
                        .iter()
                        .any(|&(start, end)| (start..end).contains(&source));
                    assert!(if is_gap {
                        ring.producer_gap(1) == 1
                    } else {
                        ring.producer_push_sample(0.5)
                    });
                    source += 1;
                }
                {
                    let state = unsafe { &mut *ring.consumer.get() };
                    state.ratio = nominal * correction;
                    saw_pending |= state.input_pending != 0;
                    saw_converter_backlog |= state.converter.queued_input().unwrap() != 0;
                }
                let (_, real, failed) = ring.consumer_render_sample();
                assert!(
                    !failed,
                    "rate={rate}, correction={correction}, output={index}"
                );
                missing += usize::from(!real);
            }
            let expected = 162.0 * nominal * correction;
            assert!(
                (missing as f64 - expected).abs() <= 4.0,
                "rate={rate}, correction={correction}: missing={missing}, expected={expected}"
            );
            assert!(saw_pending);
            assert!(saw_converter_backlog);
            assert!(unsafe { &*ring.consumer.get() }.gaps.is_empty());
        }
    }
}

#[test]
fn concurrent_spsc_preserves_pcm_across_cursor_wraps() {
    let ring = Arc::new(ring(513));
    let producer = Arc::clone(&ring);
    let worker = thread::spawn(move || {
        for index in 0..10_000 {
            let sample = index as f32 / 10_000.0;
            while !producer.producer_push_sample(sample) {
                thread::yield_now();
            }
        }
    });
    for index in 0..10_000 {
        let actual = loop {
            if let Some(sample) = ring.consumer_take_source_sample() {
                break sample;
            }
            thread::yield_now();
        };
        assert_eq!(
            actual,
            SourceSample {
                pcm: index as f32 / 10_000.0,
                missing: false,
                stamp: None,
            }
        );
    }
    worker.join().unwrap();
    assert_eq!(ring.available(), 0);
}

#[test]
fn failed_reset_silences_cached_and_new_pcm_until_successful_reset() {
    static FAIL_RESET: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn reset(_converter: *mut c_void) -> c_int {
        if FAIL_RESET.swap(false, Ordering::Relaxed) {
            -1
        } else {
            0
        }
    }
    let descriptor = AdapterDescriptor {
        converter_reset: Some(reset),
        ..FAKE_DESCRIPTOR
    };
    let adapter = unsafe { AdapterFunctions::from_descriptor(&descriptor) }.unwrap();
    let ring = Ring::create(
        Settings {
            reserve: 2,
            ..crate::tests::settings(513, 8_000, 8_000)
        },
        adapter,
    )
    .unwrap();
    assert_eq!(ring.producer_push(&[0.7; 16]), 16);
    let mut output = [0.0];
    assert_eq!(ring.consumer_render(&mut output), (1, false));
    assert_eq!(output, [0.7]);

    FAIL_RESET.store(true, Ordering::Relaxed);
    assert_eq!(ring.consumer_reset(), Err(()));
    assert_eq!(ring.available(), 0);
    assert_eq!(ring.consumer_render(&mut output), (0, true));
    assert_eq!(output, [0.0]);
    assert_eq!(ring.producer_push(&[0.9]), 1);
    assert_eq!(ring.consumer_render_sample(), (0.0, false, true));
    assert_eq!(ring.available(), 1);

    assert_eq!(ring.consumer_reset(), Ok(()));
    assert_eq!(ring.available(), 0);
    assert_eq!(ring.producer_push(&[0.2]), 1);
    assert_eq!(ring.consumer_render(&mut output), (0, false));
    assert_eq!(ring.producer_push(&[0.3]), 1);
    assert_eq!(ring.consumer_render(&mut output), (1, false));
    assert_eq!(output, [0.2]);
}
