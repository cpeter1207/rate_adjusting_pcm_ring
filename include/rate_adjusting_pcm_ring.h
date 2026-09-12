/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Frozen ABI-major-one S16 compatibility facade for the Rust PCM ring.
 */
#ifndef RATE_ADJUSTING_PCM_RING_H
#define RATE_ADJUSTING_PCM_RING_H

#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Retained only because the released public layout exposed this pointer type.
 */
struct SRC_STATE_tag;

/** @brief Published converter-quality values forwarded to the Rust core. */
enum rpcr_quality {
  RPCR_SINC_BEST,
  RPCR_SINC_MEDIUM,
  RPCR_SINC_FASTEST,
};

/** @brief Lock-free consumer/controller measurements captured by @ref
 * rpcr_observe. */
struct rpcr_observation {
  size_t capacity_samples;  /**< Fixed ring capacity. */
  size_t available_samples; /**< PCM currently readable from the producer. */
  size_t reserve_samples;   /**< Current protected PCM floor. */
  size_t filtered_occupancy_samples; /**< Slowly filtered clock-controller
                                        occupancy. */
  size_t target_samples;    /**< Latest requested clock-recovery target. */
  int ratio_correction_ppm; /**< Applied source-ratio correction in parts per
                               million. */
};

/** @brief Frozen ABI-major-one S16 compatibility layout.
 *
 * This nonopaque structure remains unchanged so already-built consumers retain
 * their released ABI. New code must use the opaque canonical-F32 ABI-major-two
 * descriptor. The C facade stores a private Rust-owned handle in this layout
 * and mirrors the documented atomic diagnostics. Callers must not inspect or
 * modify compatibility-reserved storage or controller members, except that
 * legacy consumers may atomically set @ref reserve_samples before rendering.
 */
struct rpcr_ring {
  int16_t *storage; /**< @deprecated Compatibility-reserved state. */
  float *input;     /**< @deprecated Compatibility-reserved state. */
  float *output;    /**< @deprecated Compatibility-reserved state. */
  int16_t *history; /**< @deprecated Compatibility-reserved state. */
  size_t capacity;  /**< Storage and workspace capacity in samples. */
  atomic_uint_fast64_t written; /**< Mirrored monotonically increasing producer
                                    cursor. */
  atomic_uint_fast64_t read;    /**< Mirrored monotonically increasing consumer
                                    cursor. */
  atomic_uint_fast64_t discarded; /**< Mirrored rejected incoming samples. */
  atomic_uint_fast64_t
      missing; /**< Mirrored consumer-observed output shortfall samples. */
  atomic_uint_fast64_t
      consecutive_underruns; /**< Mirrored current contiguous shortfall. */
  atomic_uint_fast64_t
      underrun_average_milli; /**< Mirrored ten-second shortfall EWMA. */
  atomic_uint_fast64_t reserve_samples; /**< Legacy caller-selected retained
                                           input reserve. */
  atomic_uint_fast64_t
      target_samples; /**< Legacy consumer-selected clock-recovery target. */
  atomic_uint_fast64_t
      filtered_occupancy_milli; /**< Mirrored filtered occupancy in
                                   millisamples. */
  atomic_int
      ratio_correction_ppm; /**< Mirrored signed source-ratio correction. */
  struct SRC_STATE_tag
      *converter; /**< Private C-facade handle; type retained for ABI layout. */
  uint64_t occupancy_milli; /**< @deprecated Compatibility-reserved state. */
  double ratio;             /**< @deprecated Compatibility-reserved state. */
  size_t input_offset;      /**< @deprecated Compatibility-reserved state. */
  size_t input_pending;     /**< @deprecated Compatibility-reserved state. */
  size_t output_offset;     /**< @deprecated Compatibility-reserved state. */
  size_t output_pending;    /**< @deprecated Compatibility-reserved state. */
  size_t history_length;    /**< @deprecated Compatibility-reserved state. */
  size_t history_next;      /**< @deprecated Compatibility-reserved state. */
  size_t plc_period;        /**< @deprecated Compatibility-reserved state. */
  size_t plc_samples;       /**< @deprecated Compatibility-reserved state. */
  size_t recovery_samples;  /**< @deprecated Compatibility-reserved state. */
  unsigned int input_rate;  /**< PCM rate written by the producer. */
  unsigned int output_rate; /**< PCM rate rendered by the consumer. */
  bool primed; /**< @deprecated Retained source compatibility state; it no
                  longer gates playout. */
};

/** @brief Allocate a ring and persistent converter outside real-time
 * processing.
 * @param ring Zeroed destination.
 * @param capacity PCM samples retained and converted per callback at most.
 * @param quality Published converter-quality selection.
 * @return Zero on success, minus one on invalid input or allocation/converter
 * failure.
 */
