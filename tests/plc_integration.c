/** @file plc_integration.c
 * @brief Exercise approved callers through the real ring and converter DSOs.
 */
#include <assert.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "rate_adjusting_pcm_ring3/rate_adjusting_pcm_ring3.h"

static const struct rpcr3_descriptor *api;
extern void ring_test_realtime_begin(void);
extern void ring_test_realtime_end(void);

/** Publish constant PCM in calls respecting the immutable producer bound. */
static void supply(struct rpcr3_ring *ring, uint64_t count, uint64_t maximum) {
  float source[4096];
  for (unsigned i = 0; i < 4096; ++i)
    source[i] = 0.25F;
  while (count != 0) {
    uint64_t chunk = count < maximum ? count : maximum;
    uint64_t accepted = 0;
    assert(chunk <= 4096);
    assert(api->ring_producer_push(ring, source, chunk, &accepted) == RPCR3_OK);
    assert(accepted == chunk);
    count -= chunk;
  }
}

/** Read a correctly sized observation from the actual shared object. */
static struct rpcr3_observation observe(struct rpcr3_ring *ring) {
  struct rpcr3_observation result = {.struct_size = sizeof(result)};
  assert(api->ring_observe(ring, &result) == RPCR3_OK);
  return result;
}

/** Check native caller policies, loss, recovery, and burst reset. */
static void exercise_policy(struct rpcr3_config config) {
  struct rpcr3_ring *ring = NULL;
  assert(api->ring_create(&config, &ring) == RPCR3_OK);
  float out[4096] = {0};
  uint64_t real = 0;
  if (config.reserve_samples > 0) {
    supply(ring, config.reserve_samples - 1, config.max_producer_samples);
    assert(api->ring_consumer_render(ring, out, 16, &real) == RPCR3_OK);
    assert(real == 0 && observe(ring).missing_samples == 0);
    supply(ring, 1, config.max_producer_samples);
  } else {
    supply(ring, 512, config.max_producer_samples);
  }
  assert(api->ring_consumer_render(ring, out, 256, &real) == RPCR3_OK);
  assert(real > 0);
  for (unsigned i = 0; i < 256; ++i)
    assert(isfinite(out[i]) && fabsf(out[i]) <= 1.0F);
  for (unsigned block = 0; block < 70; ++block)
    assert(api->ring_consumer_render(ring, out, 256, &real) == RPCR3_OK);
  assert(real == 0 && observe(ring).missing_samples > 0);
  for (unsigned i = 0; i < 256; ++i)
    assert(out[i] == 0.0F);
  supply(ring, 512, config.max_producer_samples);
  assert(api->ring_consumer_render(ring, out, 256, &real) == RPCR3_OK);
  assert(real > 0); /* No re-prime after loss. */
  assert(api->ring_consumer_reset(ring) == RPCR3_OK);
  assert(observe(ring).available_samples == 0);
  api->ring_destroy(ring);
}

/** Compare all five policy shapes, retaining offline no-PLC duration. */
static void callers(void) {
  struct rpcr3_config c = {.struct_size = sizeof(c),
                           .abi_version = RPCR3_ABI_VERSION,
                           .capacity_samples = 640,
                           .input_rate_hz = 8000,
                           .output_rate_hz = 48000,
                           .reserve_samples = 162,
                           .target_samples = 320,
                           .max_producer_samples = 160,
                           .max_output_samples = 960,
                           .plc_mode = RPCR3_PLC_G711_APPENDIX_I};
  exercise_policy(c); /* ASL program ring. */
  c.input_rate_hz = 48000;
  c.capacity_samples = 3840;
  c.reserve_samples = 960;
  c.target_samples = 1920;
  c.max_producer_samples = 960;
  c.plc_mode = RPCR3_PLC_DISABLED;
  exercise_policy(c); /* Advanced fallback. */
  c.capacity_samples = 6176;
  c.input_rate_hz = 8000;
  c.reserve_samples = 685;
  c.target_samples = 2080;
  c.max_producer_samples = c.max_output_samples = 4096;
  c.plc_mode = RPCR3_PLC_G711_APPENDIX_I;
  exercise_policy(c); /* Negotiated 8 kHz peer. */
  c.input_rate_hz = 48000;
  c.capacity_samples = 14400;
  c.reserve_samples = c.target_samples = 7200;
  c.plc_mode = RPCR3_PLC_DISABLED;
  exercise_policy(c); /* Delayed local receive. */
  c.capacity_samples = 1024;
  c.reserve_samples = c.target_samples = 0;
  exercise_policy(c); /* Offline media, no replacement output. */
}

