/* SPDX-License-Identifier: GPL-2.0-only */

#include "rate_adjusting_pcm_ring.h"

#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Private ABI-major-one-to-Rust bridge declarations duplicated for shim tests.
 */
struct rpcr1_bridge_ring;
struct rpcr1_bridge_observation {
  uint64_t capacity_samples;
  uint64_t available_samples;
  uint64_t written_samples;
  uint64_t read_samples;
  uint64_t reserve_samples;
  uint64_t filtered_occupancy_samples;
  uint64_t target_samples;
  int32_t ratio_correction_ppm;
  uint64_t discarded_samples;
  uint64_t missing_samples;
  uint64_t consecutive_underruns;
  uint64_t underrun_average_milli;
};
struct rpcr1_bridge_descriptor {
  uint32_t struct_size;
  uint32_t abi_version;
  const char *capability_name;
  int (*create)(uint64_t, uint32_t, uint32_t, uint32_t,
                struct rpcr1_bridge_ring **);
  void (*destroy)(struct rpcr1_bridge_ring *);
  int (*reconfigure)(struct rpcr1_bridge_ring *, uint32_t, uint32_t);
  int (*push_sample)(struct rpcr1_bridge_ring *, int16_t, bool *,
                     struct rpcr1_bridge_observation *);
  int (*push)(struct rpcr1_bridge_ring *, const int16_t *, uint64_t,
              uint64_t *);
  int (*pop_sample)(struct rpcr1_bridge_ring *, int16_t *, bool *);
  int (*render_sample)(struct rpcr1_bridge_ring *, int16_t *, uint64_t, bool *,
                       struct rpcr1_bridge_observation *);
  int (*render)(struct rpcr1_bridge_ring *, int16_t *, uint64_t, uint64_t,
                uint64_t, uint64_t *);
  int (*conceal)(struct rpcr1_bridge_ring *, int16_t *, uint64_t);
  int (*observe)(const struct rpcr1_bridge_ring *,
                 struct rpcr1_bridge_observation *);
  int (*record_shortfall)(struct rpcr1_bridge_ring *, uint64_t, uint64_t,
                          uint32_t);
};
struct rpcr1_compat_state {
  const struct rpcr1_bridge_descriptor *descriptor;
  struct rpcr1_bridge_ring *ring;
};

extern const struct rpcr1_bridge_descriptor *
__real_rpcr1_bridge_descriptor(void);
extern void *__real_calloc(size_t, size_t);

static const struct rpcr1_bridge_descriptor *bridge_override;
static bool use_bridge_override;
static bool fail_calloc;

const struct rpcr1_bridge_descriptor *__wrap_rpcr1_bridge_descriptor(void) {
  return use_bridge_override ? bridge_override
                             : __real_rpcr1_bridge_descriptor();
}

void *__wrap_calloc(size_t count, size_t size) {
  return fail_calloc ? NULL : __real_calloc(count, size);
}

static int fail_create(uint64_t capacity, uint32_t quality, uint32_t input_rate,
                       uint32_t output_rate, struct rpcr1_bridge_ring **ring) {
  (void)capacity;
  (void)quality;
  (void)input_rate;
  (void)output_rate;
  (void)ring;
  return -1;
}

static int fail_observe(const struct rpcr1_bridge_ring *ring,
                        struct rpcr1_bridge_observation *observation) {
  (void)ring;
  (void)observation;
  return -1;
}

static void clear_observation(struct rpcr1_bridge_observation *observation) {
  assert(observation);
  observation->capacity_samples = 0;
  observation->available_samples = 0;
  observation->written_samples = 0;
  observation->read_samples = 0;
  observation->reserve_samples = 0;
  observation->filtered_occupancy_samples = 0;
  observation->target_samples = 0;
  observation->ratio_correction_ppm = 0;
  observation->discarded_samples = 0;
  observation->missing_samples = 0;
  observation->consecutive_underruns = 0;
  observation->underrun_average_milli = 0;
}

static int fail_reconfigure(struct rpcr1_bridge_ring *ring, uint32_t input_rate,
                            uint32_t output_rate) {
  (void)ring;
  (void)input_rate;
  (void)output_rate;
  return -1;
}

