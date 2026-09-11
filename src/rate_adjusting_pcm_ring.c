/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Persistent sinc conversion and slow SPSC-ring clock recovery.
 */
#include "rate_adjusting_pcm_ring.h"
#include <math.h>
#include <samplerate.h>
#include <stdlib.h>

/** @cond INTERNAL
 * Reject targets whose diagnostic or cursor atomics could take a hidden
 * library lock in the audio callback.
 */
_Static_assert(ATOMIC_INT_LOCK_FREE == 2 && ATOMIC_LONG_LOCK_FREE == 2 &&
                   ATOMIC_LLONG_LOCK_FREE == 2,
               "real-time PCM ring requires lock-free callback atomics");
/** @endcond */

/** @brief Lowest voiced fundamental used by generic speech concealment. */
#define RPCR_PLC_MIN_PITCH_HZ 60U
/** @brief Highest voiced fundamental used by generic speech concealment. */
#define RPCR_PLC_MAX_PITCH_HZ 400U
/** @brief Analysis span for pitch matching. */
#define RPCR_PLC_ANALYSIS_MS 5U
/** @brief Equal-power fade between real and synthesized waveform segments. */
#define RPCR_PLC_CROSSFADE_MS 8U
/** @brief Keep a synthesized waveform at full level before fading it. */
#define RPCR_PLC_HOLD_MS 10U
/** @brief End a sustained erasure by fading rather than endlessly repeating. */
#define RPCR_PLC_FADE_MS 60U

/** @brief Convert a public quality selection to its libsamplerate constant.
 *
 * The selected sinc converter supplies all anti-aliasing; the ring has no
 * parallel filtering implementation.
 */
static int converter_type(enum rpcr_quality quality) {
  switch (quality) {
  case RPCR_SINC_BEST:
    return SRC_SINC_BEST_QUALITY;
  case RPCR_SINC_MEDIUM:
    return SRC_SINC_MEDIUM_QUALITY;
  case RPCR_SINC_FASTEST:
    return SRC_SINC_FASTEST;
  }
  return -1;
}

/** @brief Convert a bounded millisecond duration to PCM samples. */
static size_t milliseconds_to_samples(unsigned int sample_rate,
                                      unsigned int milliseconds) {
  return ((size_t)sample_rate * milliseconds) / 1000U;
}

/** @brief Return one sample preceding the current consumer history cursor. */
static int16_t history_back(const struct rpcr_ring *ring, size_t distance) {
  size_t offset = distance % ring->capacity;
  return ring->history[(ring->history_next + ring->capacity - offset) %
                       ring->capacity];
}

/** @brief Retain the actual playout waveform for a later shortfall. */
static void remember_output(struct rpcr_ring *ring, const int16_t *samples,
                            size_t count) {
  for (size_t index = 0; index < count; ++index) {
    ring->history[ring->history_next] = samples[index];
    ring->history_next = (ring->history_next + 1U) % ring->capacity;
    if (ring->history_length < ring->capacity)
      ++ring->history_length;
  }
}

/** @brief Find a low-error pitch period in the most recent output history. */
static size_t detect_pitch(const struct rpcr_ring *ring) {
  if (!ring->output_rate)
    return 0;
  size_t minimum = ring->output_rate / RPCR_PLC_MAX_PITCH_HZ;
  size_t maximum = ring->output_rate / RPCR_PLC_MIN_PITCH_HZ;
  size_t window =
      milliseconds_to_samples(ring->output_rate, RPCR_PLC_ANALYSIS_MS);
  size_t resolution = ring->output_rate / 48000U;
  if (!minimum)
    minimum = 1;
  if (!window)
    window = 1;
  if (!resolution)
    resolution = 1;
  if (maximum < minimum || ring->history_length < maximum + window)
    return 0;
  uint64_t best_error = UINT64_MAX;
  size_t best_period = 0;
  for (size_t period = minimum; period <= maximum; period += resolution) {
    uint64_t error = 0;
    for (size_t index = 0; index < window; index += resolution) {
      int difference = (int)history_back(ring, index + 1U) -
                       (int)history_back(ring, index + period + 1U);
      error += (uint64_t)(difference < 0 ? -difference : difference);
    }
    if (error < best_error) {
      best_error = error;
      best_period = period;
    }
  }
  return best_period;
}

