# Selectable PCM loss concealment

Status: written design for review; implementation has not started.

## Approved intent

The shared rate-adjusting PCM ring will replace its custom concealment with
caller-selectable G.711 Appendix I concealment. Conversion remains SRC_LINEAR.
The public quality selector is removed. No node installation, release, unrelated
audio-policy changes, or wishlist work is included.

Requirements confirmed by the user on 2026-09-25:

- PLC-1: "Remove the resampling quality parameter."
- PLC-2: "Implement G.711 PLC. Make this selectable by the caller. For each
  caller, ask me if I want PLC selected."
- PLC-3: "If PLC is selected, require the reserve, target, and capacity to be
  set to values that allow for PLC and clock drift correction."
- PLC-4: "ABI changes are acceptable."

The user also confirmed the following caller choices, independent PLC history,
3.75 ms algorithmic delay, fade to silence by 60 ms, strict buffer validation,
and coordinated ABI/consumer changes:

| Caller | PLC |
| --- | --- |
| USBRadioPlus app_rpt transmit program ring | Enabled |
| USBRadioPlus advanced transmit fallback | Disabled |
| rpt_advanced incoming decoded link ring, per peer | Enabled |
| rpt_advanced local receive/squelch-delay ring | Disabled |
| rpt_advanced offline sound-file and speech conversion | Disabled |

The advanced transmit fallback is reachable in USBRadioPlus but bypassed by
current rpt_advanced direct callbacks. Keep that distinction; do not remove the
fallback or add another ring to direct callback audio.

## PCM algorithm