static int fail_push_sample(struct rpcr1_bridge_ring *ring, int16_t sample,
                            bool *accepted,
                            struct rpcr1_bridge_observation *observation) {
  (void)ring;
  (void)sample;
  (void)accepted;
  clear_observation(observation);
  return -1;
}

static int fail_push(struct rpcr1_bridge_ring *ring, const int16_t *input,
                     uint64_t samples, uint64_t *accepted) {
  (void)ring;
  (void)input;
  (void)samples;
  (void)accepted;
  return -1;
}

static int fail_pop_sample(struct rpcr1_bridge_ring *ring, int16_t *output,
                           bool *available) {
  (void)ring;
  (void)output;
  (void)available;
  return -1;
}

static int fail_render_sample(struct rpcr1_bridge_ring *ring, int16_t *output,
                              uint64_t target, bool *real,
                              struct rpcr1_bridge_observation *observation) {
  (void)ring;
  (void)output;
  (void)target;
  (void)real;
  clear_observation(observation);
  return -1;
}

static int fail_render(struct rpcr1_bridge_ring *ring, int16_t *output,
                       uint64_t samples, uint64_t reserve, uint64_t target,
                       uint64_t *real_samples) {
  (void)ring;
  (void)output;
  (void)samples;
  (void)reserve;
  (void)target;
  (void)real_samples;
  return -1;
}

static int fail_conceal(struct rpcr1_bridge_ring *ring, int16_t *output,
                        uint64_t samples) {
  (void)ring;
  (void)output;
  (void)samples;
  return -1;
}

static int fail_record_shortfall(struct rpcr1_bridge_ring *ring,
                                 uint64_t missing, uint64_t samples,
                                 uint32_t rate) {
  (void)ring;
  (void)missing;
  (void)samples;
  (void)rate;
  return -1;
}

static struct rpcr1_bridge_descriptor copy_descriptor(void) {
  return *__real_rpcr1_bridge_descriptor();
}

static void
expect_invalid_descriptor(struct rpcr1_bridge_descriptor descriptor) {
  struct rpcr_ring ring;
  use_bridge_override = true;
  bridge_override = &descriptor;
  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) != 0);
  use_bridge_override = false;
  bridge_override = NULL;
}

static void test_initialization_validation(void) {
  struct rpcr_ring ring;
  struct rpcr1_bridge_descriptor descriptor = copy_descriptor();

  rpcr_destroy(NULL);
  assert(rpcr_init(NULL, 512, RPCR_SINC_BEST) != 0);
  assert(rpcr_init(&ring, 0, RPCR_SINC_BEST) != 0);
  assert(rpcr_init(&ring, 512, (enum rpcr_quality)99) != 0);

  use_bridge_override = true;
  bridge_override = NULL;
  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) != 0);
  use_bridge_override = false;
  bridge_override = NULL;
  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) == 0);
  rpcr_destroy(&ring);

  descriptor.struct_size = 0;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.abi_version++;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.capability_name = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.capability_name = "wrong";
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.create = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.destroy = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.reconfigure = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.push_sample = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.push = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.pop_sample = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.render_sample = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.render = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.conceal = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.observe = NULL;
  expect_invalid_descriptor(descriptor);
  descriptor = copy_descriptor();
  descriptor.record_shortfall = NULL;
  expect_invalid_descriptor(descriptor);

  fail_calloc = true;
  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) != 0);
  fail_calloc = false;

  descriptor = copy_descriptor();
  descriptor.create = fail_create;
  use_bridge_override = true;
  bridge_override = &descriptor;
  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) != 0);
  use_bridge_override = false;
  bridge_override = NULL;
}

