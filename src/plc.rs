//! G.711 Appendix I waveform substitution in the caller's output-rate domain.
//!
//! Pitch search uses the most recent 20 ms over 5–15 ms lags. A 3.75 ms
//! lookahead allows overlap-add into already received, not yet played PCM.
//! Loss expands the frozen waveform to two/three periods at 10/20 ms and
//! attenuates to silence at 60 ms. Storage depends on output rate, not FIFO size.

use crate::ring::allocate_samples;

/// Consumer-owned, preallocated waveform history and output lookahead.
pub(crate) struct Plc {
    history: Box<[f32]>,
    frozen: Box<[f32]>,
    last_quarter: Box<[f32]>,
    overlap: Box<[f32]>,
    real: Box<[f32]>,
    next: usize,
    delayed: usize,
    min_pitch: usize,
    max_pitch: usize,
    analysis: usize,
    ten_ms: usize,
    pitch: usize,
    cycle: usize,
    phase: usize,
    lost: usize,
    overlap_left: usize,
    recovery_left: usize,
    recovery_length: usize,
    recovery_gain: f32,
    four_ms: usize,
}

impl Plc {
    /// Discard delayed PCM and synthesis history between unrelated bursts.
    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.real.fill(0.0);
        self.next = 0;
        self.delayed = 0;
        self.lost = 0;
        self.recovery_left = 0;
        // The next loss replaces frozen history and all overlap state.
    }

    /// Allocate history once for a nonzero, previously validated output rate.
    pub(crate) fn new(output_rate_hz: u32) -> Result<Self, ()> {
        let delay = (u64::from(output_rate_hz) * 15).div_ceil(4000) as usize;
        let max_pitch = (u64::from(output_rate_hz) * 15).div_ceil(1000) as usize;
        let history = max_pitch * 3 + delay;
        Ok(Self {
            history: allocate_samples(history)?,
            frozen: allocate_samples(history)?,
            last_quarter: allocate_samples(delay)?,
            overlap: allocate_samples(delay)?,
            real: allocate_samples(delay)?,
            next: 0,
            delayed: 0,
            min_pitch: (u64::from(output_rate_hz) * 5).div_ceil(1000) as usize,
            max_pitch,
            analysis: (u64::from(output_rate_hz) * 20).div_ceil(1000) as usize,
            ten_ms: (u64::from(output_rate_hz) * 10).div_ceil(1000) as usize,
            pitch: 0,
            cycle: 0,
            phase: 0,
            lost: 0,
            overlap_left: 0,
            recovery_left: 0,
            recovery_length: 0,
            recovery_gain: 1.0,
            four_ms: (u64::from(output_rate_hz) * 4).div_ceil(1000) as usize,
        })
    }

    /// Delay one converted sample, substituting loss and classifying played PCM.
    /// Recovery blends count as real; initial delay and synthesized loss do not.
    pub(crate) fn process(&mut self, input: Option<f32>) -> (f32, bool) {
        let sample = match input {
            Some(sample) => self.recover(sample),
            None => self.conceal(),
        };
        let read = (self.next + self.history.len() - self.real.len()) % self.history.len();
        let output = (self.history[read], self.real[self.delayed] != 0.0);
        self.history[self.next] = sample;
        self.real[self.delayed] = f32::from(input.is_some());
        self.next = (self.next + 1) % self.history.len();
        self.delayed = (self.delayed + 1) % self.real.len();
        output
    }

    /// Compare recent 20 ms with 5–15 ms lags, first coarsely then at full rate.
    fn detect_pitch(&self) -> usize {
        let score = |lag: usize, step: usize| {
            let end = self.frozen.len();
            let mut correlation = 0.0_f64;
            let mut energy = 0.0_f64;
            for i in (end - self.analysis..end).step_by(step) {
                let a = f64::from(self.frozen[i]);
                let b = f64::from(self.frozen[i - lag]);
                correlation += a * b;
                energy += b * b;
            }
            if energy > 0.0 {
                correlation / energy.sqrt()
            } else {
                0.0
            }
        };
        let mut pitch = self.min_pitch;
        let mut best = f64::NEG_INFINITY;
        for lag in (self.min_pitch..=self.max_pitch).step_by(2) {
            let value = score(lag, 2);
            if value > best {
                best = value;
                pitch = lag;
            }
        }
        best = f64::NEG_INFINITY;
        let coarse = pitch;
        for lag in coarse.saturating_sub(1).max(self.min_pitch)..=(coarse + 1).min(self.max_pitch) {
            let value = score(lag, 1);
            if value > best {
                best = value;
                pitch = lag;
            }
        }
        pitch
    }

    /// Freeze the last 48.75 ms; pending lookahead permits a retrospective OLA.
    fn begin_loss(&mut self) {
        let end = self.history.len();
        for i in 0..end {
            self.frozen[i] = self.history[(self.next + i) % end];
        }
        self.pitch = self.detect_pitch();
        let quarter = self.pitch / 4;
        self.last_quarter[..quarter].copy_from_slice(&self.frozen[end - quarter..]);
        self.cycle = self.pitch;
        self.phase = 0;
        self.overlap_left = 0;
        self.smooth_cycle_tail();
        for i in 0..quarter {
            self.history[(self.next + end - quarter + i) % end] = self.frozen[end - quarter + i];
        }
    }

    /// Join the repeated waveform tail to samples one cycle earlier.
    fn smooth_cycle_tail(&mut self) {
        let end = self.frozen.len();
        let quarter = self.pitch / 4;
        for i in 0..quarter {
            self.frozen[end - quarter + i] = blend(
                self.last_quarter[i],
                self.frozen[end - self.cycle - quarter + i],
                i,
                quarter,
            );
        }
    }

    /// Emit the continuing frozen waveform without changing its oscillator phase.
    fn continuation(&mut self) -> f32 {
        let sample = self.frozen[self.frozen.len() - self.cycle + self.phase];
        self.phase = (self.phase + 1) % self.cycle;
        sample
    }

    /// Expand to two and three periods after 10 and 20 ms to reduce buzz.
    fn expand_cycle(&mut self) {
        let quarter = self.pitch / 4;
        let phase = self.phase;
        for i in 0..quarter {
            self.overlap[i] = self.continuation();
        }
        self.phase = phase;
        while self.phase > self.pitch {
            self.phase -= self.pitch;
        }
        self.cycle += self.pitch;
        self.smooth_cycle_tail();
        self.overlap_left = quarter;
    }

    /// Repeat pitch then linearly attenuate from 10 ms through silence at 60 ms.
    fn conceal(&mut self) -> f32 {
        self.recovery_left = 0;
        if self.lost == 0 {
            self.begin_loss();
        }
        if self.lost == self.ten_ms || self.lost == self.ten_ms * 2 {
            self.expand_cycle();
        }
        let sample = self.substitution();
        let gain = self.loss_gain();
        self.lost = self.lost.saturating_add(1);
        sample * gain
    }

    /// Finish a period-expansion overlap even if real PCM returns mid-transition.
    fn substitution(&mut self) -> f32 {
        let mut sample = self.continuation();
        if self.overlap_left != 0 {
            let quarter = self.pitch / 4;
            let i = quarter - self.overlap_left;
            sample = blend(self.overlap[i], sample, i, quarter);
            self.overlap_left -= 1;
        }
        sample
    }

    /// Freeze the loss envelope while returning audio replaces the repeated wave.
    fn recover(&mut self, sample: f32) -> f32 {
        if self.lost != 0 {
            let lost_frames = self.lost.div_ceil(self.ten_ms);
            self.recovery_length = (self.pitch / 4)
                .saturating_add(lost_frames.saturating_sub(1).saturating_mul(self.four_ms))
                .min(self.ten_ms);
            self.recovery_left = self.recovery_length;
            self.recovery_gain = self.loss_gain();
            self.lost = 0;
        }
        if self.recovery_left == 0 {
            return sample;
        }
        let old = self.substitution() * self.recovery_gain;
        let output = blend(
            old,
            sample,
            self.recovery_length - self.recovery_left,
            self.recovery_length,
        );
        self.recovery_left -= 1;
        output
    }

    /// Unity for the first 10 ms, then linear decay to silence at 60 ms.
    fn loss_gain(&self) -> f32 {
        (1.0 - self.lost.saturating_sub(self.ten_ms) as f32 / (self.ten_ms * 5) as f32).max(0.0)
    }
}

/// Appendix I uses linear overlap-add, not an equal-power gain boost.
fn blend(old: f32, new: f32, index: usize, count: usize) -> f32 {
    let weight = (index + 1) as f32 / count as f32;
    old * (1.0 - weight) + new * weight
}

#[cfg(test)]
mod tests;