/** @brief Apply the bounded erasure envelope without overflowing PCM math. */
static int16_t attenuate_concealment(const struct rpcr_ring *ring,
                                     int16_t sample, size_t position) {
  size_t hold = milliseconds_to_samples(ring->output_rate, RPCR_PLC_HOLD_MS);
  size_t fade = milliseconds_to_samples(ring->output_rate, RPCR_PLC_FADE_MS);
  if (!fade || position <= hold)
    return sample;
  if (position >= fade)
    return 0;
  return (int16_t)(((int64_t)sample * (int64_t)(fade - position)) /
                   (int64_t)(fade - hold));
}

/** @brief Produce one pitch-repeated output sample for the current erasure. */
static int16_t concealed_sample(const struct rpcr_ring *ring, size_t position) {
  if (!ring->plc_period)
    return 0;
  size_t phase = position % ring->plc_period;
  int16_t sample = history_back(ring, ring->plc_period - phase);
  return attenuate_concealment(ring, sample, position);
}

/** @brief Blend adjacent waveform segments without a transition-level dip.
 *
 * The sine-law-equivalent square-root gains retain roughly constant power as
 * real PCM enters or leaves pitch-period continuation.  Both callers advance
 * the same @c plc_samples cursor, so the generated waveform remains phase
 * continuous across an erasure and its recovery.
 */
static int16_t equal_power_mix(int16_t outgoing, int16_t incoming, size_t index,
                               size_t samples) {
  double progress = ((double)index + 1.0) / ((double)samples + 1.0);
  double mixed = sqrt(1.0 - progress) * outgoing + sqrt(progress) * incoming;
  if (mixed > INT16_MAX)
    return INT16_MAX;
  if (mixed < INT16_MIN)
    return INT16_MIN;
  return (int16_t)lround(mixed);
}

/** @brief Fill a shortfall with a bounded, pitch-repeated PCM waveform. */
static void conceal_output(struct rpcr_ring *ring, int16_t *output,
                           size_t samples) {
  if (!ring->plc_samples)
    ring->plc_period = detect_pitch(ring);
  ring->recovery_samples = 0;
  size_t crossfade =
      milliseconds_to_samples(ring->output_rate, RPCR_PLC_CROSSFADE_MS);
  int16_t previous = ring->history_length ? history_back(ring, 1) : 0;
  for (size_t index = 0; index < samples; ++index) {
    size_t position = ring->plc_samples;
    int16_t sample = concealed_sample(ring, position);
    if (crossfade && position < crossfade)
      sample = equal_power_mix(previous, sample, position, crossfade);
    output[index] = sample;
    ++ring->plc_samples;
  }
}

/** @brief Smooth one recovered source sample into the concealed waveform. */
static int16_t recover_sample(struct rpcr_ring *ring, int16_t sample) {
  size_t crossfade;
  size_t index;

  if (!ring->plc_samples)
    return sample;
  crossfade = milliseconds_to_samples(ring->output_rate, RPCR_PLC_CROSSFADE_MS);
  if (!crossfade) {
    ring->plc_period = 0;
    ring->plc_samples = 0;
    return sample;
  }
  if (!ring->recovery_samples)
    ring->recovery_samples = crossfade;
  index = crossfade - ring->recovery_samples;
  sample = equal_power_mix(concealed_sample(ring, ring->plc_samples + index),
                           sample, index, crossfade);
  if (!--ring->recovery_samples) {
    ring->plc_period = 0;
    ring->plc_samples = 0;
  }
  return sample;
}

