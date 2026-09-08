/* SPDX-License-Identifier: GPL-2.0-only */
/** @file
 * @brief Verify that an installed shared library has a usable ABI.
 */
#include "rate_adjusting_pcm_ring.h"
#include <assert.h>

int main(void) {
  struct rpcr_ring ring;
  assert(rpcr_init(&ring, 64, RPCR_SINC_BEST) == 0);
  rpcr_destroy(&ring);
  return 0;
}
