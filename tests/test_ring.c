/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Basic lock-free ring priming and rate-controller tests.
 */
#include "rate_adjusting_pcm_ring.h"
#include <assert.h>
#include <samplerate.h>
#include <stdio.h>
#include <stdlib.h>

static unsigned int fail_calloc_call;
static unsigned int calloc_calls;
static bool fail_src_new;
static bool force_src_error;
static bool fail_src_process;
static long processed_input_frames;
void *__real_calloc(size_t count, size_t size);
SRC_STATE *__real_src_new(int converter_type, int channels, int *error);
int __real_src_process(SRC_STATE *state, SRC_DATA *data);
void *__wrap_calloc(size_t count, size_t size) {
  ++calloc_calls;
  return calloc_calls == fail_calloc_call ? NULL : __real_calloc(count, size);
}
SRC_STATE *__wrap_src_new(int converter_type, int channels, int *error) {
  if (fail_src_new) {
    *error = 1;
    return NULL;
  }
  SRC_STATE *state = __real_src_new(converter_type, channels, error);
  if (force_src_error)
    *error = 1;
  return state;
}
int __wrap_src_process(SRC_STATE *state, SRC_DATA *data) {
  processed_input_frames = data->input_frames;
  return fail_src_process ? 1 : __real_src_process(state, data);
}

static void seed_history(struct rpcr_ring *ring, size_t period) {
  for (size_t index = 0; index < ring->capacity; ++index)
    ring->history[index] = (int16_t)((index % period) * 300 - 12000);
  ring->history_length = ring->capacity;
  ring->history_next = 0;
}

