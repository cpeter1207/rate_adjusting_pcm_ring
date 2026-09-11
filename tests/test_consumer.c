/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Verify that an installed shared library exposes the public
 * sample-at-a-time ABI.
 */
#include "rate_adjusting_pcm_ring.h"
#include <assert.h>

int main(void) {
  struct rpcr_ring ring;
  int16_t output = 0;

  assert(rpcr_init(&ring, 1024, RPCR_SINC_BEST) == 0);
  assert(rpcr_set_rates(&ring, 8000, 8000) == 0);
  assert(rpcr_producer_push_sample(&ring, 123));
  assert(rpcr_consumer_pop_sample(&ring, &output));
  assert(output == 123);
  for (size_t index = 0; index < 1024; ++index)
    assert(rpcr_producer_push_sample(&ring, (int16_t)index));
  assert(rpcr_consumer_render_sample(&ring, &output, 480));
  rpcr_destroy(&ring);
  return 0;
}
