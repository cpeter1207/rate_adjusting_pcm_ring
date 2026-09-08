/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Lock-free single-producer/single-consumer PCM ring with clock
 * recovery.
 */
#ifndef RATE_ADJUSTING_PCM_RING_H
#define RATE_ADJUSTING_PCM_RING_H

#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

struct SRC_STATE_tag;

/** @brief libsamplerate quality selections supported by the ring. */
enum rpcr_quality {
  RPCR_SINC_BEST,
  RPCR_SINC_MEDIUM,
  RPCR_SINC_FASTEST,
};

/** @brief One preallocated SPSC PCM ring and its consumer-only rate controller.
 *
 * Producer release-ordering publishes complete storage writes. The sole
 * consumer acquires that cursor and advances @ref read. Render and write are
 * therefore lock-free and allocation-free. Persistent conversion retains
 * history across callbacks, avoiding frame-boundary artifacts.
 */
struct rpcr_ring {
  int16_t *storage; /**< Owned PCM storage. */
  float *input;     /**< Owned consumer converter workspace. */
  float *output;    /**< Owned consumer converter workspace. */
  size_t capacity;  /**< Storage and workspace capacity in samples. */
  atomic_uint_fast64_t
      written; /**< Producer-owned monotonically increasing cursor. */
  atomic_uint_fast64_t
      read; /**< Consumer-owned monotonically increasing cursor. */
  struct SRC_STATE_tag
      *converter; /**< Consumer-owned persistent libsamplerate state. */
  uint64_t occupancy_milli; /**< Consumer-owned filtered occupancy. */
  double ratio;             /**< Consumer-owned filtered source ratio. */
  bool primed;              /**< Consumer starts only after target occupancy. */
};

/** @brief Allocate a ring and persistent converter outside real-time
 * processing.
 * @param ring Zeroed destination.
 * @param capacity PCM samples retained and converted per callback at most.
 * @param quality libsamplerate quality selection.
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

/** @brief Write bounded PCM without blocking, preserving the newest samples on
 * overrun.
 * @param ring Initialized ring with exactly one producer.
 * @param input PCM samples to publish.
 * @param samples Number of samples.
 */
void rpcr_write(struct rpcr_ring *ring, const int16_t *input, size_t samples);

/** @brief Render one fixed hardware-paced PCM block with a slowly corrected
 * ratio.
 * @param ring Initialized ring with exactly one consumer.
 * @param output Destination PCM block.
 * @param samples Requested output samples, not exceeding ring capacity.
 * @param reserve Minimum input samples retained to prevent underrun.
 * @param target Target ring occupancy used for clock correction and priming.
 * @return True when audio was rendered; false writes silence while unprimed or
 * empty.
 *
 * Priming waits for @p target samples and then preserves @p reserve samples.
 * Occupancy and ratio each have slow filters, making independent source and
 * hardware-clock correction inaudible rather than periodically dropping PCM.
 */
bool rpcr_render(struct rpcr_ring *ring, int16_t *output, size_t samples,
                 size_t reserve, size_t target);

#endif