static void test_forwarded_operations(void) {
  struct rpcr_ring ring;
  struct rpcr_ring invalid = {0};
  struct rpcr_observation observation;
  struct rpcr1_compat_state *state;
  const struct rpcr1_bridge_descriptor *real_descriptor;
  struct rpcr1_bridge_descriptor descriptor;
  int16_t source[1024];
  int16_t output[2048];
  int16_t sample = 0;
  size_t available_before;
  size_t index;
  bool concealed = false;
  bool rendered = false;

  for (index = 0; index < 1024; ++index)
    source[index] = (int16_t)(index * 31U);

  assert(!rpcr_producer_push_sample(NULL, 1));
  assert(!rpcr_producer_push_sample(&invalid, 1));
  assert(!rpcr_consumer_pop_sample(NULL, &sample));
  assert(!rpcr_consumer_pop_sample(&invalid, &sample));
  assert(!rpcr_consumer_pop_sample(&invalid, NULL));
  rpcr_write(NULL, source, 1);
  rpcr_write(&invalid, source, 1);
  rpcr_write(&invalid, NULL, 1);
  assert(rpcr_set_sample_rate(NULL, 8000) != 0);
  assert(rpcr_set_rates(&invalid, 8000, 8000) != 0);
  assert(!rpcr_render(NULL, output, 1, 0, 1));
  assert(!rpcr_render(&invalid, output, 1, 0, 1));
  assert(!rpcr_render(&invalid, NULL, 1, 0, 1));
  assert(!rpcr_consumer_render_sample(NULL, output, 1));
  assert(!rpcr_consumer_render_sample(&invalid, output, 1));
  assert(!rpcr_consumer_render_sample(&invalid, NULL, 1));
  assert(rpcr_available(NULL) == 0);
  rpcr_observe(NULL, &observation);
  assert(!observation.capacity_samples);
  rpcr_observe(&invalid, &observation);
  assert(!observation.capacity_samples);
  rpcr_observe(&invalid, NULL);
  rpcr_record_shortfall(NULL, 1, 1, 8000);
  rpcr_record_shortfall(&invalid, 1, 1, 8000);

  assert(rpcr_init(&ring, 512, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_rates(&ring, 0, 8000) != 0);
  assert(rpcr_set_rates(&ring, 8000, 0) != 0);
  rpcr_write(&ring, source, 1);
  output[0] = INT16_MAX;
  assert(!rpcr_consumer_render_sample(&ring, &output[0], 1));
  assert(output[0] == 0);
  assert(rpcr_available(&ring) == 1);
  assert(atomic_load(&ring.target_samples) == 1);
  rpcr_observe(&ring, &observation);
  assert(observation.target_samples == 1);
  output[0] = INT16_MAX;
  output[1] = INT16_MAX;
  assert(!rpcr_render(&ring, output, 2, 0, 1));
  assert(output[0] == 0);
  assert(output[1] == 0);
  assert(rpcr_available(&ring) == 1);
  assert(atomic_load(&ring.missing) == 3);
  assert(atomic_load(&ring.consecutive_underruns) == 3);
  assert(atomic_load(&ring.underrun_average_milli) == 0);
  assert(atomic_load(&ring.target_samples) == 1);
  rpcr_observe(&ring, &observation);
  assert(observation.target_samples == 1);
  assert(!rpcr_render(&ring, output, 0, 23, 999));
  assert(atomic_load(&ring.reserve_samples) == 23);
  assert(atomic_load(&ring.target_samples) == 1);
  rpcr_observe(&ring, &observation);
  assert(observation.reserve_samples == 23);
  assert(observation.target_samples == 1);
  assert(rpcr_set_rates(&ring, 16000, 8000) == 0);
  assert(rpcr_consumer_pop_sample(&ring, &sample));
  assert(sample == source[0]);
  assert(rpcr_set_sample_rate(&ring, 8000) == 0);
  assert(rpcr_available(&ring) == 0);
  rpcr_observe(&ring, &observation);
  assert(observation.capacity_samples == 512);
  assert(!observation.available_samples);

  assert(rpcr_producer_push_sample(&ring, 1234));
  assert(rpcr_consumer_pop_sample(&ring, &sample));
  assert(sample == 1234);
  assert(!rpcr_consumer_pop_sample(&ring, &sample));
  for (index = 0; index < ring.capacity; ++index)
    source[index] = 4000;
  rpcr_write(&ring, source, ring.capacity);
  assert(rpcr_available(&ring) == ring.capacity);
  assert(!rpcr_producer_push_sample(&ring, 1));
  atomic_store(&ring.reserve_samples, 123);
  for (index = 0; index < ring.capacity / 2; ++index) {
    (void)rpcr_consumer_render_sample(&ring, &output[index], 512);
    concealed |= output[index] != 0;
  }
  assert(concealed);
  rpcr_observe(&ring, &observation);
  assert(observation.reserve_samples == 123);
  available_before = rpcr_available(&ring);
  memset(output, 0, sizeof(output));
  assert(!rpcr_render(&ring, output, ring.capacity + 1, 0, 1));
  concealed = false;
  for (index = 0; index <= ring.capacity; ++index)
    concealed |= output[index] != 0;
  assert(concealed);
  assert(rpcr_available(&ring) == available_before);
  assert(rpcr_render(&ring, output, 160, 0, 512) <= 160);

  rpcr_destroy(&ring);
  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_rates(&ring, 8000, 8000) == 0);
  rpcr_write(&ring, source, 1024);
  for (index = 0; index < 16; ++index) {
    if (rpcr_consumer_render_sample(&ring, output + index, 512))
      rendered = true;
  }
  assert(rendered);
  rpcr_record_shortfall(&ring, 2, 160, 8000);
  rpcr_observe(&ring, &observation);
  assert(observation.reserve_samples == 0);
  assert(observation.target_samples == 512);
  assert(atomic_load(&ring.missing) >= 2);
  assert(atomic_load(&ring.consecutive_underruns) >= 2);
  assert(atomic_load(&ring.underrun_average_milli) > 0);

  state = (struct rpcr1_compat_state *)ring.converter;
  real_descriptor = state->descriptor;
  descriptor = copy_descriptor();
  descriptor.reconfigure = fail_reconfigure;
  state->descriptor = &descriptor;
  assert(rpcr_set_rates(&ring, 8000, 8000) != 0);
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.push_sample = fail_push_sample;
  state->descriptor = &descriptor;
  assert(!rpcr_producer_push_sample(&ring, 77));
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.push = fail_push;
  state->descriptor = &descriptor;
  rpcr_write(&ring, source, 1);
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.pop_sample = fail_pop_sample;
  state->descriptor = &descriptor;
  assert(!rpcr_consumer_pop_sample(&ring, &sample));
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.render_sample = fail_render_sample;
  state->descriptor = &descriptor;
  output[0] = 123;
  assert(!rpcr_consumer_render_sample(&ring, &output[0], 512));
  assert(output[0] == 0);
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.render = fail_render;
  state->descriptor = &descriptor;
  assert(!rpcr_render(&ring, output, 1, 0, 512));
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.conceal = fail_conceal;
  state->descriptor = &descriptor;
  for (index = 0; index <= ring.capacity; ++index)
    output[index] = INT16_MAX;
  assert(!rpcr_render(&ring, output, ring.capacity + 1, 0, 512));
  for (index = 0; index <= ring.capacity; ++index)
    assert(output[index] == 0);
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.record_shortfall = fail_record_shortfall;
  state->descriptor = &descriptor;
  rpcr_record_shortfall(&ring, 1, 1, 8000);
  state->descriptor = real_descriptor;

  descriptor = copy_descriptor();
  descriptor.observe = fail_observe;
  state->descriptor = &descriptor;
  assert(rpcr_producer_push_sample(&ring, 77));
  assert(rpcr_available(&ring) == 0);
  rpcr_observe(&ring, &observation);
  assert(!observation.capacity_samples);
  rpcr_record_shortfall(&ring, 1, 1, 8000);
  state->descriptor = real_descriptor;

  rpcr_destroy(&ring);
  rpcr_destroy(&invalid);
}

static void test_defensive_compatibility_states(void) {
  struct rpcr_ring ring = {.capacity = 1};
  struct rpcr1_compat_state *state;
  struct rpcr1_bridge_descriptor invalid_descriptor = copy_descriptor();

  assert(rpcr_available(&ring) == 0);

  state = calloc(1, sizeof(*state));
  assert(state);
  state->descriptor = __real_rpcr1_bridge_descriptor();
  ring.converter = (struct SRC_STATE_tag *)state;
  assert(rpcr_available(&ring) == 0);
  rpcr_destroy(&ring);

  state = calloc(1, sizeof(*state));
  assert(state);
  invalid_descriptor.struct_size = 0;
  state->descriptor = &invalid_descriptor;
  state->ring = (struct rpcr1_bridge_ring *)(uintptr_t)1;
  ring = (struct rpcr_ring){.capacity = 1,
                            .converter = (struct SRC_STATE_tag *)state};
  assert(rpcr_available(&ring) == 0);
  rpcr_destroy(&ring);
}

int main(void) {
  test_initialization_validation();
  test_forwarded_operations();
  test_defensive_compatibility_states();
  return 0;
}
