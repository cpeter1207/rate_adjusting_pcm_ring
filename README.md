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

The library dynamically links `librptadv_samplerate_adapter.so.1`, the
separately versioned project adapter. It does not ship a static archive.

## Public ABI

ABI major two is the canonical-F32 interface:

- runtime library: `librate_adjusting_pcm_ring2.so.2`
- runtime package: `librate-adjusting-pcm-ring2`
- development package: `librate-adjusting-pcm-ring2-dev`
- public header:
  [`include/rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h`](include/rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h)
- pkg-config module: `rate_adjusting_pcm_ring2`

The descriptor owns lifecycle, rendering, observations, and error translation.
Consumers dynamically link the released ABI-2 package and use its opaque
handle. The former S16 ABI-major-one facade and its packages are not shipped;
consumers must convert boundary PCM to canonical F32 and use ABI major two.

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
