# rate-adjusting-pcm-ring

`rate-adjusting-pcm-ring` is the shared lock-free single-producer,
single-consumer playout ring for the rpt_advanced project. ABI major three is a
Rust dynamic shared object with an opaque C handle and canonical mono F32 PCM
in the normalized range `-1.0` through `+1.0`.

The producer never waits or overwrites unread PCM. The consumer owns one
persistent dynamic sample-rate adapter. Block rendering waits for the requested
reserve before new-burst playout. The occupancy target controls slow clock-drift
correction. Concealment is selected at construction: disabled mode emits
silence on loss; G.711 Appendix I mode uses output-rate pitch history, adds
3.75 ms lookahead, and fades sustained loss to silence by 60 ms.

Reset discards pending PCM and concealment history even if the adapter reset
fails. Rendering stays silent after a failed reset until a later reset succeeds.

The library dynamically links `librptadv_samplerate_adapter.so.2`, the
separately versioned project adapter. It uses libswresample with
`filter_size=16`, `cutoff=1.0`, and a Kaiser filter. The adapter implements
the ring's slow clock correction with `swr_set_compensation`. There is no
quality selector or static archive.

The FIR filter has its own fixed delay, approximately 1 ms for 8 kHz input
converted to 48 kHz output. This is separate from the configured priming
reserve and optional PLC lookahead. Construction and burst reset prepare zero
filter history; startup filter response is not counted as packet loss.
The controller includes input queued inside the adapter, excluding fixed
filter delay, so adapter buffering does not consume the configured jitter
headroom. The public `available_samples` observation remains the unread FIFO
count. Empty-input calls drain available converted PCM without an end-of-stream
flush; continuous playout and recovery keep the filter state. `ring_output_delay`
reports the intrinsic output prefix for callers that manage their own finite
padding. Callers can instead use the finite-burst operations below.

`ring_producer_gap` (3.0.0-alpha.3) appends a counted missing interval in input
samples. Check `RPCR3_DESCRIPTOR_GAP_MIN_SIZE` before using this compatible
descriptor tail. Gaps occupy FIFO capacity and obey the same producer maximum,
prefix acceptance, discard accounting, and chronological retry rule as PCM.
They remain ordered between older buffered PCM and following PCM, even inside
converter input, FIR history, or a pending output cache. Reset discards them.

The consumer feeds internal zero placeholders through the same persistent
converter to preserve source time and FIR phase, then maps generated output
to the tagged source intervals. It queries live delay in billionths of a source
frame immediately before one-output conversions while gaps are pending; the
quantized source position is floored, so an exact boundary belongs to the next
span. The adapter rounds to nearest with at most half a billionth of a source
frame error. Each mapped missing output reaches the existing PLC exactly once
(or silence in disabled mode), never as real PCM. `missing_samples` includes
these outputs as well as actual converter shortfalls. No-gap rendering retains
the ordinary batched converter path. Marker storage is bounded and allocated
at construction; neither endpoint allocates or locks while streaming.

## Source muting

Check `RPCR3_DESCRIPTOR_MUTE_MIN_SIZE` and all mute callbacks before use.
`ring_producer_push_tagged` publishes PCM with a generation, its absolute
input-sample position and optional half-open mute intervals. Retry only the
unaccepted suffix. `ring_producer_mute` adds intervals for source PCM already
accepted by the ring, including samples retained in converter or PLC history.
It returns interval-prefix credit. Retain and retry any unaccepted intervals.

The consumer mutes output at its actual source position. Keep source positions
contiguous within a generation; reset the consumer before changing generations
or restarting source positions. Metadata backpressure consumes no PCM and does
not count as discarded input. Exhausted PCM capacity follows the normal
rejected-sample accounting.

## Timed sample rendering

Check `RPCR3_DESCRIPTOR_TIMED_MIN_SIZE` and the
`ring_consumer_render_sample_timed` callback before using it. The sized
`rpcr3_timed_sample_v1` result accompanies one sample from the existing
consumer, including its converter cache and PLC delay. Ordinary and timed
rendering can be interleaved on that same sole consumer. The ABI3 prefix,
configuration, SONAME and existing callbacks stay unchanged.

With `RPCR3_TIMED_POSITION_VALID`, generation plus signed magnitude identifies
the output on the original input timeline: sign times `(source_frame +
source_fraction_billionths/1e9)`. Whole frames retain the full unsigned 64-bit
domain; fractional startup can be negative. This uses the same measured
converter-position mapping as muting and does not multiply a long-lived index
into a signed 64-bit fixed-point value. Begin with tagged publication for a
fully mapped stream. Earlier untagged PCM remains unmapped after later tagging;
its previous mute behavior is preserved.

`REAL` matches the existing real-sample classification, including recovery
blends. `ZERO_EXTENSION` distinguishes startup FIR history from admitted PCM.
`GAP` identifies an ordered missing source slot; `PLC` identifies synthesis for
that slot or an unmapped shortfall, not fresh source PCM. These source slots
describe playout timing, not the provenance of the historical PLC waveform.
Intentional muting adds `MUTED` without converting real silence into loss.
Muting a gap before PLC admission replaces its PLC request with intentional
real silence: `GAP | MUTED | REAL`, without PLC. A later mute of a gap already
retained in PLC lookahead adds `MUTED` and gates its output to zero while
preserving `GAP | PLC` and the original false REAL classification.
Initial PLC delay and idle output remain unmapped. Reset and finite completion
retire metadata with the PCM/history they describe.