/** Both drift directions remain within the declared app_rpt geometry. */
static void drift(int ppm) {
  struct rpcr3_config c = {.struct_size = sizeof(c),
                           .abi_version = RPCR3_ABI_VERSION,
                           .capacity_samples = 640,
                           .input_rate_hz = 8000,
                           .output_rate_hz = 48000,
                           .reserve_samples = 162,
                           .target_samples = 320,
                           .max_producer_samples = 160,
                           .max_output_samples = 960,
                           .plc_mode = RPCR3_PLC_G711_APPENDIX_I};
  struct rpcr3_ring *ring = NULL;
  assert(api->ring_create(&c, &ring) == RPCR3_OK);
  supply(ring, 320, 160);
  uint64_t accumulated = 0;
  for (unsigned block = 0; block < 9000; ++block) {
    float output[960];
    uint64_t real = 0;
    assert(api->ring_consumer_render(ring, output, 960, &real) == RPCR3_OK);
    accumulated += 160U * (uint64_t)(1000000 + ppm);
    supply(ring, accumulated / 1000000U, 160);
    accumulated %= 1000000U;
  }
  struct rpcr3_observation result = observe(ring);
  assert(result.discarded_samples == 0 && result.missing_samples == 0);
  assert(result.adapter_error_count == 0);
  assert(result.ratio_correction_ppm >= -1000 &&
         result.ratio_correction_ppm <= 1000);
  assert((int64_t)result.ratio_correction_ppm * ppm < 0);
  assert(result.filtered_occupancy_samples > 128 &&
         result.filtered_occupancy_samples < c.capacity_samples);
  api->ring_destroy(ring);
}

/** Timed metadata follows native SRC/PLC without callback allocations or stale
 * tags. */
static void timed_rendering(void) {
  struct rpcr3_config c = {.struct_size = sizeof(c),
                           .abi_version = RPCR3_ABI_VERSION,
                           .capacity_samples = 2048,
                           .input_rate_hz = 8000,
                           .output_rate_hz = 48000,
                           .reserve_samples = 700,
                           .target_samples = 1000,
                           .max_producer_samples = 640,
                           .max_output_samples = 32,
                           .plc_mode = RPCR3_PLC_G711_APPENDIX_I};
  struct rpcr3_ring *ring = NULL;
  assert(api->ring_create(&c, &ring) == RPCR3_OK);
  float source[512];
  for (unsigned i = 0; i < 512; ++i)
    source[i] = 0.25F;
  uint64_t accepted = 0, token = 0;
  assert(api->ring_producer_push_tagged(ring, source, 512, 1, 0, NULL, 0,
                                        &accepted) == RPCR3_OK &&
         accepted == 512);
  assert(api->ring_producer_gap(ring, 16, &accepted) == RPCR3_OK &&
         accepted == 16);
  assert(api->ring_producer_push_tagged(ring, source, 512, 1, 528, NULL, 0,
                                        &accepted) == RPCR3_OK &&
         accepted == 512);
  assert(api->ring_producer_finish(ring, &token) == RPCR3_OK);
  bool gap = false, plc = false, negative = false, muted = false;
  uint32_t status = RPCR3_FINISH_PENDING;
  ring_test_realtime_begin();
  for (unsigned count = 0; status == RPCR3_FINISH_PENDING; ++count) {
    assert(count < 8000);
    if (count == 100) {
      const struct rpcr3_mute_span span = {.start = 10, .end = 30};
      uint32_t credit = 0;
      assert(api->ring_producer_mute(ring, 1, &span, 1, &credit) == RPCR3_OK &&
             credit == 1);
    }
    struct rpcr3_timed_sample_v1 timing = {.struct_size = sizeof(timing)};
    float sample = 0.0F;
    assert(api->ring_consumer_render_sample_timed(ring, &sample, &timing) ==
           RPCR3_OK);
    assert(isfinite(sample));
    gap |= (timing.flags & RPCR3_TIMED_GAP) != 0;
    plc |= (timing.flags & RPCR3_TIMED_PLC) != 0;
    negative |= (timing.flags & RPCR3_TIMED_NEGATIVE) != 0;
    muted |= (timing.flags & RPCR3_TIMED_MUTED) != 0;
    if (timing.flags & RPCR3_TIMED_POSITION_VALID)
      assert(timing.generation == 1 && timing.source_frame < 1040);
    assert(api->ring_finish_status(ring, token, &status) == RPCR3_OK);
  }
  assert(api->ring_consumer_reset(ring) == RPCR3_OK);
  ring_test_realtime_end();
  assert(gap && plc && negative && muted);
  api->ring_destroy(ring);
}

int main(void) {
  api = rpcr3_descriptor();
  assert(api && api->abi_version == RPCR3_ABI_VERSION);
  timed_rendering();
  callers();
  drift(-100);
  drift(100);
  drift(-500);
  drift(500);
  puts("PLC callers and +/-100/500 ppm drift: passed");
  return 0;
}