int rpcr_init(struct rpcr_ring *ring, size_t capacity,
              enum rpcr_quality quality);

/** @brief Release all preallocated storage after producer and consumer have
 * stopped.
 * @param ring Initialized or zeroed ring.
 */
void rpcr_destroy(struct rpcr_ring *ring);

/** @brief Publish one source PCM sample without waiting.
 * @return True when the sample was published, false when the ring is full.
 */
bool rpcr_producer_push_sample(struct rpcr_ring *ring, int16_t sample);

/** @brief Consume one raw source sample without waiting.
 * @return True when a source sample was acquired.
 */
bool rpcr_consumer_pop_sample(struct rpcr_ring *ring, int16_t *sample);

/** @brief Render one hardware-paced PCM sample with persistent conversion.
 *
 * The consumer begins using source PCM immediately; @p target controls only
 * slow clock correction. A shortfall produces one concealed sample.
 * @return True when the output came from real converted source PCM.
 */
bool rpcr_consumer_render_sample(struct rpcr_ring *ring, int16_t *output,
                                 size_t target);

/** @brief Publish bounded PCM without blocking or overwriting unread storage.
 *
 * This compatibility wrapper loops over @ref rpcr_producer_push_sample.
 * @param ring Initialized ring with exactly one producer.
 * @param input PCM samples to publish.
 * @param samples Number of samples.
 */
void rpcr_write(struct rpcr_ring *ring, const int16_t *input, size_t samples);

/** @brief Set one shared PCM rate for producer and consumer.
 * @param ring Initialized ring whose producer and consumer are stopped.
 * @param sample_rate PCM sample rate in Hz.
 * @return Zero on success, or minus one for an invalid rate or a failed
 * persistent-adapter reset.
 *
 * This compatibility shorthand calls @ref rpcr_set_rates with identical
 * input and output rates.  It must be set before the first call to
 * @ref rpcr_render.
 */
int rpcr_set_sample_rate(struct rpcr_ring *ring, unsigned int sample_rate);

/** @brief Set distinct producer and consumer PCM rates.
 * @param ring Initialized ring whose producer and consumer are stopped.
 * @param input_rate PCM rate written by @ref rpcr_write in Hz.
 * @param output_rate PCM rate rendered by @ref rpcr_render in Hz.
 * @return Zero on success, or minus one for an invalid rate.
 *
 * The persistent dynamically linked adapter performs nominal conversion and
 * slow occupancy-driven correction together. Consequently a producer such as an
 * 8 kHz Asterisk channel can feed a native-rate hardware callback without an
 * intermediate converted PCM queue.  Capacity, reserve, target, and @ref
 * rpcr_available remain expressed in input samples.  This function must be
 * called before the first render, or after both endpoints stop.
 */
int rpcr_set_rates(struct rpcr_ring *ring, unsigned int input_rate,
                   unsigned int output_rate);

/** @brief Render one fixed hardware-paced PCM block with a slowly corrected
 * ratio.
 * @param ring Initialized ring with exactly one consumer.
 * @param output Destination PCM block.
 * @param samples Requested output samples, not exceeding ring capacity.
 * @param reserve Retained for source compatibility; no longer gates playout.
 * @param target Target ring occupancy used for clock correction.
 * @return Number of real source samples rendered. Any shortfall is filled by
 * consumer-side pitch waveform concealment when recent PCM history exists.
 * Zero writes silence only before the first usable PCM history or after its
 * bounded concealment tail has faded.
 *
 * This compatibility wrapper loops over @ref rpcr_consumer_render_sample.
 */
size_t rpcr_render(struct rpcr_ring *ring, int16_t *output, size_t samples,
                   size_t reserve, size_t target);

/** @brief Return published PCM available to the consumer.
 * @param ring Initialized ring.
 * @return Readable sample count, bounded by capacity.
 */
size_t rpcr_available(const struct rpcr_ring *ring);

/** @brief Copy lock-free occupancy and source-rate-controller measurements.
 * @param ring Initialized ring, or null to return an all-zero observation.
 * @param observation Destination for one diagnostic snapshot, or null to
 * discard it.
 *
 * Producer and consumer continue independently while this function runs.
 * Fields are individually current rather than transactionally coherent, so
 * live diagnostics never put a lock in an audio path.
 */
void rpcr_observe(const struct rpcr_ring *ring,
                  struct rpcr_observation *observation);

/** @brief Update observable output-shortfall statistics after one callback.
 * @param ring Initialized consumer-owned ring.
 * @param missing PCM samples unavailable to the callback.
 * @param samples Requested callback length.
 * @param rate Callback sample rate in Hz for the ten-second EWMA window.
 */
void rpcr_record_shortfall(struct rpcr_ring *ring, size_t missing,
                           size_t samples, unsigned int rate);

#endif
