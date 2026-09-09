# rate_adjusting_pcm_ring

Lock-free, single-producer/single-consumer PCM playout ring with persistent
libsamplerate clock recovery for real-time audio callbacks.

The producer publishes signed 16-bit mono PCM without waiting. The consumer
keeps a configured reserve, starts only after priming, and drives one
persistent libsamplerate converter with a deliberately slow occupancy-derived
ratio. This corrects independent clocks without callback-rate pitch modulation
or buffer-drop artifacts.

On a source shortfall, the consumer retains recent real PCM and uses bounded
pitch-period continuation with entry and recovery crossfades. This conceals a
brief gap without blocking or allocating in either audio operation; sustained
loss fades to silence instead of repeating speech indefinitely. Call
`rpcr_set_sample_rate()` before rendering so the concealer uses the active PCM
rate.  Call `rpcr_set_rates()` before rendering when producer and consumer
rates differ; its single persistent converter performs both nominal conversion
and clock correction.  `rpcr_set_sample_rate()` remains a shorthand for a
same-rate ring.

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

The project is licensed under GPL-2.0-only.