/** @brief Refill the consumer-owned converted PCM cache without blocking. */
static bool refill_output(struct rpcr_ring *ring, size_t target) {
  const size_t quantum = ring->capacity < 256U ? ring->capacity : 256U;
  int16_t sample = 0;
  SRC_DATA data;

  if (ring->output_pending)
    return true;
  if (!ring->input_rate || !ring->output_rate || !ring->converter)
    return false;
  if (ring->input_offset && ring->input_offset + quantum > ring->capacity) {
    for (size_t index = 0; index < ring->input_pending; ++index)
      ring->input[index] = ring->input[ring->input_offset + index];
    ring->input_offset = 0;
  }
  while (ring->input_pending < quantum &&
         rpcr_consumer_pop_sample(ring, &sample)) {
    ring->input[ring->input_offset + ring->input_pending++] = sample / 32768.0F;
  }
  if (!ring->input_pending)
    return false;

  uint64_t occupancy =
      (uint64_t)(rpcr_available(ring) + ring->input_pending) * 1000U;
  if (!ring->occupancy_milli)
    ring->occupancy_milli = occupancy;
  else
    ring->occupancy_milli =
        (uint64_t)((int64_t)ring->occupancy_milli +
                   ((int64_t)occupancy - (int64_t)ring->occupancy_milli) / 128);
  atomic_store_explicit(&ring->filtered_occupancy_milli, ring->occupancy_milli,
                        memory_order_relaxed);
  double nominal = (double)ring->output_rate / (double)ring->input_rate;
  double error =
      target ? ((double)ring->occupancy_milli - (double)target * 1000.0) /
                   ((double)target * 1000.0)
             : 0.0;
  if (error > 1.0)
    error = 1.0;
  ring->ratio += ring->ratio
                     ? (nominal * (1.0 - error * 0.001) - ring->ratio) / 512.0
                     : nominal;
  atomic_store_explicit(&ring->ratio_correction_ppm,
                        (int)lround((ring->ratio / nominal - 1.0) * 1000000.0),
                        memory_order_relaxed);
  data = (SRC_DATA){.data_in = ring->input + ring->input_offset,
                    .data_out = ring->output,
                    .input_frames = (long)ring->input_pending,
                    .output_frames = (long)quantum,
                    .src_ratio = ring->ratio};
  if (src_process(ring->converter, &data) || data.input_frames_used < 0 ||
      data.input_frames_used > (long)ring->input_pending ||
      data.output_frames_gen <= 0 || data.output_frames_gen > (long)quantum)
    return false;
  ring->input_offset += (size_t)data.input_frames_used;
  ring->input_pending -= (size_t)data.input_frames_used;
  if (!ring->input_pending)
    ring->input_offset = 0;
  ring->output_offset = 0;
  ring->output_pending = (size_t)data.output_frames_gen;
  return true;
}

int rpcr_init(struct rpcr_ring *ring, size_t capacity,
              enum rpcr_quality quality) {
  int error = 0;
  if (!ring || !capacity || converter_type(quality) < 0) {
    return -1;
  }
  *ring = (struct rpcr_ring){0};
  ring->storage = calloc(capacity, sizeof(*ring->storage));
  ring->input = calloc(capacity, sizeof(*ring->input));
  ring->output = calloc(capacity, sizeof(*ring->output));
  ring->history = calloc(capacity, sizeof(*ring->history));
  ring->converter = src_new(converter_type(quality), 1, &error);
  ring->capacity = capacity;
  if (!ring->storage || !ring->input || !ring->output || !ring->history ||
      !ring->converter || error) {
    rpcr_destroy(ring);
    return -1;
  }
  atomic_init(&ring->written, 0);
  atomic_init(&ring->read, 0);
  atomic_init(&ring->discarded, 0);
  atomic_init(&ring->missing, 0);
  atomic_init(&ring->consecutive_underruns, 0);
  atomic_init(&ring->underrun_average_milli, 0);
  atomic_init(&ring->reserve_samples, 0);
  atomic_init(&ring->target_samples, 0);
  atomic_init(&ring->filtered_occupancy_milli, 0);
  atomic_init(&ring->ratio_correction_ppm, 0);
  return 0;
}

void rpcr_destroy(struct rpcr_ring *ring) {
  if (!ring)
    return;
  src_delete(ring->converter);
  free(ring->storage);
  free(ring->input);
  free(ring->output);
  free(ring->history);
  *ring = (struct rpcr_ring){0};
}

bool rpcr_producer_push_sample(struct rpcr_ring *ring, int16_t sample) {
  if (!ring || !ring->storage || !ring->capacity)
    return false;
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_relaxed);
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
  if (written - read >= ring->capacity) {
    atomic_fetch_add_explicit(&ring->discarded, 1, memory_order_relaxed);
    return false;
  }
  ring->storage[written % ring->capacity] = sample;
  atomic_store_explicit(&ring->written, written + 1U, memory_order_release);
  return true;
}

bool rpcr_consumer_pop_sample(struct rpcr_ring *ring, int16_t *sample) {
  if (!ring || !sample || !ring->storage || !ring->capacity)
    return false;
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_relaxed);
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_acquire);
  if (read == written || written - read > ring->capacity)
    return false;
  *sample = ring->storage[read % ring->capacity];
  atomic_store_explicit(&ring->read, read + 1U, memory_order_release);
  return true;
}

void rpcr_write(struct rpcr_ring *ring, const int16_t *input, size_t samples) {
  if (!ring || !input)
    return;
  for (size_t index = 0; index < samples; ++index)
    (void)rpcr_producer_push_sample(ring, input[index]);
}

