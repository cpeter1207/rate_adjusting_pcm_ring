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
                    missing: false
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
                missing: false
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
