# rate_adjusting_pcm_ring

Lock-free, single-producer/single-consumer PCM playout ring with persistent
libsamplerate clock recovery for real-time audio callbacks.

The producer publishes signed 16-bit mono PCM without waiting. The consumer
uses available PCM immediately and drives one persistent libsamplerate
converter with a deliberately slow occupancy-derived ratio. The target
occupancy steers that ratio; it is not a playout-start threshold. This corrects
independent clocks without callback-rate pitch modulation or periodic
buffer-adjustment artifacts.

The producer never overwrites PCM that the consumer has not released. If the
ring fills, it publishes the leading portion that fits and drops the remaining
incoming samples; the public `discarded` counter records those drops. This
preserves strict single-producer/single-consumer ownership and chronological
playout without locks or allocation in either audio operation.

On a source shortfall, the consumer retains recent real PCM and uses bounded
pitch-period continuation with entry and recovery crossfades. This conceals a
brief gap without blocking or allocating in either audio operation; sustained
loss fades to silence instead of repeating speech indefinitely. Call
`rpcr_set_sample_rate()` before rendering so the concealer uses the active PCM
rate.  Call `rpcr_set_rates()` before rendering when producer and consumer
rates differ; its single persistent converter performs both nominal conversion
and clock correction.  `rpcr_set_sample_rate()` remains a shorthand for a
same-rate ring.

## API and compatibility

The real-time API is sample-at-a-time:

1. Initialize the ring with `rpcr_init()` and set its rates before either
   endpoint starts.
2. The sole producer calls `rpcr_producer_push_sample()` for each source PCM
   sample.
3. The sole hardware-paced consumer calls
   `rpcr_consumer_render_sample()` for each requested output sample. It returns
   `false` and supplies bounded concealment when source PCM is unavailable.

`rpcr_consumer_pop_sample()` is available for a consumer that needs raw source
PCM instead of rate-adjusted output. It and `rpcr_consumer_render_sample()`
share the same consumer cursor, so a consumer uses one or the other for a
given ring at a time.

The older block calls remain source-compatible: `rpcr_write()` loops over
`rpcr_producer_push_sample()`, and `rpcr_render()` loops over
`rpcr_consumer_render_sample()`. Their playout behavior intentionally follows
the sample API. In particular, `reserve` is retained for diagnostics and
`target` only steers slow clock recovery; neither reserve, priming, nor target
occupancy gates playout. The public `primed` member is retained only for source
compatibility and no longer controls playout.

Version 1 introduces the sample staging state in the public `struct rpcr_ring`
and therefore has a new shared-library ABI major. Programs built against 0.x
must be rebuilt and linked with `librate_adjusting_pcm_ring.so.1`; source users
of the block API can otherwise retain their existing calls.

## Build and verify

Debian build prerequisites are a C11 compiler, GNU Make, libsamplerate headers,
Clang tools, Cppcheck, Doxygen, and Gcovr. Build and run the full local gate:

```sh
make ci
```

The gate builds static and shared libraries, runs unit and installed-consumer
tests, verifies the unpacked source archive, requires zero diagnostics from
formatting, Cppcheck, Clang-Tidy, and Doxygen, and enforces 100% line and
branch coverage. GitHub runs the platform-dependent portion natively on
Debian 12 and 13 for amd64 and arm64.

## Install

```sh
make
sudo make install
```

This installs `librate_adjusting_pcm_ring` and its public header under
`/usr/local` by default. Set `prefix` or `DESTDIR` for packaging. Generated API
documentation is in `build/doxygen/html/index.html` after `make docs`.

## Debian packages

The Debian source package builds an ABI-major runtime package and its matching
development package:

- `librate-adjusting-pcm-ring1` contains the versioned shared object.
- `librate-adjusting-pcm-ring-dev` contains the public header, pkg-config
  metadata, and unversioned linker symlink.

The development package deliberately does not ship a static archive. Build the
packages on Debian with:

```sh
sudo apt install build-essential debhelper libsamplerate0-dev pkg-config
dpkg-buildpackage -us -uc -b
```

`make distcheck` verifies the unpacked source archive can build those packages,
extracts both packages into a staging root, confirms the static archive is
absent, and compiles and runs an installed shared-library consumer.

## Release versioning

The development build defaults to `1.0.1-dev`. Release automation passes the
version from its `v1.0.1` tag as `VERSION=1.0.1`; the first version component
remains the shared-library SONAME, so this packaging-only release continues to
install `librate_adjusting_pcm_ring.so.1`.

The project is licensed under GPL-2.0-only.
