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
flush; continuous playout and recovery keep the filter state. Finite-media
callers use `ring_output_delay` to discard the intrinsic output prefix and
provide trailing padding before retaining exactly the source duration. This
read-only callback is appended compatibly to the ABI-3 descriptor in
3.0.0-alpha.2; callers must validate its `struct_size` before using it.

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