int main(void) {
  struct rpcr_ring ring;
  int16_t input[2048];
  int16_t output[160];
  int16_t oversized[1025];
  for (size_t index = 0; index < 2048; ++index) {
    input[index] = (int16_t)(index * 1000);
  }
  rpcr_destroy(NULL);
  assert(rpcr_init(NULL, 1024, RPCR_SINC_BEST) != 0);
  assert(rpcr_init(&ring, 0, RPCR_SINC_BEST) != 0);
  assert(rpcr_init(&ring, 1, (enum rpcr_quality)99) != 0);
  for (unsigned int attempt = 1; attempt <= 4; ++attempt) {
    fail_calloc_call = attempt;
    calloc_calls = 0;
    assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) != 0);
  }
  fail_calloc_call = 0;
  fail_src_new = true;
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) != 0);
  fail_src_new = false;
  force_src_error = true;
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) != 0);
  force_src_error = false;
  assert(rpcr_init(&ring, 1024, RPCR_SINC_MEDIUM) == 0);
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_FASTEST) == 0);
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_sample_rate(NULL, 8000) != 0);
  assert(rpcr_set_sample_rate(&ring, 0) != 0);
  assert(rpcr_set_rates(NULL, 8000, 48000) != 0);
  assert(rpcr_set_rates(&ring, 0, 48000) != 0);
  assert(rpcr_set_rates(&ring, 8000, 0) != 0);
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  assert(ring.input_rate == 8000 && ring.output_rate == 8000);
  assert(rpcr_available(&ring) == 0);
  struct rpcr_observation observation = {0};
  rpcr_observe(NULL, &observation);
  assert(!observation.capacity_samples && !observation.available_samples &&
         !observation.ratio_correction_ppm);
  rpcr_observe(&ring, NULL);
  rpcr_observe(&ring, &observation);
  assert(observation.capacity_samples == ring.capacity &&
         !observation.available_samples && !observation.target_samples);
  /* Migrated from USBRadioPlus native-FIFO priming: protected reserve is
   * silence. */
  assert(!rpcr_render(&ring, output, 160, 320, 480));
  rpcr_observe(&ring, &observation);
  assert(observation.reserve_samples == 320 &&
         observation.target_samples == 480);
  rpcr_write(&ring, input, 1024);
  assert(rpcr_available(&ring) == ring.capacity);
  assert(!rpcr_render(&ring, oversized, 1025, 0, 480));
  bool rendered = false;
  for (size_t attempt = 0; attempt < 4 && !rendered; ++attempt) {
    rendered = rpcr_render(&ring, output, 160, 0, 1024);
  }
  assert(rendered);
  /* A callback may consume only a bounded amount of source PCM.  Letting
   * libsamplerate drain the entire reserve margin creates a false underrun on
   * the following callback while valid source PCM still exists. */
  assert(processed_input_frames <= 320);
  assert(ring.ratio > 0.99 && ring.ratio < 1.01);
  rpcr_observe(&ring, &observation);
  assert(observation.available_samples <= observation.capacity_samples &&
         observation.filtered_occupancy_samples &&
         observation.reserve_samples == 0 &&
         observation.target_samples == 1024);
  assert(!rpcr_render(&ring, output, 160, ring.capacity, 480));
  /* Migrated from RPT Advanced elastic-peer tests: newest PCM survives overrun.
   */
  rpcr_write(&ring, input, 2048);
  assert(atomic_load(&ring.discarded) > 0);
  assert(rpcr_render(&ring, output, 160, 0, 480));
  /* Extreme occupancy correction remains deliberately bounded and gradual. */
  rpcr_write(&ring, input, 1024);
  ring.occupancy_milli = (uint64_t)ring.capacity * 3000U;
  assert(rpcr_render(&ring, output, 160, 0, 1));
  assert(ring.ratio < 1.0 && ring.ratio > 0.99);
  rpcr_observe(&ring, &observation);
  assert(observation.ratio_correction_ppm < 0);
  fail_src_process = true;
  assert(!rpcr_render(&ring, output, 160, 0, 480));
  fail_src_process = false;
  rpcr_record_shortfall(&ring, 2, 160, 8000);
  assert(atomic_load(&ring.missing) == 2);
  assert(atomic_load(&ring.consecutive_underruns) == 2);
  assert(atomic_load(&ring.underrun_average_milli) > 0);
  rpcr_record_shortfall(&ring, 0, 160, 8000);
  assert(atomic_load(&ring.consecutive_underruns) == 0);
  rpcr_record_shortfall(&ring, 1, 90000, 8000);
  rpcr_record_shortfall(&ring, 1, 160, 0);
  rpcr_destroy(&ring);
  /* One persistent stream combines an 8 kHz source conversion with the
   * slow occupancy correction used by a 48 kHz hardware callback. */
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_rates(&ring, 8000, 48000) == 0);
  rpcr_write(&ring, input, 1024);
  processed_input_frames = 0;
  assert(rpcr_render(&ring, output, 960, 0, 480));
  assert(processed_input_frames <= 320);
  assert(ring.ratio > 5.99 && ring.ratio < 6.01);
  rpcr_observe(&ring, &observation);
  assert(observation.ratio_correction_ppm > -100 &&
         observation.ratio_correction_ppm < 100);
  rpcr_write(&ring, input, 1024);
  assert(rpcr_render(&ring, output, 960, 0, 0));
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  rpcr_write(&ring, input, 160);
  processed_input_frames = 0;
  assert(rpcr_render(&ring, output, 160, 0, 1));
  assert(processed_input_frames == 160);
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  rpcr_write(&ring, input, 1024);
  int16_t tiny_output[2];
  processed_input_frames = 0;
  (void)rpcr_render(&ring, tiny_output, 2, 0, 1);
  assert(processed_input_frames == 256);
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  /* A ring with no configured PCM rate retains historical silence behavior. */
  seed_history(&ring, 80);
  ring.primed = true;
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  assert(!output[0] && !output[1]);
  /* A partially configured ring must also fail closed. */
  ring.input_rate = 8000;
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  /* Low-rate validation covers the generic concealer's zero-duration guards. */
  ring.plc_samples = ring.plc_period = 0;
  assert(rpcr_set_sample_rate(&ring, 1) == 0);
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  ring.plc_period = 1;
  ring.plc_samples = 1;
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  ring.plc_samples = ring.plc_period = 0;
  assert(rpcr_set_sample_rate(&ring, 48000) == 0);
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  /* A recent periodic waveform is continued across a short source shortage,
   * rather than replaced with zero-valued PCM. */
  seed_history(&ring, 80);
  ring.primed = true;
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  assert(!rpcr_render(&ring, output, 160, 0, 480));
  assert(ring.plc_period == 80);
  assert(output[100] != 0);
  /* The bounded PLC tail fades out after 60 ms of missing source PCM. */
  assert(!rpcr_render(&ring, output, 160, 0, 480));
  assert(!rpcr_render(&ring, output, 160, 0, 480));
  assert(!rpcr_render(&ring, output, 160, 0, 480));
  assert(output[159] == 0);
  /* A real waveform crossfades back in and resets the erasure state. */
  rpcr_write(&ring, input, 1024);
  bool recovered = false;
  for (size_t attempt = 0; attempt < 4 && !recovered; ++attempt)
    recovered = rpcr_render(&ring, output, 160, 0, 1) != 0;
  assert(recovered);
  assert(ring.plc_samples == 0);
  rpcr_destroy(&ring);
  /* Equal-power entry may exceed PCM range only when both adjacent segments
   * are already at a rail; saturate rather than wrap at either polarity. */
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  for (size_t index = 0; index < ring.capacity; ++index)
    ring.history[index] = INT16_MAX;
  ring.history_length = ring.capacity;
  ring.primed = true;
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  assert(!rpcr_render(&ring, output, 2, 0, 480));
  assert(output[0] == INT16_MAX);
  for (size_t index = 0; index < ring.capacity; ++index)
    ring.history[index] = INT16_MIN;
  ring.history_length = ring.capacity;
  ring.history_next = 0;
  ring.plc_period = ring.plc_samples = 0;
  assert(!rpcr_render(&ring, output, 2, 0, 480));
  assert(output[0] == INT16_MIN);
  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  seed_history(&ring, 80);
  ring.primed = true;
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  ring.plc_period = 80;
  ring.plc_samples = 1;
  rpcr_write(&ring, input, 1024);
  bool short_recovery = false;
  for (size_t attempt = 0; attempt < 4 && !short_recovery; ++attempt)
    short_recovery = rpcr_render(&ring, tiny_output, 2, 0, 1) != 0;
  assert(short_recovery);
  assert(ring.plc_samples == 0);
  rpcr_destroy(&ring);
  puts("rate-adjusting PCM ring tests passed");
  return 0;
}
