/**
 * @file rate_adjusting_pcm_ring2.h
 * @brief Canonical-F32 lock-free rate-adjusting PCM ring ABI.
 *
 * ABI major two owns all implementation state in a Rust shared object.  PCM
 * buffers contain mono IEEE-754 binary32 samples in the normalized range
 * -1.0 through +1.0.  A producer and consumer must each have exactly one
 * caller.  Creation and destruction occur only while both endpoints are
 * stopped; every producer and consumer function is allocation-free and
 * lock-free after successful creation.
 */

#ifndef RATE_ADJUSTING_PCM_RING2_H
#define RATE_ADJUSTING_PCM_RING2_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** @brief ABI implemented by @ref rpcr2_descriptor. */
#define RPCR2_ABI_VERSION 2U
/** @brief Exact capability literal supplied by @ref rpcr2_descriptor. */
#define RPCR2_CAPABILITY_NAME "rptadv.rate-adjusting-pcm-ring.f32"

/** @brief Smallest permitted preallocated source-sample workspace. */
#define RPCR2_MINIMUM_CAPACITY_SAMPLES 512U
/** @brief Largest capacity accepted by the selected adapter's count ABI. */
#define RPCR2_MAXIMUM_CAPACITY_SAMPLES UINT32_MAX

/** @brief Opaque Rust-owned PCM ring. */
struct rpcr2_ring;

/** @brief Result returned by a descriptor operation. */
enum rpcr2_result {
  /** Operation completed successfully. */
  RPCR2_OK = 0,
  /** A pointer, structure size, rate, quality, or count was invalid. */
  RPCR2_INVALID_ARGUMENT = -1,
  /** Control-plane allocation could not reserve required storage. */
  RPCR2_NO_MEMORY = -2,
  /** The required dynamic sample-rate adapter was unavailable or rejected
     setup. */
  RPCR2_ADAPTER_ERROR = -3,
};

/** @brief Persistent conversion quality supplied to the selected adapter. */
enum rpcr2_quality {
  /** Highest quality offered by the selected adapter. */
  RPCR2_QUALITY_BEST = 0,
  /** Balanced quality and CPU use offered by the selected adapter. */
  RPCR2_QUALITY_MEDIUM = 1,
  /** Lowest-latency quality offered by the selected adapter. */
  RPCR2_QUALITY_FASTEST = 2,
};

#if defined(__cplusplus)
static_assert(sizeof(enum rpcr2_result) == sizeof(int32_t),
              "rpcr2_result must use the Rust c_int ABI");
static_assert(sizeof(enum rpcr2_quality) == sizeof(int32_t),
              "rpcr2_quality must use the Rust c_int ABI");
#else
_Static_assert(sizeof(enum rpcr2_result) == sizeof(int32_t),
               "rpcr2_result must use the Rust c_int ABI");
_Static_assert(sizeof(enum rpcr2_quality) == sizeof(int32_t),
               "rpcr2_quality must use the Rust c_int ABI");
#endif

/**
 * @brief Immutable ring setup supplied before either endpoint starts.
 *
 * The input and output rates remain fixed for the ring lifetime.  Change a
 * native rate by stopping and destroying this ring, then creating another.
 */
struct rpcr2_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Required descriptor ABI version. */
  uint32_t abi_version;
  /** Fixed SPSC storage and conversion-workspace size in input samples, from
   * @ref RPCR2_MINIMUM_CAPACITY_SAMPLES through
   * @ref RPCR2_MAXIMUM_CAPACITY_SAMPLES. */
  uint64_t capacity_samples;
  /** Immutable producer sample rate in Hz. */
  uint32_t input_rate_hz;
  /** Immutable consumer sample rate in Hz. */
  uint32_t output_rate_hz;
  /** One @ref rpcr2_quality value. */
  uint32_t quality;
};

/**
 * @brief Individually current, lock-free diagnostic snapshot.
 *
 * Fields are not transactionally coherent because producer and consumer keep
 * running while this snapshot is copied.  That property keeps diagnostics out
 * of every audio critical section.
 */
struct rpcr2_observation {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Descriptor ABI that populated this snapshot. */
  uint32_t abi_version;
  /** Immutable input-sample capacity. */
  uint64_t capacity_samples;
  /** Input samples currently available to the consumer. */
  uint64_t available_samples;
  /** Latest caller-selected protected reserve, for diagnostics only. */
  uint64_t reserve_samples;
  /** Low-pass filtered input occupancy used by the rate controller. */
  uint64_t filtered_occupancy_samples;
  /** Latest caller-selected occupancy target. */
  uint64_t target_samples;
  /** Applied ratio correction relative to nominal conversion, in parts per
   * million. */
  int32_t ratio_correction_ppm;
  /** Reserved for compatible extension; always zero in ABI major two. */
  uint32_t reserved;
  /** Producer samples rejected because unread input storage was full. */
  uint64_t discarded_samples;
  /** Consumer output samples supplied by loss concealment. */
  uint64_t missing_samples;
  /** Current contiguous concealed-output run in samples. */
  uint64_t consecutive_shortfall_samples;
  /** Ten-second consecutive-shortfall moving average in millisamples. */
  uint64_t shortfall_average_milli;
  /** Consumer sample-rate-adapter calls that failed after construction. */
  uint64_t adapter_error_count;
};

