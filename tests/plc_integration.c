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

/** Both signs of 100 ppm drift remain within the declared app_rpt geometry. */
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
  for (unsigned block = 0; block < 3000; ++block) {
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
  api->ring_destroy(ring);
}

int main(void) {
  api = rpcr3_descriptor();
  assert(api && api->abi_version == RPCR3_ABI_VERSION);
  callers();
  drift(-100);
  drift(100);
  puts("PLC callers and +/-100 ppm drift: passed");
  return 0;
}