Use the algorithm in [ITU-T G.711 Appendix I, sections I.2 and I.3](https://www.itu.int/rec/dologin_pub.asp?id=T-REC-G.711-199909-I%21AppI%21PDF-E&lang=e&type=items).
Implement from the description in Rust; do not copy the publication's code.

Operate on normalized F32 at the existing output rate, without companding or an
additional 8 kHz conversion. Allocate 48.75 ms of output history independently
of input FIFO capacity. Delay output by 3.75 ms for onset overlap-add. Use the
specified normalized-correlation pitch search and complementary overlap ramps.
Preserve waveform phase; expand the repeated history with erasure duration.
Attenuation starts after 10 ms and reaches silence at 60 ms. Recovery overlap
depends on loss duration and is bounded at 10 ms. Advance state by samples, not
callback count, including synthesized history across successive losses.

Round duration-to-storage calculations upward. Allocate all history, delay,
pitch-search and conversion workspace before audio starts. No allocation, locks,
logging, I/O or unbounded retry occurs in producer or consumer calls.

With PLC disabled, real-time shortfalls produce zero-valued silence. They never
synthesize replacement audio, add PLC delay, or allocate PLC history. Offline
conversion continues to retain only real output and report incomplete conversion
as an error. Preserve its duration and cancellation behavior.

Reset clears conversion, priming and PLC state at a burst boundary. A shortfall
alone does not reset the ring or require re-priming; returning PCM resumes through
the recovery overlap. Startup priming silence is not a lost-sample event. Before
enough history exists, unavailable history is silence, never uninitialized data.

## Buffer contract

Move reserve and target into creation configuration alongside capacity, input
and output rates, PLC mode and declared maximum producer/consumer block sizes.
Current production callers already hold these timings constant for each ring's
lifetime. This makes block and per-sample rendering use one validated policy;
the old sample-render path must not bypass reserve priming. Changing policy
requires a newly prepared ring. Do not change node reload behavior in this work.

All FIFO counts are input-rate samples. Consumer block counts, PLC history and
PLC delay are output-rate samples. Use checked arithmetic, rejecting overflow,
unknown modes, incompatible descriptors and out-of-range counts before mutation.

For PLC-enabled rings, let:

- `Fi`, `Fo`: immutable input and output rates.
- `N`: declared maximum output block size, greater than zero.
- `P`: declared maximum producer block size, greater than zero.
- `qmin = max((Fo / Fi) * 0.999, 1 / 256)`: the existing controller's minimum
  output/input conversion ratio, including its adapter limit.
- `B = ceil(N / qmin) + 1`: one maximum callback's input budget plus the linear
  interpolator's successor sample.

Require `reserve >= B`, `target >= reserve`, and `capacity >= target + P`, in
addition to existing rate and workspace limits. The controller retains its
existing correction range and smoothing. Validate actual block sizes against
the declared maxima; oversized calls fail without partially changing state.

These are conservative admission rules, not G.711-mandated FIFO sizes or a
guarantee against arbitrary jitter. They allow a primed callback and one maximum
producer write when occupancy is at or below target. Repeated late producers,
multiple unserved bursts and excessive sustained clock error can still exhaust
the ring. Preserve chronological partial acceptance for an otherwise valid write
when the FIFO is full.

The PLC output delay emits startup silence and therefore does not require extra
source prefetch. Do not add its history or lookahead duration to the input reserve
again. Include the separate delay when documenting total playout latency.

For PLC-disabled rings retain zero reserve, zero target, and equal reserve/target
where callers use them. Check each timing against capacity, but do not impose PLC
minima. Retain the existing converter workspace minimum of 512 input samples.

Invalid library configurations fail; the library never silently enlarges them.
Consumer preparation must explicitly select valid values and report errors.
The coordinated consumer updates use these policies:

| Caller | Explicit buffer policy |
| --- | --- |
| USBRadioPlus app_rpt | `P=160`, `N=960`, 8 to 48 kHz; reserve 162 input samples, target 320, capacity 640. Only the reserve rises, from 20 to 20.25 ms, plus separate PLC delay. |
| rpt_advanced incoming peer | `P=4096`, `N=4096`; reserve is the greater of existing 60 ms and `B`; retain the 260 ms target. Capacity is the greater of existing 300 ms, 512 samples, and `target + 4096`. At 8 kHz this is reserve 685, target 2080, capacity 6176 input samples. Larger capacity does not increase target latency. |
| Advanced fallback, local RX, offline media | Keep existing timings, capacity and conversion semantics; PLC disabled. |

The declared bounds reflect supported interfaces, not assumptions that every IAX
packet is 20 ms. Verify the input-budget boundary against the released linear
adapter in targeted tests before implementation is accepted.

## ABI and consumers

Use ABI/SONAME 3 and the matching `rpcr3` public descriptor, header, pkg-config
and runtime/development package names. Remove the old ring quality enum and
argument, and select either `disabled` or `g711_appendix_i` during creation.
Remove per-render timing arguments now represented by immutable configuration.
Preserve both block and per-sample operations with common state handling.

Update USBRadioPlus and rpt_advanced bindings, descriptor checks, build discovery,
tests and package dependencies together. Old consumers must not load the new ABI
under an old name. Do not provide an ABI-2 compatibility shim or vendor the ring.
The samplerate adapter's separate public interface is outside this change; call
its existing linear implementation with a fixed internal selector.

Missing-sample counters continue to measure actual converter shortfalls, whether
PLC is enabled or disabled. Intentional priming/algorithm-delay silence must not
increment them. Preserve clear real-versus-replacement output classification
through the delay, and leave adapter errors distinguishable from normal loss.

## Verification

Create deterministic cases for loss onset, repeated loss, recovery, silence,
insufficient history, long erasure, reset and arbitrary callback partitioning.
Include 8 and 48 kHz output, 8-to-48 conversion, and the 640-input-sample case
whose old history allocation prevented pitch detection. Compare against the
published algorithm's timing and independent signal/overlap oracles; do not use
the implementation's own output as the expected result.

Exercise every buffer boundary, invalid mode/rate/count, overflow, wrong ABI,
oversized block and allocation/adapter failure. Retain cursor wrap, producer/
consumer ordering, fullness, reset and sustained clock-drift tests. Confirm no
allocation after construction and sample/block partition equivalence. Test each
consumer's approved PLC choice through the real dynamic library, including
disabled local zero-delay and offline exact-duration paths.

Use targeted tests while implementing. Before push run formatting, lint and
static analysis. The full GitHub PR gate runs Debian 13 native amd64/arm64 tests
and packaging checks, with 100% production line/branch coverage on amd64 only.
Update Rustdoc and C-header documentation before implementation commits. Update
operator documentation before a PR. No push, PR, release or node deployment is
part of this design-review step.

## Source baseline and scope

- Ring: `origin/main` at `3e94d7d`; Rust implementation and public ABI in
  `src/ring.rs`, `src/lib.rs`, and `include/rate_adjusting_pcm_ring2/`.
- USBRadioPlus: `dc9f82fa`; `rust/ring/src/lib.rs`, `rust/station/src/program.rs`,
  and native callback bounds in `rust/asterisk/src/lib.rs`.
- rpt_advanced: `origin/main` at `0389162a`; `rust/product/src/link/ring.rs`,
  `media.rs`, `worker.rs`, `host.rs`, and decoded input validation in
  `rust/asterisk/src/link/peer_io.rs`.

Do not alter filtering, jitter-buffer settings, clock-controller tuning, channel
routing, radio signaling or the current same-device configuration-reload policy.
Preserve unrelated changes in other working trees.