/**
 * @brief Versioned function table exported by the Rust shared object.
 *
 * Before calling a function, validate @c abi_version against
 * @ref RPCR2_ABI_VERSION, ensure @c struct_size is at least
 * @c sizeof(struct rpcr2_descriptor), compare @c capability_name exactly to
 * @ref RPCR2_CAPABILITY_NAME, and require every callback used by the caller
 * to be non-null.  A larger structure is a compatible future extension.
 *
 * All lifecycle functions are control-plane operations.  After creation,
 * producer and consumer calls perform no allocation, locking, I/O, logging,
 * or intentional panic.  Rendering is the only consumer endpoint, so the
 * ring always owns sample-rate conversion and clock-drift recovery.
 */
struct rpcr2_descriptor {
  /** Size of this descriptor, enabling compatible future extension. */
  uint32_t struct_size;
  /** ABI implemented by every function in this table. */
  uint32_t abi_version;
  /** Stable capability identifier for this implementation. */
  const char *capability_name;
  /**
   * @brief Create one stopped ring and its persistent converter.
   * @param config Immutable creation configuration.
   * @param out_ring Destination for the newly owned ring on success.
   * @return One @ref rpcr2_result value.
   */
  enum rpcr2_result (*ring_create)(const struct rpcr2_config *config,
                                   struct rpcr2_ring **out_ring);
  /**
   * @brief Destroy a stopped ring.
   * @param ring Ring previously obtained from @ref ring_create, or null.
   */
  void (*ring_destroy)(struct rpcr2_ring *ring);
  /**
   * @brief Try to append one normalized F32 producer sample.
   * @param ring Ring with exactly one active producer.
   * @param sample Canonical F32 input sample.
   * @param accepted True only if storage accepted the sample.
   * @return One @ref rpcr2_result value.
   */
  enum rpcr2_result (*ring_producer_push_sample)(struct rpcr2_ring *ring,
                                                 float sample, bool *accepted);
  /**
   * @brief Append a chronological producer block without waiting.
   * @param ring Ring with exactly one active producer.
   * @param input Canonical F32 input samples, or null only when @p samples is
   * zero.
   * @param samples Number of input samples.
   * @param accepted Destination for the number of accepted leading samples.
   * @return One @ref rpcr2_result value.
   */
  enum rpcr2_result (*ring_producer_push)(struct rpcr2_ring *ring,
                                          const float *input, uint64_t samples,
                                          uint64_t *accepted);
  /**
   * @brief Render one output sample with persistent conversion and concealment.
   * @param ring Ring with exactly one active consumer.
   * @param sample Destination for one canonical F32 output sample.
   * @param target_samples Input occupancy target for slow ratio correction.
   * @param real True when output derives from converted source PCM.
   * @return One @ref rpcr2_result value.
   */
  enum rpcr2_result (*ring_consumer_render_sample)(struct rpcr2_ring *ring,
                                                   float *sample,
                                                   uint64_t target_samples,
                                                   bool *real);
  /**
   * @brief Render an arbitrary native-rate output block.
   * @param ring Ring with exactly one active consumer.
   * @param output Destination for @p samples canonical F32 outputs, or null
   * only for zero.
   * @param samples Arbitrary requested output-sample count.
   * @param reserve_samples Protected reserve published for diagnostics only.
   * @param target_samples Input occupancy target for slow ratio correction.
   * @param real_samples Destination for the count derived from source PCM.
   * @return One @ref rpcr2_result value.
   *
   * Reserve and target never gate initial playout; target only gently adjusts
   * persistent conversion ratio.  Any source shortfall is concealed and
   * internally accounted exactly once by this rendering API.
   */
  enum rpcr2_result (*ring_consumer_render)(struct rpcr2_ring *ring,
                                            float *output, uint64_t samples,
                                            uint64_t reserve_samples,
                                            uint64_t target_samples,
                                            uint64_t *real_samples);
  /**
   * @brief Copy a best-effort observation without stopping either endpoint.
   * @param ring Ring to observe.
   * @param observation Caller-sized destination snapshot.
   * @return One @ref rpcr2_result value.
   */
  enum rpcr2_result (*ring_observe)(const struct rpcr2_ring *ring,
                                    struct rpcr2_observation *observation);
};

/**
 * @brief Return the immutable ABI-major-2 ring descriptor.
 * @return A process-lifetime descriptor that must not be modified or freed.
 */
const struct rpcr2_descriptor *rpcr2_descriptor(void);

#ifdef __cplusplus
}
#endif

#endif
