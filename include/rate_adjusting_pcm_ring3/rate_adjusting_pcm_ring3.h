/**
 * @file rate_adjusting_pcm_ring3.h
 * @brief Canonical-F32 lock-free rate-adjusting PCM ring ABI.
 *
 * ABI major three owns all implementation state in a Rust shared object.  PCM
 * buffers contain mono IEEE-754 binary32 samples in the normalized range
 * -1.0 through +1.0.  A producer and consumer must each have exactly one
 * caller.  Creation and destruction occur only while both endpoints are
 * stopped; every producer and consumer function is allocation-free and
 * lock-free after successful creation.
 */

#ifndef RATE_ADJUSTING_PCM_RING3_H
#define RATE_ADJUSTING_PCM_RING3_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** @brief ABI implemented by @ref rpcr3_descriptor. */
#define RPCR3_ABI_VERSION 3U
/** @brief Exact capability literal supplied by @ref rpcr3_descriptor. */
#define RPCR3_CAPABILITY_NAME "rptadv.rate-adjusting-pcm-ring.f32"

/** @brief Smallest permitted preallocated source-sample workspace. */
#define RPCR3_MINIMUM_CAPACITY_SAMPLES 512U
/** @brief Largest capacity accepted by the selected adapter's count ABI. */
#define RPCR3_MAXIMUM_CAPACITY_SAMPLES UINT32_MAX

/** @brief Opaque Rust-owned PCM ring. */
struct rpcr3_ring;

/** @brief Result returned by a descriptor operation. */
enum rpcr3_result {
  /** Operation completed successfully. */
  RPCR3_OK = 0,
  /** A pointer, structure size, rate, mode, buffer policy, or count was
     invalid. */
  RPCR3_INVALID_ARGUMENT = -1,
  /** Control-plane allocation could not reserve required storage. */
  RPCR3_NO_MEMORY = -2,
  /** The required dynamic sample-rate adapter was unavailable or rejected
     setup. */
  RPCR3_ADAPTER_ERROR = -3,
};

/** @brief Immutable missing-output policy. */
enum rpcr3_plc_mode {
  /** Emit silence on shortfall with no PLC history or algorithmic delay. */
  RPCR3_PLC_DISABLED = 0,
  /** Use G.711 Appendix I at the output rate with 3.75 ms lookahead. */
  RPCR3_PLC_G711_APPENDIX_I = 1,
};

#if defined(__cplusplus)
static_assert(sizeof(enum rpcr3_result) == sizeof(int32_t),
              "rpcr3_result must use the Rust c_int ABI");
static_assert(sizeof(enum rpcr3_plc_mode) == sizeof(int32_t),
              "rpcr3_plc_mode must use the Rust c_int ABI");
#else
_Static_assert(sizeof(enum rpcr3_result) == sizeof(int32_t),
               "rpcr3_result must use the Rust c_int ABI");
_Static_assert(sizeof(enum rpcr3_plc_mode) == sizeof(int32_t),
               "rpcr3_plc_mode must use the Rust c_int ABI");
#endif

/**
 * @brief Immutable ring setup supplied before either endpoint starts.
 *
 * All fields remain fixed for the ring lifetime. Prepare a new ring to change
 * rates, timing, block bounds or concealment. Conversion always uses the
 * required dynamic adapter's SRC_LINEAR implementation.
 *
 * Rates must be nonzero with output/input ratio from 1/256 through 256.
 * Both block maxima must be nonzero; reserve and target must not exceed
 * capacity. Enabled PLC additionally requires reserve at least
 * ceil(max_output_samples / max(output_rate_hz / input_rate_hz * 0.999,
 * 1/256)) + 1, target at least reserve, and capacity at least target plus
 * max_producer_samples. These bounds permit one primed callback and one
 * producer burst at or below target; they do not guarantee against arbitrary
 * jitter. PLC history and delay are separately allocated at the output rate.
 */
struct rpcr3_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Required descriptor ABI version. */
  uint32_t abi_version;
  /** Fixed SPSC storage and conversion-workspace size in input samples, from
   * @ref RPCR3_MINIMUM_CAPACITY_SAMPLES through
   * @ref RPCR3_MAXIMUM_CAPACITY_SAMPLES. */
  uint64_t capacity_samples;
  /** Immutable producer sample rate in Hz. */
  uint32_t input_rate_hz;
  /** Immutable consumer sample rate in Hz. */
  uint32_t output_rate_hz;
  /** Initial and post-reset priming threshold, in input samples. */
  uint64_t reserve_samples;
  /** Input-sample occupancy target; zero disables clock correction. */
  uint64_t target_samples;
  /** Largest accepted producer block, in input samples. */
  uint64_t max_producer_samples;
  /** Largest accepted render block, in output samples. */
  uint64_t max_output_samples;
  /** One @ref rpcr3_plc_mode value. */
  uint32_t plc_mode;
};

/**
 * @brief Individually current, lock-free diagnostic snapshot.
 *
 * Fields are not transactionally coherent because producer and consumer keep
 * running while this snapshot is copied.  That property keeps diagnostics out
 * of every audio critical section.
 */
