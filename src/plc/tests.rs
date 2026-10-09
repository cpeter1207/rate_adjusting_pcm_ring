//! Signal-based regressions for Appendix I timing and transitions.

use super::Plc;

#[test]
fn recovery_continues_an_unfinished_period_expansion_overlap() {
    for (lost, gain, length) in [(81, 0.9975, 52.0), (161, 0.7975, 80.0)] {
        let mut plc = Plc::new(8000).unwrap();
        let end = plc.frozen.len();
        plc.frozen[end - 240..end - 160].fill(-0.2);
        plc.frozen[end - 160..end - 80].fill(0.2);
        plc.frozen[end - 80..].fill(0.6);
        plc.last_quarter[..20].fill(0.6);
        plc.pitch = 80;
        plc.cycle = 80;
        plc.smooth_cycle_tail();
        plc.lost = 1;
        plc.phase = 1;
        for _ in 1..lost {
            plc.conceal();
        }
        // The pending extension is 10% complete: 0.6*0.9+0.2*0.1=0.56.
        let expected = 0.56 * gain * ((length - 1.0) / length) - 0.25 / length;
        assert!(
            (plc.recover(-0.25) - expected).abs() < 0.00001,
            "lost={lost}"
        );
    }
}

#[test]
fn all_history_allocations_report_failure_without_panicking() {
    for (bytes, occurrence) in [(9360, 1), (9360, 2), (720, 1), (720, 2), (720, 3)] {
        assert!(crate::tests::fail_allocation_at(bytes, occurrence, || Plc::new(48_000)).is_err());
    }
}

#[test]
fn expansion_preserves_nonzero_phase_at_ten_milliseconds() {
    let mut plc = Plc::new(8000).unwrap();
    let end = plc.frozen.len();
    for i in 0..60 {
        plc.frozen[end - 60 + i] = 0.2 + 0.005 * i as f32;
        plc.frozen[end - 120 + i] = -0.4 + 0.005 * i as f32;
    }
    plc.pitch = 60;
    plc.cycle = 60;
    plc.last_quarter[..15].copy_from_slice(&plc.frozen[end - 15..]);
    plc.smooth_cycle_tail();
    plc.lost = 1;
    plc.phase = 1;
    for _ in 1..80 {
        plc.conceal();
    }
    assert!((plc.conceal() - 0.26).abs() < 0.00001);
    for _ in 81..94 {
        plc.conceal();
    }
    assert!((plc.conceal() + 0.22195).abs() < 0.00001);
    assert!((plc.conceal() + 0.2165625).abs() < 0.00001);
    // At 20 ms this pitch leaves phase100: subtract one period, not modulo.
    for _ in 96..161 {
        assert!(plc.conceal().is_finite());
    }
}

#[test]
fn returning_audio_is_saved_before_a_second_loss() {
    let mut plc = Plc::new(8000).unwrap();
    for _ in 0..800 {
        plc.process(Some(0.5));
    }
    plc.process(None);
    plc.pitch = 80;
    plc.cycle = 80;
    for _ in 1..160 {
        plc.process(None);
    }
    for _ in 0..26 {
        plc.process(Some(-0.25));
    }
    let end = plc.history.len();
    assert!((plc.history[(plc.next + end - 1) % end] - 0.075).abs() < 0.00001);
    plc.process(None);
    assert!((plc.last_quarter[plc.pitch / 4 - 1] - 0.075).abs() < 0.00001);
}

#[test]
fn reset_removes_prior_burst_and_pending_recovery() {
    let mut plc = Plc::new(48_000).unwrap();
    for _ in 0..4800 {
        plc.process(Some(0.6));
    }
    for _ in 0..100 {
        plc.process(None);
    }
    for _ in 0..10 {
        plc.process(Some(-0.3));
    }
    plc.reset();
    for _ in 0..180 {
        assert_eq!(plc.process(Some(0.1)), (0.0, false));
    }
    assert_eq!(plc.process(Some(0.1)), (0.1, true));
}

#[test]
fn empty_or_silent_history_produces_silence_without_nan() {
    for rate in [1, 8_000, 48_000] {
        let mut plc = Plc::new(rate).unwrap();
        for _ in 0..rate / 5 + 10 {
            assert_eq!(plc.process(None), (0.0, false));
        }
        for _ in 0..rate / 5 + 10 {
            assert_eq!(plc.process(Some(0.0)).0, 0.0);
        }
    }
}

