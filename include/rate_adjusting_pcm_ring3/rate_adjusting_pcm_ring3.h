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
#include <stddef.h>
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

/** @brief Maximum intentional-mute intervals supplied in one producer call. */
#define RPCR3_MAXIMUM_MUTE_SPANS 64U
/** @brief Half-open absolute input-rate source interval, not a storage cursor.
 */
struct rpcr3_mute_span {
  uint64_t start; /**< First intentionally silent source sample. */
  uint64_t end;   /**< First following sample not in this interval. */
};

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
  /** Emit silence on shortfall with no PLC history or added PLC delay. */
  RPCR3_PLC_DISABLED = 0,
  /** Use G.711 Appendix I at the output rate with 3.75 ms lookahead. */
  RPCR3_PLC_G711_APPENDIX_I = 1,
};

/** @brief Acquired result for the latest finite-source boundary token. */
enum rpcr3_finish_status {
  RPCR3_FINISH_PENDING = 0,       /**< Accepted PCM is still retained. */
  RPCR3_FINISH_DRAINED = 1,       /**< FIR and PLC tail has played. */
  RPCR3_FINISH_CANCELLED = 2,     /**< Explicit consumer reset discarded it. */
  RPCR3_FINISH_ADAPTER_ERROR = 3, /**< Conversion or final reset failed. */
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
 * rates, timing, block bounds or concealment. Conversion uses dynamic adapter
 * ABI 2: libswresample with filter_size=16, cutoff=1.0 and a Kaiser filter.
 * Its reported FIR delay is separate from the configured reserve and PLC delay.
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
  /** Low-pass filtered input occupancy, including adapter backlog but
   * excluding its fixed FIR delay, used by the rate controller. */
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
  /** Mapped gaps and output-rate converter shortfalls, excluding priming and
   * PLC delay silence.
   */
  uint64_t missing_samples;
  /** Current contiguous missing-output run, including mapped source gaps. */
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
  /**
   * @brief Report fixed FIR delay in output samples, excluding PLC lookahead.
   * @param ring Live ring whose immutable conversion settings are queried.
   * @param samples Destination for the delay; unchanged on invalid arguments.
   * @return One @ref rpcr3_result value.
   *
   * Compatible descriptor extension introduced in 3.0.0-alpha.2. Validate
   * struct_size before accessing this callback. Finite-media consumers may
   * discard this prefix and supply enough trailing padding to retain the
   * source duration. The query performs no allocation or consumer-state access.
   */
  enum rpcr3_result (*ring_output_delay)(const struct rpcr3_ring *ring,
                                         uint64_t *samples);
  /**
   * @brief Append a counted missing-source span at the current producer
   * position.
   * @param ring Ring with exactly one active producer, serialized with PCM
   * writes.
   * @param input_samples Missing input-rate samples, within the producer
   * maximum.
   * @param accepted Destination for the accepted chronological prefix length.
   * @return One @ref rpcr3_result value. Invalid arguments change nothing.
   *
   * Compatible descriptor extension introduced in 3.0.0-alpha.3. Check the
   * gap minimum size before accessing this tail. Accepted gaps occupy FIFO
   * slots like PCM; retry an unaccepted suffix before publishing later PCM.
   * Gaps remain between older and following PCM through the persistent FIR
   * and clock compensation. Internal zero placeholders are never classified
   * as real output. Each mapped output is concealed once by the ring's selected
   * PLC (or silence when disabled). Source boundaries use the adapter's live
   * position quantized to billionths of a source frame, then floored; an exact
   * boundary belongs to the following span. Reset discards pending gaps.
   */
  enum rpcr3_result (*ring_producer_gap)(struct rpcr3_ring *ring,
                                         uint64_t input_samples,
                                         uint64_t *accepted);
  /**
   * @brief Publish source mute metadata before its accepted PCM prefix.
   * @param ring Ring with exactly one active producer.
   * @param input Canonical F32 input, null only for zero samples.
   * @param samples Input count within the configured producer maximum.
   * @param generation Identity for this source burst; a change requires a
   * completed consumer reset with the old producer quiescent.
   * @param source_start Absolute source position of input[0]. Repeated calls
   * must continue at the previously accepted prefix end, including counted
   * gaps.
   * @param spans Up to RPCR3_MAXIMUM_MUTE_SPANS intervals, null only for zero.
   * Each interval has start less than end, end at most source_start + samples,
   * and start no earlier than source_start minus capacity (saturating at zero).
   * @param span_count Number of intervals.
   * @param accepted Receives accepted PCM prefix length; unchanged on invalid
   * arguments.
   * @return RPCR3_OK on success, including zero credit; otherwise
   * RPCR3_INVALID_ARGUMENT.
   *
   * Retry the unaccepted suffix with its original source positions and
   * intervals. Bounded metadata backpressure accepts no PCM; it never waits or
   * overwrites. Intentional silence is not loss and does not request PLC or add
   * shortfalls. Tags follow actual converter source consumption through
   * cached/FIR and PLC delay. Insufficient input reserve may already have
   * played a qualifying prefix. Older PCM/gap operations continue this timeline
   * after tagging starts.
   * @note Compatible ABI 3 tail; check RPCR3_DESCRIPTOR_MUTE_MIN_SIZE first.
   */
  enum rpcr3_result (*ring_producer_push_tagged)(
      struct rpcr3_ring *ring, const float *input, uint64_t samples,
      uint64_t generation, uint64_t source_start,
      const struct rpcr3_mute_span *spans, uint32_t span_count,
      uint64_t *accepted);
  /**
   * @brief Add intervals for PCM already accepted, including retained output
   * caches.
   * @param ring Ring with the same sole producer as PCM/gap publication.
   * @param generation Must match the current source generation.
   * @param spans Valid intervals within the accepted source timeline; lookback
   * is bounded to capacity before the producer's next source position.
   * @param span_count At most RPCR3_MAXIMUM_MUTE_SPANS.
   * @param accepted Receives the interval-prefix count accepted without
   * waiting.
   * @return RPCR3_OK, including zero metadata credit; otherwise
   * RPCR3_INVALID_ARGUMENT. Retry the remaining intervals without advancing
   * PCM. Reset discards old tags; stale generation updates are rejected. All
   * calls are allocation-free and lock-free.
   * @note Compatible ABI 3 tail; check RPCR3_DESCRIPTOR_MUTE_MIN_SIZE first.
   */
  enum rpcr3_result (*ring_producer_mute)(struct rpcr3_ring *ring,
                                          uint64_t generation,
                                          const struct rpcr3_mute_span *spans,
                                          uint32_t span_count,
                                          uint32_t *accepted);
  /**
   * @brief End the accepted source prefix without discarding its retained PCM.
   * @param ring Ring with exactly one active producer.
   * @param token Receives a nonzero, nonwrapping boundary identity on success.
   * @return RPCR3_OK, or RPCR3_INVALID_ARGUMENT if a boundary is outstanding,
   * token space is exhausted, or an argument is invalid. Failure changes no
   * output or source state.
   *
   * Stop PCM/gap publication until ring_finish_status reports completion.
   * Interval-only mute updates remain valid. The consumer may prime a short
   * finite burst below reserve, releases real FIR and PLC lookahead using
   * non-missing zero extension, then resets the warmed converter before
   * acknowledging. Extension and post-completion silence are not packet loss.
   * No socket, EOF flush, allocation, lock, or waiting occurs. Consumer reset
   * with the producer quiescent cancels a pending boundary distinctly.
   * @note Compatible ABI 3 tail; check RPCR3_DESCRIPTOR_FINISH_MIN_SIZE first.
   */
  enum rpcr3_result (*ring_producer_finish)(struct rpcr3_ring *ring,
                                            uint64_t *token);
  /**
   * @brief Acquire the latest boundary's exact playback outcome.
   * @param ring Live ring; this operation touches only atomic state.
   * @param token Most recently returned token, not zero or an older token.
   * @param status Receives one rpcr3_finish_status value on success.
   * @return RPCR3_OK, or RPCR3_INVALID_ARGUMENT without changing status.
   * Call from the sole producer before starting its next burst. A drained
   * acknowledgement, unlike zero occupancy, includes SRC and PLC caches.
   * @note Compatible ABI 3 tail; check RPCR3_DESCRIPTOR_FINISH_MIN_SIZE first.
   */
  enum rpcr3_result (*ring_finish_status)(const struct rpcr3_ring *ring,
                                          uint64_t token, uint32_t *status);
};

/** Readable prefix required before finite-source completion operations. */
#define RPCR3_DESCRIPTOR_FINISH_MIN_SIZE                                       \
  (offsetof(struct rpcr3_descriptor, ring_finish_status) +                     \
   sizeof(((struct rpcr3_descriptor *)0)->ring_finish_status))

/** Readable prefix required before using source-tagged mute operations. */
#define RPCR3_DESCRIPTOR_MUTE_MIN_SIZE                                         \
  (offsetof(struct rpcr3_descriptor, ring_producer_mute) +                     \
   sizeof(((struct rpcr3_descriptor *)0)->ring_producer_mute))

/** Readable prefix required before calling the counted-gap descriptor tail. */
#define RPCR3_DESCRIPTOR_GAP_MIN_SIZE                                          \
  (offsetof(struct rpcr3_descriptor, ring_producer_gap) +                      \
   sizeof(((struct rpcr3_descriptor *)0)->ring_producer_gap))

/**
 * @brief Return the immutable ABI-major-3 ring descriptor.
 * @return A process-lifetime descriptor that must not be modified or freed.
 */
const struct rpcr3_descriptor *rpcr3_descriptor(void);

#ifdef __cplusplus
}
#endif

#endif
