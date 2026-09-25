/**
 * @file descriptor_smoke.c
 * @brief Verify a C consumer can use the Rust F32 ring descriptor.
 */

#include <assert.h>
#include <stddef.h>
#include <string.h>

#include "rate_adjusting_pcm_ring3/rate_adjusting_pcm_ring3.h"

_Static_assert(sizeof(struct rpcr3_config) == 64, "ABI-3 configuration size");
_Static_assert(offsetof(struct rpcr3_config, reserve_samples) == 24,
               "ABI-3 reserve offset");
_Static_assert(offsetof(struct rpcr3_config, plc_mode) == 56,
               "ABI-3 mode offset");

int main(void) {
  const struct rpcr3_descriptor *descriptor = rpcr3_descriptor();
  struct rpcr3_config config = {
      .struct_size = sizeof(config),
      .abi_version = RPCR3_ABI_VERSION,
      .capacity_samples = 640,
      .input_rate_hz = 8000,
      .output_rate_hz = 8000,
      .reserve_samples = 162,
      .target_samples = 320,
      .max_producer_samples = 160,
      .max_output_samples = 160,
      .plc_mode = RPCR3_PLC_DISABLED,
  };
  struct rpcr3_ring *ring = NULL;
  struct rpcr3_observation observation = {
      .struct_size = sizeof(observation),
  };
  float input[160] = {0.0F};
  float output[160] = {0.0F};

  assert(descriptor != NULL);
  assert(descriptor->abi_version == 3U);
  assert(descriptor->abi_version == RPCR3_ABI_VERSION);
  assert(descriptor->struct_size >= sizeof(*descriptor));
  assert(strcmp(descriptor->capability_name, RPCR3_CAPABILITY_NAME) == 0);
  assert(descriptor->ring_create != NULL);
  assert(descriptor->ring_destroy != NULL);
  assert(descriptor->ring_producer_push_sample != NULL);
  assert(descriptor->ring_producer_push != NULL);
  assert(descriptor->ring_consumer_render_sample != NULL);
  assert(descriptor->ring_consumer_render != NULL);
  assert(descriptor->ring_consumer_reset != NULL);
  assert(descriptor->ring_observe != NULL);
  assert(descriptor->ring_create(&config, NULL) == RPCR3_INVALID_ARGUMENT);
  assert(descriptor->ring_create(NULL, &ring) == RPCR3_INVALID_ARGUMENT);
  assert(ring == NULL);
  descriptor->ring_destroy(NULL);

  for (uint32_t mode = RPCR3_PLC_DISABLED; mode <= RPCR3_PLC_G711_APPENDIX_I;
       ++mode) {
    struct rpcr3_config invalid = config;
    const uint32_t short_config = sizeof(short_config);
    uint32_t short_observation = sizeof(short_observation);
    float sample = 0.75F;
    bool real = true;
    bool single_accepted = true;
    config.plc_mode = mode;
    assert(descriptor->ring_create(&config, &ring) == RPCR3_OK);
    assert(ring != NULL);
    struct rpcr3_ring *original = ring;
    invalid.abi_version = 2;
    assert(descriptor->ring_create(&invalid, &ring) == RPCR3_INVALID_ARGUMENT);
    assert(ring == original);
    invalid = config;
    invalid.plc_mode = 2;
    assert(descriptor->ring_create(&invalid, &ring) == RPCR3_INVALID_ARGUMENT);
    assert(ring == original);
    assert(descriptor->ring_create((const struct rpcr3_config *)&short_config,
                                   &ring) == RPCR3_INVALID_ARGUMENT);
    assert(ring == original);

    assert(descriptor->ring_observe(
               ring, (struct rpcr3_observation *)&short_observation) ==
           RPCR3_INVALID_ARGUMENT);
    assert(short_observation == sizeof(short_observation));

    uint64_t accepted = 17;
    assert(descriptor->ring_producer_push(ring, (const float *)(uintptr_t)1,
                                          161,
                                          &accepted) == RPCR3_INVALID_ARGUMENT);
    assert(accepted == 17);
    assert(descriptor->ring_producer_push(ring, NULL, 1, &accepted) ==
           RPCR3_INVALID_ARGUMENT);
    assert(accepted == 17);
    assert(descriptor->ring_producer_push_sample(
               NULL, 0.5F, &single_accepted) == RPCR3_INVALID_ARGUMENT);
    assert(single_accepted);
    uint64_t real_samples = 23;
    assert(descriptor->ring_consumer_render(ring, (float *)(uintptr_t)1, 161,
                                            &real_samples) ==
           RPCR3_INVALID_ARGUMENT);
    assert(real_samples == 23);
    assert(descriptor->ring_consumer_render_sample(NULL, &sample, &real) ==
           RPCR3_INVALID_ARGUMENT);
    assert(sample == 0.75F && real);
    assert(descriptor->ring_observe(ring, &observation) == RPCR3_OK);
    assert(observation.available_samples == 0);
    assert(observation.missing_samples == 0);

    assert(descriptor->ring_producer_push(ring, NULL, 0, &accepted) ==
           RPCR3_OK);
    assert(accepted == 0);
    assert(descriptor->ring_consumer_render(ring, NULL, 0, &real_samples) ==
           RPCR3_OK);
    assert(real_samples == 0);
    assert(descriptor->ring_consumer_render_sample(ring, &sample, &real) ==
           RPCR3_OK);
    assert(sample == 0.0F && !real);
    for (unsigned int block = 0; block < 3; ++block) {
      assert(descriptor->ring_producer_push(ring, input, 160, &accepted) ==
             RPCR3_OK);
      assert(accepted == 160);
    }
    assert(descriptor->ring_consumer_render(ring, output, 160, &real_samples) ==
           RPCR3_OK);
    assert(real_samples == (mode == RPCR3_PLC_DISABLED ? 160U : 130U));
    assert(descriptor->ring_observe(ring, &observation) == RPCR3_OK);
    assert(observation.abi_version == RPCR3_ABI_VERSION);
    assert(observation.capacity_samples == 640);
    assert(observation.reserve_samples == 162);
    assert(observation.target_samples == 320);
    assert(observation.missing_samples == 0);
    assert(descriptor->ring_consumer_reset(ring) == RPCR3_OK);
    descriptor->ring_destroy(ring);
  }
  return 0;
}
