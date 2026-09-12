/* SPDX-License-Identifier: GPL-2.0-only */
/**
 * @file
 * @brief Frozen ABI-major-one forwarding shim for the Rust-owned PCM ring.
 *
 * This file retains the released S16 structure layout and symbol names while
 * forwarding every substantive ring operation through the private descriptor
 * exported by the Rust ABI-major-two shared object. It owns only C-ABI
 * validation, layout mirroring, and the S16 boundary required by old users.
 */

#include "rate_adjusting_pcm_ring.h"

#include <stddef.h>
#include <stdlib.h>
#include <string.h>

/* The frozen public atomics remain on every ABI-major-one audio operation. */
_Static_assert(ATOMIC_INT_LOCK_FREE == 2 && ATOMIC_LONG_LOCK_FREE == 2 &&
                   ATOMIC_LLONG_LOCK_FREE == 2,
               "ABI-major-one PCM facade requires lock-free callback atomics");

/** @brief Private descriptor ABI accepted by the C forwarding facade. */
#define RPCR1_BRIDGE_ABI_VERSION 1U
/** @brief Private descriptor capability accepted by the C forwarding facade. */
#define RPCR1_BRIDGE_CAPABILITY "rptadv.rate-adjusting-pcm-ring.legacy-s16"

/** Opaque Rust-owned legacy forwarding state. */
struct rpcr1_bridge_ring;

/** Rust snapshot mirrored into the released C structure's public atomics. */
struct rpcr1_bridge_observation {
  uint64_t capacity_samples;  /**< Fixed input capacity. */
  uint64_t available_samples; /**< Input samples readable by the consumer. */
  uint64_t written_samples;   /**< Producer's monotonic source cursor. */
  uint64_t read_samples;      /**< Consumer's monotonic source cursor. */
  uint64_t reserve_samples;   /**< Rust-side retained-reserve diagnostic. */
  uint64_t filtered_occupancy_samples; /**< Low-pass occupancy diagnostic. */
  uint64_t target_samples;             /**< Rust-side rate-controller target. */
  int32_t ratio_correction_ppm;        /**< Applied rate correction. */
  uint64_t discarded_samples;          /**< Rejected source samples. */
  uint64_t missing_samples;            /**< Compatibility shortfall samples. */
  uint64_t consecutive_underruns;      /**< Current contiguous shortfall. */
  uint64_t underrun_average_milli;     /**< Ten-second shortfall average. */
};

/** Private C-compatible function table implemented by the Rust core. */
struct rpcr1_bridge_descriptor {
  uint32_t struct_size;        /**< Size for compatible private extension. */
  uint32_t abi_version;        /**< Required private bridge ABI. */
  const char *capability_name; /**< Exact private capability literal. */
  /** Create one stopped Rust-owned S16 bridge handle. */
  int (*create)(uint64_t capacity, uint32_t quality, uint32_t input_rate_hz,
                uint32_t output_rate_hz, struct rpcr1_bridge_ring **out_ring);
  /** Destroy a stopped Rust-owned bridge handle. */
  void (*destroy)(struct rpcr1_bridge_ring *ring);
  /** Reset one stopped handle for a fixed-rate change. */
  int (*reconfigure)(struct rpcr1_bridge_ring *ring, uint32_t input_rate_hz,
                     uint32_t output_rate_hz);
  /** Publish one signed-16 source sample and snapshot. */
  int (*push_sample)(struct rpcr1_bridge_ring *ring, int16_t sample,
                     bool *accepted,
                     struct rpcr1_bridge_observation *observation);
  /** Publish a chronological signed-16 source block. */
  int (*push)(struct rpcr1_bridge_ring *ring, const int16_t *input,
              uint64_t samples, uint64_t *accepted);
  /** Consume one raw signed-16 source sample. */
  int (*pop_sample)(struct rpcr1_bridge_ring *ring, int16_t *output,
                    bool *available);
  /** Render one signed-16 output sample and snapshot. */
  int (*render_sample)(struct rpcr1_bridge_ring *ring, int16_t *output,
                       uint64_t target_samples, bool *real,
                       struct rpcr1_bridge_observation *observation);
  /** Render one signed-16 output block. */
  int (*render)(struct rpcr1_bridge_ring *ring, int16_t *output,
                uint64_t samples, uint64_t reserve_samples,
                uint64_t target_samples, uint64_t *real_samples);
  /** Advance only the legacy concealment state. */
  int (*conceal)(struct rpcr1_bridge_ring *ring, int16_t *output,
                 uint64_t samples);
  /** Copy one lock-free private observation. */
  int (*observe)(const struct rpcr1_bridge_ring *ring,
                 struct rpcr1_bridge_observation *observation);
  /** Account for one ABI-major-one callback shortfall. */
  int (*record_shortfall)(struct rpcr1_bridge_ring *ring, uint64_t missing,
                          uint64_t samples, uint32_t rate_hz);
};