#[test]
fn good_audio_has_exact_lookahead() {
    for (rate, delay) in [(8_000, 30), (48_000, 180)] {
        let mut plc = Plc::new(rate).unwrap();
        for _ in 0..delay {
            assert_eq!(plc.process(Some(0.25)), (0.0, false));
        }
        assert_eq!(plc.process(Some(0.25)), (0.25, true));
    }
}

#[test]
fn sustained_loss_uses_appendix_i_envelope() {
    for (rate, delay, ten_ms) in [(8_000, 30, 80), (48_000, 180, 480)] {
        let mut plc = Plc::new(rate).unwrap();
        for _ in 0..rate / 10 {
            plc.process(Some(0.5));
        }
        let lost: Vec<_> = (0..ten_ms * 7 + delay).map(|_| plc.process(None)).collect();
        for (offset, level) in [
            (0, 0.5),
            (ten_ms, 0.5),
            (2 * ten_ms, 0.4),
            (5 * ten_ms, 0.1),
            (6 * ten_ms, 0.0),
        ] {
            let (sample, real) = lost[delay + offset];
            assert!(
                (sample - level).abs() < 0.00001,
                "rate={rate}, offset={offset}, sample={sample}"
            );
            assert!(!real);
        }
    }
}

#[test]
fn pitch_continuation_preserves_a_100_hz_waveform() {
    for (rate, delay, period) in [(8_000, 30, 80), (48_000, 180, 480)] {
        let mut plc = Plc::new(rate).unwrap();
        let wave = |n: usize| (n as f64 * std::f64::consts::TAU / period as f64).sin() as f32 * 0.5;
        for n in 0..period * 10 {
            plc.process(Some(wave(n)));
        }
        for n in 0..period * 3 + delay {
            let (sample, real) = plc.process(None);
            if n >= delay {
                let position = n - delay;
                let gain = 1.0 - position.saturating_sub(period) as f32 / (period * 5) as f32;
                assert!(
                    (sample - wave(position) * gain).abs() < 0.015,
                    "rate={rate}, n={n}, sample={sample}"
                );
                assert!(!real);
            }
        }
    }
}

#[test]
fn recovery_overlaps_by_loss_duration_without_further_attenuation() {
    for (rate, delay, scale) in [(8_000, 30, 1), (48_000, 180, 6)] {
        for (lost, length, gain) in [(80, 20, 1.0), (160, 52, 0.8), (240, 80, 0.6)] {
            let mut plc = Plc::new(rate).unwrap();
            for _ in 0..rate / 10 {
                plc.process(Some(0.5));
            }
            plc.process(None);
            // Prescribe a 100 Hz period; the separate sine test exercises search.
            plc.pitch = 80 * scale;
            plc.cycle = plc.pitch;
            for _ in 1..lost * scale {
                plc.process(None);
            }
            let output: Vec<_> = (0..length * scale + delay)
                .map(|_| plc.process(Some(-0.25)))
                .collect();
            for i in [0, length * scale / 2 - 1, length * scale - 1] {
                let u = (i + 1) as f32 / (length * scale) as f32;
                let expected = 0.5 * gain * (1.0 - u) - 0.25 * u;
                assert!(
                    (output[delay + i].0 - expected).abs() < 0.00001,
                    "rate={rate}, lost={lost}, i={i}, actual={:?}",
                    output[delay + i]
                );
                assert!(output[delay + i].1);
            }
        }
    }
}

#[test]
fn pending_tail_and_period_extensions_use_complementary_overlap() {
    let mut plc = Plc::new(8_000).unwrap();
    plc.pitch = 80;
    plc.cycle = 80;
    let end = plc.frozen.len();
    plc.frozen[end - 240..end - 160].fill(-0.2);
    plc.frozen[end - 160..end - 80].fill(0.2);
    plc.frozen[end - 80..].fill(0.6);
    plc.last_quarter[..20].fill(0.6);
    plc.smooth_cycle_tail();
    plc.lost = 1;
    plc.phase = 1;
    for _ in 1..160 {
        plc.conceal();
    }
    assert!((plc.conceal() - 0.464).abs() < 0.00001);
}