The ring reads no wall clock and invents no admission timestamps. Its caller
keeps bounded admission anchors keyed by generation and the actually accepted
source prefixes. Invalid pointers or short result sizes consume nothing and
leave output unchanged. Streaming and reset retain their prepared storage.

## Finite bursts

Check `RPCR3_DESCRIPTOR_FINISH_MIN_SIZE` and both finish callbacks before use.
After publishing all accepted PCM and ordered gaps, call `ring_producer_finish`
once. Keep its nonzero token and continue rendering. A finite burst can start
below reserve. The ring drains the accepted source duration, converter history
and optional PLC lookahead without counting its internal padding as loss.
Do not append caller padding when using this operation.

Query `ring_finish_status` with the exact token:

- `RPCR3_FINISH_PENDING`: keep rendering. New PCM and gaps return zero credit.
- `RPCR3_FINISH_DRAINED`: accepted audio has played and the warmed reset succeeded.
- `RPCR3_FINISH_CANCELLED`: consumer reset discarded the burst.
- `RPCR3_FINISH_ADAPTER_ERROR`: conversion or reset failed. The failed burst is
  discarded, not reported as played. If reset failed, call `ring_consumer_reset`
  successfully before resuming output.

Only the most recent token can be queried. Tokens do not wrap. Completion
permits another burst; normal reserve priming applies to its new PCM. Rendering
between completed bursts is silent and does not add shortfalls.

## Public ABI

ABI major three is the canonical-F32 interface:

- runtime library: `librate_adjusting_pcm_ring3.so.3`
- runtime package: `librate-adjusting-pcm-ring3`
- development package: `librate-adjusting-pcm-ring3-dev`
- public header:
  [`include/rate_adjusting_pcm_ring3/rate_adjusting_pcm_ring3.h`](include/rate_adjusting_pcm_ring3/rate_adjusting_pcm_ring3.h)
- pkg-config module: `rate_adjusting_pcm_ring3`

The descriptor owns lifecycle, rendering, observations, and error translation.
Consumers dynamically link the released ABI-3 package and use its opaque
handle. This resampler migration preserves the existing public descriptor
prefix and configuration layout. Consumers must convert boundary PCM to canonical F32.

## Build and verify

Install `librptadv-samplerate-adapter-dev` version `0.2.0~alpha2` or newer, then run:

```sh
make
make lint static-analysis
make test
```

`make ci` runs formatting, static analysis, Doxygen, Rust documentation,
tests, staged installation, Debian package and autopkgtest checks, archive
checks, and production-code line and branch coverage. `make container-ci`
builds the project quality image and runs the complete gate through its labeled
disposable container. The package test runs inside a separate disposable
project-owned Debian 13 testbed; the Docker socket is passed only by
`container-ci`. Doxygen generation
checks documented production Rust items and embeds complete source context;
Rustdoc publishes the symbol-level reference, including private implementation
items. `make container-coverage` starts and removes a socket-free disposable
quality container deterministically through `tools/run-in-quality-container.sh`.

For a locally staged adapter rather than an installed package, pass its paths:

```sh
make SAMPLERATE_ADAPTER_LIBDIR=/path/to/lib \
     test
```

The disposable quality image consumes one already-built, versioned runtime
adapter package and one matching development package rather than rebuilding or
vendoring its source. Give their directory explicitly:

```sh
make SAMPLERATE_ADAPTER_DEB_DIR=/path/to/adapter-debs quality-image
```

## Release order

Release `rptadv-samplerate-adapter` `v0.2.0-alpha2` first, including both
Debian 13 architecture packages. Pin that release's merged commit in
`.github/dependencies.json`; keep the same tag in
the ring quality and release workflows. Open a ring pull request and wait for
its full required quality gate, including native amd64/arm64 jobs, before
merging. Tag the validated main commit `v3.0.0-alpha3` to publish this ABI-3
extension. Confirm the GitHub release contains both architecture archives,
runtime and development Debian packages, and the source archive; then confirm
the package index was refreshed for both dependency releases. A source push
alone is not a release, and this procedure does not install anything on a node.

## Install

```sh
sudo make install PREFIX=/usr/local
sudo ldconfig
```

This installs the versioned shared object, its unversioned development linker
symlink, public header, and pkg-config metadata. The runtime requires the
matching sample-rate adapter package.

The public C ABI reference and generated Rust declaration reference are in
`build/doxygen/html/index.html`; Rustdoc supplements them in `target/doc`.
Source builds generate both trees deterministically. Release automation may
publish those trees as documentation artifacts; this source repository
deliberately contains no host-specific publication command.

The project is licensed under [GPL-2.0-only](COPYING).

## Prepared memory

FIFO capacity retains its configured jitter duration and source tags. Conversion
uses a separate 512-frame input workspace and 256-frame output quantum, so a
large FIFO does not enlarge the samplerate adapter's warmed DSP buffers. A
converter that makes no progress after a complete 512-frame prefix reports an
adapter error. Source tags retain all 64 bits of generation and position in
compact 24-byte slots. All storage is still prepared before audio; the public
ABI and FIFO admission bounds are unchanged.