/** Return the process-lifetime private bridge table from Rust. */
extern const struct rpcr1_bridge_descriptor *rpcr1_bridge_descriptor(void);

/** C shim control-plane state stored in the legacy converter field. */
struct rpcr1_compat_state {
  const struct rpcr1_bridge_descriptor *descriptor; /**< Rust callback table. */
  struct rpcr1_bridge_ring *ring; /**< Rust-owned ring handle. */
  bool configured;                /**< Rates have completed required setup. */
};

/** Select which SPSC endpoint may update its published cursor mirror. */
enum rpcr1_sync_owner {
  RPCR1_SYNC_PRODUCER, /**< Mirror producer-owned diagnostics only. */
  RPCR1_SYNC_CONSUMER, /**< Mirror consumer-owned diagnostics only. */
  RPCR1_SYNC_BOTH,     /**< Mirror both stopped-endpoint diagnostic groups. */
};

/** Return the C wrapper state without exposing it in the published header. */
static struct rpcr1_compat_state *compat_state(struct rpcr_ring *ring) {
  return ring ? (struct rpcr1_compat_state *)ring->converter : NULL;
}

/** Return a const wrapper state without qualifying away legacy callers. */
static const struct rpcr1_compat_state *
compat_state_const(const struct rpcr_ring *ring) {
  return ring ? (const struct rpcr1_compat_state *)ring->converter : NULL;
}

/** Verify the fixed private descriptor prefix before forwarding C calls. */
static bool descriptor_valid(const struct rpcr1_bridge_descriptor *descriptor) {
  return descriptor && descriptor->struct_size >= sizeof(*descriptor) &&
         descriptor->abi_version == RPCR1_BRIDGE_ABI_VERSION &&
         descriptor->capability_name &&
         !strcmp(descriptor->capability_name, RPCR1_BRIDGE_CAPABILITY) &&
         descriptor->create && descriptor->destroy && descriptor->reconfigure &&
         descriptor->push_sample && descriptor->push &&
         descriptor->pop_sample && descriptor->render_sample &&
         descriptor->render && descriptor->conceal && descriptor->observe &&
         descriptor->record_shortfall;
}

/** Return whether one initialized C facade still owns a Rust ring. */
static bool ring_valid(const struct rpcr_ring *ring) {
  const struct rpcr1_compat_state *state = compat_state_const(ring);
  return ring && ring->capacity && state && state->ring &&
         descriptor_valid(state->descriptor);
}

/** Copy a Rust observation after the public caller has validated the ring. */
static void sync_observation(struct rpcr_ring *ring,
                             const struct rpcr1_bridge_observation *observation,
                             enum rpcr1_sync_owner owner) {
  /*
   * Rust retains these fields in the private snapshot for ABI layout
   * completeness. The frozen public C atomics remain authoritative for them
   * so a legacy caller's direct reserve/target diagnostics are not replaced.
   */
  (void)observation->capacity_samples;
  (void)observation->reserve_samples;
  (void)observation->target_samples;
  if (owner != RPCR1_SYNC_CONSUMER) {
    atomic_store_explicit(&ring->written, observation->written_samples,
                          memory_order_release);
    atomic_store_explicit(&ring->discarded, observation->discarded_samples,
                          memory_order_relaxed);
  }
  if (owner != RPCR1_SYNC_PRODUCER) {
    atomic_store_explicit(&ring->read, observation->read_samples,
                          memory_order_release);
    atomic_store_explicit(&ring->missing, observation->missing_samples,
                          memory_order_relaxed);
    atomic_store_explicit(&ring->consecutive_underruns,
                          observation->consecutive_underruns,
                          memory_order_relaxed);
    atomic_store_explicit(&ring->underrun_average_milli,
                          observation->underrun_average_milli,
                          memory_order_relaxed);
    atomic_store_explicit(&ring->filtered_occupancy_milli,
                          observation->filtered_occupancy_samples * 1000U,
                          memory_order_relaxed);
    atomic_store_explicit(&ring->ratio_correction_ppm,
                          observation->ratio_correction_ppm,
                          memory_order_relaxed);
  }
}