struct rpcr3_observation {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Descriptor ABI that populated this snapshot. */
  uint32_t abi_version;
  /** Immutable input-sample capacity. */
  uint64_t capacity_samples;
  /** Input samples currently available to the consumer. */
  uint64_t available_samples;
  /** Immutable input-sample playout priming reserve. */
  uint64_t reserve_samples;
  /** Low-pass filtered input occupancy used by the rate controller. */
  uint64_t filtered_occupancy_samples;
  /** Immutable input-sample occupancy target. */
  uint64_t target_samples;
  /** Applied ratio correction relative to nominal conversion, in parts per
   * million. */
  int32_t ratio_correction_ppm;
  /** Reserved for compatible extension; always zero in ABI major three. */
  uint32_t reserved;
  /** Producer samples rejected because unread input storage was full. */
  uint64_t discarded_samples;
  /** Output-rate converter shortfalls, excluding priming and PLC delay silence.
   */
  uint64_t missing_samples;
  /** Current contiguous converter-shortfall run in output samples. */
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
 * @ref RPCR3_ABI_VERSION, ensure @c struct_size is at least
 * @c sizeof(struct rpcr3_descriptor), compare @c capability_name exactly to
 * @ref RPCR3_CAPABILITY_NAME, and require every callback used by the caller
 * to be non-null.  A larger structure is a compatible future extension.
 *
 * All lifecycle functions are control-plane operations.  After creation,
 * producer and consumer calls perform no allocation, locking, I/O, logging,
 * or intentional panic.  Rendering is the only consumer endpoint, so the
 * ring always owns sample-rate conversion and clock-drift recovery.
 */
struct rpcr3_descriptor {
  /** Size of this descriptor, enabling compatible future extension. */
  uint32_t struct_size;
  /** ABI implemented by every function in this table. */
  uint32_t abi_version;
  /** Stable capability identifier for this implementation. */
  const char *capability_name;
  /**
   * @brief Create one stopped ring and its persistent converter.
   * @param config Immutable creation configuration.
   * @param out_ring Destination for the newly owned ring on success; unchanged
   * on invalid arguments and null on allocation or adapter failure.
   * @return One @ref rpcr3_result value.
   */
  enum rpcr3_result (*ring_create)(const struct rpcr3_config *config,
                                   struct rpcr3_ring **out_ring);
  /**
   * @brief Destroy a stopped ring.
   * @param ring Ring previously obtained from @ref ring_create, or null.
   */
  void (*ring_destroy)(struct rpcr3_ring *ring);
  /**
   * @brief Try to append one normalized F32 producer sample.
   * @param ring Ring with exactly one active producer.
   * @param sample Canonical F32 input sample.
   * @param accepted True only if storage accepted the sample.
   * @return One @ref rpcr3_result value.
   */
  enum rpcr3_result (*ring_producer_push_sample)(struct rpcr3_ring *ring,
                                                 float sample, bool *accepted);
  /**
   * @brief Append a chronological producer block without waiting.
   * @param ring Ring with exactly one active producer.
   * @param input Canonical F32 input samples, or null only when @p samples is
   * zero.
   * @param samples Input count, at most the configured producer maximum.
   * @param accepted Destination for the number of accepted leading samples.
   * @return One @ref rpcr3_result value. Invalid arguments leave the ring and
   * @p accepted unchanged; valid writes to a full FIFO may accept a prefix.
   */
  enum rpcr3_result (*ring_producer_push)(struct rpcr3_ring *ring,
                                          const float *input, uint64_t samples,
                                          uint64_t *accepted);
  /**
   * @brief Render one output sample with persistent conversion and concealment.
   * @param ring Ring with exactly one active consumer.
   * @param sample Destination for one canonical F32 output sample.
   * @param real True when the emitted sample derives from real source PCM;
   * delayed priming or replacement samples are false.
   * @return One @ref rpcr3_result value.
   */
  enum rpcr3_result (*ring_consumer_render_sample)(struct rpcr3_ring *ring,
                                                   float *sample, bool *real);
  /**
   * @brief Render a bounded native-rate output block.
   * @param ring Ring with exactly one active consumer.
   * @param output Destination for @p samples canonical F32 outputs, or null
   * only for zero.
   * @param samples Output count, at most the configured render maximum.
   * @param real_samples Destination for the count derived from source PCM.
   * @return One @ref rpcr3_result value.
   *
   * Both render operations use the immutable reserve and target. PLC mode adds
   * 3.75 ms lookahead and attenuates sustained shortfalls to silence by 60 ms;
   * disabled mode emits silence immediately without PLC delay. Returning PCM
   * resumes without re-priming. Invalid arguments leave output, counters and
   * @p real_samples unchanged. Zero-length blocks are harmless.
   */
  enum rpcr3_result (*ring_consumer_render)(struct rpcr3_ring *ring,
                                            float *output, uint64_t samples,
                                            uint64_t *real_samples);
  /**
   * @brief End the current burst and require reserve priming before resuming.
   * @param ring Ring with exactly one active consumer.
   * @return One @ref rpcr3_result value.
   *
   * Pending PCM and concealment history are discarded even when reset fails.
   * On failure, rendering produces silence and reports @ref RPCR3_ADAPTER_ERROR
   * until a later reset succeeds. Serialize reset with every rendering call.
   */
  enum rpcr3_result (*ring_consumer_reset)(struct rpcr3_ring *ring);
  /**
   * @brief Copy a best-effort observation without stopping either endpoint.
   * @param ring Ring to observe.
   * @param observation Caller-sized destination snapshot.
   * @return One @ref rpcr3_result value.
   */
  enum rpcr3_result (*ring_observe)(const struct rpcr3_ring *ring,
                                    struct rpcr3_observation *observation);
};

/**
 * @brief Return the immutable ABI-major-3 ring descriptor.
 * @return A process-lifetime descriptor that must not be modified or freed.
 */
const struct rpcr3_descriptor *rpcr3_descriptor(void);

#ifdef __cplusplus
}
#endif

#endif
