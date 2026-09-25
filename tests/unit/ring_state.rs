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
            assert_eq!(ring.consumer_take_source_sample(), Some(expected));
        }
        assert_eq!(ring.consumer_take_source_sample(), None);
        assert_eq!(ring.available(), 0);
        assert!(ring.read.load(Ordering::Relaxed) < 1_026);
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
        assert_eq!(actual, index as f32 / 10_000.0);
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