int rpcr_set_sample_rate(struct rpcr_ring *ring, unsigned int sample_rate) {
  return rpcr_set_rates(ring, sample_rate, sample_rate);
}

int rpcr_set_rates(struct rpcr_ring *ring, unsigned int input_rate,
                   unsigned int output_rate) {
  if (!ring || !ring->converter || !input_rate || !output_rate)
    return -1;
  ring->input_rate = input_rate;
  ring->output_rate = output_rate;
  ring->occupancy_milli = 0;
  ring->ratio = 0.0;
  ring->input_offset = ring->input_pending = 0;
  ring->output_offset = ring->output_pending = 0;
  ring->plc_period = 0;
  ring->plc_samples = 0;
  ring->recovery_samples = 0;
  src_reset(ring->converter);
  return 0;
}

size_t rpcr_available(const struct rpcr_ring *ring) {
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_acquire);
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
  return written - read < ring->capacity ? (size_t)(written - read)
                                         : ring->capacity;
}

void rpcr_observe(const struct rpcr_ring *ring,
                  struct rpcr_observation *observation) {
  if (!observation)
    return;
  *observation = (struct rpcr_observation){0};
  if (!ring)
    return;
  observation->capacity_samples = ring->capacity;
  observation->available_samples = rpcr_available(ring);
  observation->reserve_samples =
      atomic_load_explicit(&ring->reserve_samples, memory_order_relaxed);
  observation->filtered_occupancy_samples =
      atomic_load_explicit(&ring->filtered_occupancy_milli,
                           memory_order_relaxed) /
      1000U;
  observation->target_samples =
      atomic_load_explicit(&ring->target_samples, memory_order_relaxed);
  observation->ratio_correction_ppm =
      atomic_load_explicit(&ring->ratio_correction_ppm, memory_order_relaxed);
}

void rpcr_record_shortfall(struct rpcr_ring *ring, size_t missing,
                           size_t samples, unsigned int rate) {
  atomic_fetch_add_explicit(&ring->missing, missing, memory_order_relaxed);
  uint64_t consecutive =
      missing ? atomic_fetch_add_explicit(&ring->consecutive_underruns, missing,
                                          memory_order_relaxed) +
                    missing
              : 0;
  if (!missing)
    atomic_store_explicit(&ring->consecutive_underruns, 0,
                          memory_order_relaxed);
  uint64_t average =
      atomic_load_explicit(&ring->underrun_average_milli, memory_order_relaxed);
  uint64_t denominator = (uint64_t)rate * 10000U;
  uint64_t weight = denominator ? (uint64_t)samples * 1000U : 0;
  if (weight > denominator)
    weight = denominator;
  int64_t difference = (int64_t)(consecutive * 1000U) - (int64_t)average;
  int64_t adjustment =
      denominator ? difference * (int64_t)weight / (int64_t)denominator : 0;
  atomic_store_explicit(&ring->underrun_average_milli,
                        (uint64_t)((int64_t)average + adjustment),
                        memory_order_relaxed);
}

bool rpcr_consumer_render_sample(struct rpcr_ring *ring, int16_t *output,
                                 size_t target) {
  if (!ring || !output)
    return false;
  atomic_store_explicit(&ring->target_samples, target, memory_order_relaxed);
  if (!refill_output(ring, target)) {
    conceal_output(ring, output, 1);
    rpcr_record_shortfall(ring, 1, 1, ring->output_rate);
    return false;
  }
  src_float_to_short_array(ring->output + ring->output_offset++, output, 1);
  --ring->output_pending;
  if (!ring->output_pending)
    ring->output_offset = 0;
  output[0] = recover_sample(ring, output[0]);
  remember_output(ring, output, 1);
  rpcr_record_shortfall(ring, 0, 1, ring->output_rate);
  return true;
}

size_t rpcr_render(struct rpcr_ring *ring, int16_t *output, size_t samples,
                   size_t reserve, size_t target) {
  if (!ring || !output)
    return 0;
  atomic_store_explicit(&ring->reserve_samples, reserve, memory_order_relaxed);
  if (samples > ring->capacity) {
    conceal_output(ring, output, samples);
    return 0;
  }
  size_t rendered = 0;
  for (size_t index = 0; index < samples; ++index)
    if (rpcr_consumer_render_sample(ring, output + index, target))
      ++rendered;
  return rendered;
}
