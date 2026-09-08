/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Persistent sinc conversion and slow SPSC-ring clock recovery.
 */
#include "rate_adjusting_pcm_ring.h"
#include <samplerate.h>
#include <stdlib.h>

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
  ring->converter = src_new(converter_type(quality), 1, &error);
  ring->capacity = capacity;
  if (!ring->storage || !ring->input || !ring->output || !ring->converter ||
      error) {
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
  return 0;
}

void rpcr_destroy(struct rpcr_ring *ring) {
  if (!ring)
    return;
  src_delete(ring->converter);
  free(ring->storage);
  free(ring->input);
  free(ring->output);
  *ring = (struct rpcr_ring){0};
}

void rpcr_write(struct rpcr_ring *ring, const int16_t *input, size_t samples) {
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_relaxed);
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
  size_t available = written - read < ring->capacity ? (size_t)(written - read)
                                                     : ring->capacity;
  size_t original = samples;
  if (samples > ring->capacity) {
    input += samples - ring->capacity;
    samples = ring->capacity;
  }
  size_t free_samples = ring->capacity - available;
  size_t overwritten = samples > free_samples ? samples - free_samples : 0;
  for (size_t i = 0; i < samples; ++i)
    ring->storage[(written + i) % ring->capacity] = input[i];
  atomic_store_explicit(&ring->written, written + samples,
                        memory_order_release);
  atomic_fetch_add_explicit(&ring->discarded, original - samples + overwritten,
                            memory_order_relaxed);
}

size_t rpcr_available(const struct rpcr_ring *ring) {
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_acquire);
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
  return written - read < ring->capacity ? (size_t)(written - read)
                                         : ring->capacity;
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

bool rpcr_render(struct rpcr_ring *ring, int16_t *output, size_t samples,
                 size_t reserve, size_t target) {
  uint64_t read = atomic_load_explicit(&ring->read, memory_order_relaxed);
  uint64_t written = atomic_load_explicit(&ring->written, memory_order_acquire);
  size_t available = written - read < ring->capacity ? (size_t)(written - read)
                                                     : ring->capacity;
  /* Do not start a clock-recovery stream from one short burst. */
  if (!ring->primed && available >= target)
    ring->primed = true;
  if (!ring->primed || samples > ring->capacity || available <= reserve) {
    for (size_t index = 0; index < samples; ++index)
      output[index] = 0;
    return false;
  }
  size_t input_count = available - reserve;
  for (size_t i = 0; i < input_count; ++i)
    ring->input[i] = ring->storage[(read + i) % ring->capacity] / 32768.0F;
  /* Cascaded slow controls avoid callback-rate pitch modulation. */
  uint64_t occupancy = (uint64_t)available * 1000U;
  if (!ring->occupancy_milli) {
    ring->occupancy_milli = occupancy;
  } else {
    ring->occupancy_milli =
        (uint64_t)((int64_t)ring->occupancy_milli +
                   ((int64_t)occupancy - (int64_t)ring->occupancy_milli) / 128);
  }
  double error =
      ((double)((int64_t)ring->occupancy_milli - (int64_t)target * 1000)) /
      ((double)target * 1000.0);
  if (error > 1.0)
    error = 1.0;
  double desired = 1.0 - error * 0.001;
  ring->ratio += ring->ratio ? (desired - ring->ratio) / 512.0 : 1.0;
  SRC_DATA data = {.data_in = ring->input,
                   .data_out = ring->output,
                   .input_frames = (long)input_count,
                   .output_frames = (long)samples,
                   .src_ratio = ring->ratio};
  if (src_process(ring->converter, &data))
    data.input_frames_used = data.output_frames_gen = 0;
  atomic_store_explicit(&ring->read, read + (uint64_t)data.input_frames_used,
                        memory_order_release);
  src_float_to_short_array(ring->output, output, (int)data.output_frames_gen);
  for (size_t i = (size_t)data.output_frames_gen; i < samples; ++i)
    output[i] = 0;
  return data.output_frames_gen != 0;
}