/** Fetch then mirror one bounded snapshot for a block-level C operation. */
static bool fetch_observation(struct rpcr_ring *ring,
                              enum rpcr1_sync_owner owner) {
  struct rpcr1_compat_state *state = compat_state(ring);
  struct rpcr1_bridge_observation observation;
  if (state->descriptor->observe(state->ring, &observation))
    return false;
  sync_observation(ring, &observation, owner);
  return true;
}

/** Fill one caller-bounded S16 output span after a compatibility failure. */
static void clear_output(int16_t *output, size_t samples) {
  for (size_t index = 0; index < samples; ++index)
    output[index] = 0;
}

int rpcr_init(struct rpcr_ring *ring, size_t capacity,
              enum rpcr_quality quality) {
  const struct rpcr1_bridge_descriptor *descriptor;
  struct rpcr1_compat_state *state;
  if (!ring || !capacity || quality < RPCR_SINC_BEST ||
      quality > RPCR_SINC_FASTEST)
    return -1;
  *ring = (struct rpcr_ring){0};
  descriptor = rpcr1_bridge_descriptor();
  if (!descriptor_valid(descriptor))
    return -1;
  state = calloc(1, sizeof(*state));
  if (!state)
    return -1;
  state->descriptor = descriptor;
  /*
   * The old interface requires rpcr_set_rates before active rendering. A
   * harmless equal-rate setup lets Rust reserve the persistent converter at
   * initialization while preserving zero public rates until that call.
   */
  if (descriptor->create(capacity, (uint32_t)quality, 8000U, 8000U,
                         &state->ring)) {
    free(state);
    return -1;
  }
  ring->capacity = capacity;
  ring->converter = (struct SRC_STATE_tag *)state;
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
  struct rpcr1_compat_state *state;
  if (!ring)
    return;
  state = compat_state(ring);
  if (state) {
    if (descriptor_valid(state->descriptor) && state->ring)
      state->descriptor->destroy(state->ring);
    free(state);
  }
  *ring = (struct rpcr_ring){0};
}

bool rpcr_producer_push_sample(struct rpcr_ring *ring, int16_t sample) {
  struct rpcr1_compat_state *state = compat_state(ring);
  struct rpcr1_bridge_observation observation;
  bool accepted = false;
  if (!ring_valid(ring) || state->descriptor->push_sample(
                               state->ring, sample, &accepted, &observation))
    return false;
  sync_observation(ring, &observation, RPCR1_SYNC_PRODUCER);
  return accepted;
}

