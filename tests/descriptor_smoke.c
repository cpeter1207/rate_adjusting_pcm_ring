/**
 * @file descriptor_smoke.c
 * @brief Verify a C consumer can use the Rust F32 ring descriptor.
 */

#include <assert.h>
#include <stddef.h>
#include <string.h>

#include "rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h"

int main(void) {
  const struct rpcr2_descriptor *descriptor;
  struct rpcr2_config config = {
      .struct_size = sizeof(config),
      .abi_version = RPCR2_ABI_VERSION,
      .capacity_samples = RPCR2_MINIMUM_CAPACITY_SAMPLES,
      .input_rate_hz = 8000,
      .output_rate_hz = 8000,
      .quality = RPCR2_QUALITY_BEST,
  };
  struct rpcr2_ring *ring = NULL;
  struct rpcr2_observation observation = {
      .struct_size = sizeof(observation),
  };
  float input[RPCR2_MINIMUM_CAPACITY_SAMPLES] = {0.0F};
  float output[160] = {0.0F};
  uint64_t accepted = 0;
  uint64_t real_samples = 0;

  descriptor = rpcr2_descriptor();
  assert(descriptor != NULL);
  assert(descriptor->abi_version == RPCR2_ABI_VERSION);
  assert(descriptor->struct_size >= sizeof(*descriptor));
  assert(strcmp(descriptor->capability_name, RPCR2_CAPABILITY_NAME) == 0);
  assert(descriptor->ring_create != NULL);
  assert(descriptor->ring_destroy != NULL);
  assert(descriptor->ring_producer_push_sample != NULL);
  assert(descriptor->ring_producer_push != NULL);
  assert(descriptor->ring_consumer_render_sample != NULL);
  assert(descriptor->ring_consumer_render != NULL);
  assert(descriptor->ring_observe != NULL);
  assert(descriptor->ring_create(&config, &ring) == RPCR2_OK);
  assert(ring != NULL);
  assert(descriptor->ring_producer_push(ring, input,
                                        RPCR2_MINIMUM_CAPACITY_SAMPLES,
                                        &accepted) == RPCR2_OK);
  assert(accepted == RPCR2_MINIMUM_CAPACITY_SAMPLES);
  assert(descriptor->ring_consumer_render(ring, output, 160, 40, 80,
                                          &real_samples) == RPCR2_OK);
  assert(real_samples <= 160);
  assert(descriptor->ring_observe(ring, &observation) == RPCR2_OK);
  assert(observation.abi_version == RPCR2_ABI_VERSION);
  assert(observation.capacity_samples == RPCR2_MINIMUM_CAPACITY_SAMPLES);
  descriptor->ring_destroy(ring);
  return 0;
}
