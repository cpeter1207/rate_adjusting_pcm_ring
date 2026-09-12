# rate-adjusting-pcm-ring

`rate-adjusting-pcm-ring` is the shared lock-free single-producer,
single-consumer playout ring for the rpt_advanced project. ABI major two is a
Rust dynamic shared object with an opaque C handle and canonical mono F32 PCM
in the normalized range `-1.0` through `+1.0`.

The producer never waits or overwrites unread PCM. The consumer owns one
persistent dynamic sample-rate adapter, uses source PCM as soon as it is
available, and applies a slow occupancy-driven ratio correction for clock
drift. Reserve and target values are diagnostic and controller inputs, not
startup gates. Brief output shortfalls use bounded pitch-period continuation
with equal-power entry and recovery transitions; sustained loss fades out.

ABI major two dynamically links `librptadv_samplerate_adapter.so.1`, the
separately versioned project adapter. The frozen ABI-major-one compatibility
object is a narrow C forwarding facade over that Rust implementation; it
dynamically links ABI major two and retains its released S16 symbols, layout,
and SONAME. Neither ABI ships a static archive.

## ABI and migration

ABI major one is frozen for current S16 USBRadioPlus and rpt_advanced
consumers and is built alongside ABI major two. Its released compatibility
artifacts are:

- runtime library: `librate_adjusting_pcm_ring.so.1` (SONAME)
- runtime package: `librate-adjusting-pcm-ring1`
- development package: `librate-adjusting-pcm-ring-dev`
- public header:
  [`include/rate_adjusting_pcm_ring.h`](include/rate_adjusting_pcm_ring.h)
- pkg-config module: `rate_adjusting_pcm_ring`

ABI major two is the new canonical-F32 interface:

- runtime library: `librate_adjusting_pcm_ring2.so.2`
- runtime package: `librate-adjusting-pcm-ring2`
- development package: `librate-adjusting-pcm-ring2-dev`
- public header:
  [`include/rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h`](include/rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h)
- pkg-config module: `rate_adjusting_pcm_ring2`

The new descriptor owns lifecycle, rendering, observations, and error
translation. Consumers must migrate from the public ABI-1 structure to the
opaque ABI-2 descriptor; they must dynamically link a released v2 package.
If an ABI-1 rate reset fails in the required dynamic adapter, the facade
returns an error rather than continuing with stale converter state.
Its private `rpcr1_bridge_descriptor` is solely the implementation contract
between the two installed shared objects; it is not a supported consumer ABI.

## Build and verify

Install a compatible `librptadv-samplerate-adapter-dev` package (currently
`0.1.0~alpha1` or newer), then run:

```sh
make
make lint static-analysis
make test
```

`make ci` runs formatting, static analysis, Doxygen, Rust documentation,
tests, staged installation, Debian package and autopkgtest checks, archive
checks, and production-code line and branch coverage. Doxygen generation
checks documented production Rust items and embeds complete source context;
Rustdoc publishes the symbol-level reference, including private implementation
items. `make container-coverage` starts and removes a disposable quality container
deterministically through `tools/run-in-quality-container.sh`.

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

This installs both versioned shared objects, their unversioned development
linker symlinks, public headers, and pkg-config metadata. ABI major two
requires the matching sample-rate adapter runtime package; frozen ABI major one
requires its matching ABI-major-two runtime package.

The public C ABI reference and generated Rust declaration reference are in
`build/doxygen/html/index.html`; Rustdoc supplements them in `target/doc`.
Source builds generate both trees deterministically. Release automation may
publish those trees as documentation artifacts; this source repository
deliberately contains no host-specific publication command.

The project is licensed under [GPL-2.0-only](COPYING).