bool rpcr_consumer_pop_sample(struct rpcr_ring *ring, int16_t *sample) {
  struct rpcr1_compat_state *state = compat_state(ring);
  bool available = false;
  if (!sample || !ring_valid(ring) ||
      state->descriptor->pop_sample(state->ring, sample, &available))
    return false;
  (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
  return available;
}

void rpcr_write(struct rpcr_ring *ring, const int16_t *input, size_t samples) {
  struct rpcr1_compat_state *state = compat_state(ring);
  uint64_t accepted = 0;
  if (!input || !ring_valid(ring) ||
      state->descriptor->push(state->ring, input, samples, &accepted))
    return;
  (void)accepted;
  (void)fetch_observation(ring, RPCR1_SYNC_PRODUCER);
}

int rpcr_set_sample_rate(struct rpcr_ring *ring, unsigned int sample_rate) {
  return rpcr_set_rates(ring, sample_rate, sample_rate);
}

int rpcr_set_rates(struct rpcr_ring *ring, unsigned int input_rate,
                   unsigned int output_rate) {
  struct rpcr1_compat_state *state = compat_state(ring);
  if (!ring_valid(ring) || !input_rate || !output_rate ||
      state->descriptor->reconfigure(state->ring, input_rate, output_rate))
    return -1;
  ring->input_rate = input_rate;
  ring->output_rate = output_rate;
  state->configured = true;
  ring->occupancy_milli = 0;
  ring->ratio = 0.0;
  ring->input_offset = ring->input_pending = 0;
  ring->output_offset = ring->output_pending = 0;
  ring->plc_period = 0;
  ring->plc_samples = 0;
  ring->recovery_samples = 0;
  (void)fetch_observation(ring, RPCR1_SYNC_BOTH);
  return 0;
}

size_t rpcr_available(const struct rpcr_ring *ring) {
  const struct rpcr1_compat_state *state = compat_state_const(ring);
  struct rpcr1_bridge_observation observation;
  if (!ring_valid(ring) ||
      state->descriptor->observe(state->ring, &observation))
    return 0;
  return observation.available_samples < ring->capacity
             ? (size_t)observation.available_samples
             : ring->capacity;
}

void rpcr_observe(const struct rpcr_ring *ring,
                  struct rpcr_observation *observation) {
  const struct rpcr1_compat_state *state = compat_state_const(ring);
  struct rpcr1_bridge_observation values;
  if (!observation)
    return;
  *observation = (struct rpcr_observation){0};
  if (!ring_valid(ring) || state->descriptor->observe(state->ring, &values))
    return;
  observation->capacity_samples = ring->capacity;
  observation->available_samples = values.available_samples < ring->capacity
                                       ? (size_t)values.available_samples
                                       : ring->capacity;
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
  struct rpcr1_compat_state *state = compat_state(ring);
  if (!ring_valid(ring) ||
      state->descriptor->record_shortfall(state->ring, missing, samples, rate))
    return;
  (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
}

bool rpcr_consumer_render_sample(struct rpcr_ring *ring, int16_t *output,
                                 size_t target) {
  struct rpcr1_compat_state *state = compat_state(ring);
  struct rpcr1_bridge_observation observation;
  bool real = false;
  if (!output || !ring_valid(ring)) {
    if (output)
      *output = 0;
    return false;
  }
  atomic_store_explicit(&ring->target_samples, target, memory_order_relaxed);
  if (!state->configured) {
    *output = 0;
    (void)state->descriptor->record_shortfall(state->ring, 1, 1, 0);
    (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
    return false;
  }
  if (state->descriptor->render_sample(state->ring, output, target, &real,
                                       &observation)) {
    *output = 0;
    return false;
  }
  sync_observation(ring, &observation, RPCR1_SYNC_CONSUMER);
  return real;
}

size_t rpcr_render(struct rpcr_ring *ring, int16_t *output, size_t samples,
                   size_t reserve, size_t target) {
  struct rpcr1_compat_state *state = compat_state(ring);
  uint64_t rendered = 0;
  if (!output || !ring_valid(ring))
    return 0;
  atomic_store_explicit(&ring->reserve_samples, reserve, memory_order_relaxed);
  if (samples > ring->capacity) {
    if (state->descriptor->conceal(state->ring, output, samples))
      clear_output(output, samples);
    else
      (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
    return 0;
  }
  if (!samples)
    return 0;
  atomic_store_explicit(&ring->target_samples, target, memory_order_relaxed);
  if (!state->configured) {
    clear_output(output, samples);
    (void)state->descriptor->record_shortfall(state->ring, samples, samples, 0);
    (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
    return 0;
  }
  if (state->descriptor->render(state->ring, output, samples, reserve, target,
                                &rendered))
    return 0;
  (void)fetch_observation(ring, RPCR1_SYNC_CONSUMER);
  return rendered < samples ? (size_t)rendered : samples;
}
