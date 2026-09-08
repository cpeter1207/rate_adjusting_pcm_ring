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
  for (unsigned int attempt = 1; attempt <= 3; ++attempt) {
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
  assert(rpcr_available(&ring) == 0);
  /* Migrated from USBRadioPlus native-FIFO priming: protected reserve is
   * silence. */
  assert(!rpcr_render(&ring, output, 160, 320, 480));
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
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  rpcr_write(&ring, input, 160);
  processed_input_frames = 0;
  assert(rpcr_render(&ring, output, 160, 0, 1));
  assert(processed_input_frames == 160);
  rpcr_destroy(&ring);
  puts("rate-adjusting PCM ring tests passed");
  return 0;
}
