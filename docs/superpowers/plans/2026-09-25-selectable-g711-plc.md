# Selectable G.711 PLC Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans for inline implementation, with independent final review. Follow test-driven development for each change. Hybrid Coder is not required for this task.

**Goal:** Replace custom ring concealment with selectable G.711 Appendix I PLC, remove the quality selector, and update existing consumers to ABI 3.

**Architecture:** One Rust PLC implementation inside the dynamically linked ring. Ring construction validates immutable timing and callback bounds. Consumers select their approved policy; they do not implement concealment themselves.

**Tech stack:** Rust, the existing samplerate adapter using SRC_LINEAR, a C descriptor/header, Cargo, Make and Debian 13 quality containers.

**Spec:** [Approved design](../specs/2026-09-25-selectable-g711-plc-design.md).

## Global constraints

- "No node installation, release, unrelated audio-policy changes, or wishlist work is included."
- "No allocation, locks, logging, I/O or unbounded retry occurs in producer or consumer calls."
- "The controller retains its existing correction range and smoothing."
- "Invalid library configurations fail; the library never silently enlarges them."
- "Do not provide an ABI-2 compatibility shim or vendor the ring."
- "Update Rustdoc and C-header documentation before implementation commits. Update operator documentation before a PR."
- Native Debian 13 amd64 and arm64 tests; production line/branch coverage on amd64 only. No automated Debian 12 work.
- Targeted checks during development; no local full platform gate solely to prepare a push. A push, PR, release or node installation needs separate authorization.
- Preserve all unrelated work. Use the clean ring feature branch and existing isolated USBRadioPlus checkout. Prepare rpt_advanced from inspected origin/main without resetting its dirty working tree.

## Review focus

1. A short FIFO at 8-to-48 kHz must not limit output-history length: test 640 input samples with full PLC history.
2. Intentional priming and algorithm-delay silence must not count as loss: test real output and counters independently.
3. One-sample rendering must obey the same reserve and delay as block rendering: compare identical streams under different partitions.
4. A producer larger than its declared bound must not partially mutate the ring; an allowed write to a full ring must retain chronological partial acceptance.
5. An old or partially upgraded consumer must fail safely, not call a mismatched descriptor: exercise actual loading and staged SONAME checks.

## Execution and evidence

Use the repository's `tools/run-in-quality-container.sh` launcher, which labels, isolates and cleans its own containers. Pull the published Debian 13 `latest` base before preparing the existing derived ring recipe; retain its resolved digest. Reuse installed released adapter packages. Do not run builds concurrently in the same mounted checkout.

For every test cycle, record the command, revision, expected failing assertion and red/green result in ignored `.work/g711-plc/progress.md`. Retain detailed logs there. Run only the affected tests during each cycle; run each affected repository's complete tests once after composition. Do not rerun completed coverage targets while working on unrelated gaps.

Inside the ring quality container, use:

```sh
RUSTFLAGS="-L native=$SAMPLERATE_ADAPTER_LIBDIR" \
LD_LIBRARY_PATH="$SAMPLERATE_ADAPTER_LIBDIR:${LD_LIBRARY_PATH:-}" \
cargo test --locked --lib TEST_FILTER
```

Replace `TEST_FILTER` with each named test/module below. A failing assertion is red evidence; a missing dependency, compiler error or malformed test is not. For a new interface, first make its test compile with only a minimal failing definition, then observe the behavioral failure before implementing the behavior.

## Task 1: G.711 sample-domain concealment

**Files:** new `src/plc.rs`; new `src/plc/tests.rs`; register the module in `src/lib.rs`. Retain the old concealment only until Task 3 replaces its caller.

**Interface:** private `Plc::new(output_rate_hz: u32) -> Result<Plc, ()>`, `Plc::process(input: Option<f32>) -> (f32, bool)`, and `Plc::reset()`. `Some` supplies converted PCM; `None` marks a missing output sample. The returned Boolean classifies emitted real-versus-replacement PCM, including the delayed sample. No public adapter or alternative implementation.

- [ ] Add `plc::tests::good_audio_has_exact_lookahead` first. The following assertions define the 48 kHz case; allocation failure is checked separately.

```rust
let mut plc = Plc::new(48_000).unwrap();
for _ in 0..180 {
    assert_eq!(plc.process(Some(0.25)), (0.0, false));
}
assert_eq!(plc.process(Some(0.25)), (0.25, true));
```

- [ ] Observe failure with a minimal compiling definition, then implement output history and delay using output-rate sizes independent of source FIFO capacity.
- [ ] Add and run failing tests for the Appendix I pitch-search/overlap behavior, loss-period expansion after 10 and 20 ms, and recovery. Use hand-derived periodic input and published timing, not an implementation-generated golden file.
- [ ] Test unity loss-envelope gain through 10 ms, 0.8 at 20 ms, 0.2 at 50 ms, and zero at 60 ms, accounting separately for output delay. Check silence and incomplete history without NaN, gain boost or uninitialized samples.
- [ ] Implement only the tested sample-driven algorithm. Run the same cases at 8 and 48 kHz; compare one-sample and mixed block partitions, and good/bad/good/bad sequences. Reset must remove prior history and delayed output.
- [ ] Add concise Rustdoc and run `cargo test --locked --lib plc::tests::`. Keep this work in the same implementation checkpoint as Task 3 so the committed library has a real PLC caller and no unused production module or warning suppression.

## Task 2: Validate immutable ring policy

**Files:** `src/ring.rs`, `src/tests.rs`.

**Interfaces:** `PlcMode::{Disabled, G711AppendixI}` with checked numeric parsing; `Settings` containing capacity, input/output rates, reserve, target and maximum producer/consumer counts. `Settings::validate() -> Result<(), CreateError>` implements the approved bounds using checked arithmetic. Add `CreateError::Invalid` alongside the existing allocation/adapter errors to represent rejected settings.

- [ ] Write table-driven tests before validation. Use literal source-sample expectations:

| Input/output Hz | Producer/consumer maximum | Reserve/target/capacity | Expected |
| --- | --- | --- | --- |
| 8000/48000 | 160/960 | 162/320/640 | PLC accepted |
| 8000/48000 | 160/960 | 161/320/640 | PLC rejected |
| 8000/48000 | 320/960 | 162/320/640 | PLC accepted |
| 8000/48000 | 320/960 | 162/320/639 | PLC rejected |
| 8000/48000 | 4096/4096 | 685/2080/6176 | PLC accepted |
| 8000/48000 | 4096/4096 | 685/2080/6175 | PLC rejected |
| 48000/48000 | 4096/4096 | 0/0/14400 | Disabled accepted |
| 48000/48000 | 4096/4096 | 7200/7200/14400 | Disabled accepted |

- [ ] Include unknown mode, zero rates, zero enabled-mode maxima, reserve/target beyond capacity, target below enabled reserve, conversion-ratio endpoints and integer overflow. Preserve valid disabled-mode reserve/target combinations, including target zero.
- [ ] Observe rejection-test failures, implement the approved formula, and rerun only policy tests. Do not add a controller, auto-sizing behavior or new user tuning options.
- [ ] Document count units and the conditional one-callback/one-producer-write guarantees. Run affected regression tests; include this work in the buildable Task 3 checkpoint.

## Task 3: Replace the old ring concealment

**Files:** `src/ring.rs`, `src/samplerate_adapter.rs`, `src/tests.rs`.

**Interface:** `Ring::create(settings: Settings, adapter: AdapterFunctions) -> Result<Ring, CreateError>`. Rendering takes no reserve/target arguments; the ring retains validated immutable settings. Per-sample and block rendering share state. The existing adapter is called with a fixed internal selector; its external ABI is unchanged.

- [ ] First add ring-level tests for disabled shortfall silence, independent history on the 640-input-sample 8-to-48 path, one-sample priming, and enabled 60 ms fade. Demonstrate failures against the old behavior before replacing it.
- [ ] Wire `Option<Plc>` into the consumer; disabled mode owns no PLC workspace. Delete the old pitch detector, equal-power crossfade, conceal/recover methods, quality enum and obsolete tests once their replacement tests pass.
- [ ] Enforce maximum block counts before mutation. Retain allowed partial writes, cursor ordering/wrap, reset, error counters and existing rate-control behavior. Zero-length blocks remain harmless.
- [ ] Verify startup and lookahead do not increment missing-sample counters; real converter shortfalls do, for either PLC mode. Do not re-prime merely because a producer ran short.
- [ ] Reuse existing allocator/adapter error fixtures only for failure injection. Use the real dynamic samplerate adapter for the source-budget boundary and non-unity conversion tests; the current copy-through fake cannot prove resampling behavior.
- [ ] Run `cargo test --locked --lib` after targeted tests. Update Rustdoc and commit only after ring tests pass.

## Task 4: ABI 3 and installation artifacts

**Files:** `src/lib.rs`, `src/tests.rs`, `tests/descriptor_smoke.c`, public header renamed to `include/rate_adjusting_pcm_ring3/rate_adjusting_pcm_ring3.h`, pkg-config template renamed to `rate_adjusting_pcm_ring3.pc.in`, `Cargo.toml`, `Cargo.lock`, `Makefile`, `Doxyfile`, and the existing `debian/` package/control/install/test files.

**Interface:** `rpcr3_descriptor()` exposes the same lifecycle and SPSC operation roles, with the new creation policy and render signatures. ABI and SONAME are 3. Mode values are disabled=0 and G.711 Appendix I=1. Remove quality; use a new package identity rather than an old-name compatibility shim.

- [ ] Add executable C/Rust boundary tests for ABI version, structure sizes, null pointers, mode/config rejection, exact returned result codes and unchanged handles/output on invalid calls.
- [ ] Update the descriptor/header together, compile and run the C consumer against the actual DSO, and verify both PLC modes. Keep all buffer pointers Rust-owned behind the C handle.
- [ ] Extend staged-install checks to require the ABI-3 SONAME/header/pkg-config artifacts and dynamic samplerate-adapter dependency; reject static archives and accidental ABI-2 exports. Run them before changing packaging so the missing artifacts are observed.
- [ ] Update Cargo/package identities and artifact checks. Build/install into temporary staging, never a node. Run `make test install-check distcheck` in the native amd64 container. Developer docs must build without warnings; check only affected docs while correcting issues.
- [ ] Commit the coherent ABI change with its tests and developer documentation.

## Task 5: USBRadioPlus caller policies

**Files:** `rust/ring/src/lib.rs`, `rust/ring/src/tests.rs`, `rust/station/src/program.rs`, existing station/driver/Asterisk ring fixtures, `src/chan_usbradioplus_shim.c`, shim fixture header/tests, `Makefile`, `debian/control`, `scripts/install-build-deps.sh`, `containers/Dockerfile`, `tools/validate_release.py`, and affected packaging tests.

**Interface:** `RingProvider::prepare` receives the ABI-3 settings and explicit PLC mode, not `ConversionQuality`. Station preparation selects enabled app_rpt policy or disabled advanced fallback policy from the existing adapter type. Do not change the direct-callback bypass.

- [ ] Add failing policy/descriptor tests: app_rpt has PLC enabled with 162/320/640 input samples and 160/960 block bounds; advanced fallback disables PLC with its existing timings. Verify ABI-2 rejection and oversized writes/renders without mutation.
- [ ] Replace the handwritten binding with ABI-3 fields/functions; remove caller quality choices and per-render timing arguments. Update fixture layouts, the C shim dependency and package checks together.
- [ ] Stage the new shared ring for testing and run:

```sh
cargo test --locked -p usbradioplus-ring -p usbradioplus-station --lib
```

- [ ] Run affected shim, artifact and packaging tests and then the existing workspace test command once. Preserve all processing-chain settings, radio behavior and direct callback flow. Update Rustdoc/C comments before committing.

## Task 6: rpt_advanced caller policies

**Files:** `rust/product/src/link/ring.rs`, `link/ring_tests.rs`, `media.rs`, `media/tests.rs`, `worker.rs` and preparation call sites, `rust/product/build.rs`, `wrapper.h`, `debian/control`, and affected `tests/test_rust_product_surface.py` checks.

**Interface:** Ring construction receives an explicit peer/local policy. Incoming peer creation selects enabled PLC with the approved rate-derived reserve/capacity and unchanged target; local receive selects disabled PLC with its captured squelch delay. Offline media construction explicitly disables PLC and uses zero timing.

- [ ] Add failing tests distinguishing peer, local and offline policy at construction. Test the negotiated 8 kHz example 685/2080/6176, native 48 kHz, local zero/150 ms delay, and source rates used by speech and file conversion.
- [ ] Update generated bindings and constructor call sites. Remove quality and render-time timing arguments. Do not change same-device reload behavior, create another media queue, or resample again outside the existing ring.
- [ ] Test missing/offline real output as an error, exact output duration, cancellation and no synthetic samples. Retain consumer/producer generation ownership tests.
- [ ] Build the existing file/speech adapter DSOs, then run the product ring and media tests using the existing Makefile test environment:

```sh
make rust-build
LIBRARY_PATH="$PWD/target/release:${LIBRARY_PATH:-}" \
LD_LIBRARY_PATH="$PWD/build:${LD_LIBRARY_PATH:-}" \
cargo test --locked -p rptadv-product --lib link::ring::tests::
LIBRARY_PATH="$PWD/target/release:${LIBRARY_PATH:-}" \
LD_LIBRARY_PATH="$PWD/build:${LD_LIBRARY_PATH:-}" \
cargo test --locked -p rptadv-product --lib media::tests::
```

- [ ] Run the existing rpt_advanced suite once after targeted checks, update Rustdoc and commit only this migration. Leave unrelated branch work untouched.

## Task 7: Composed verification and review

**Files:** new `tests/plc_integration.c` in the ring, its Makefile target, and existing consumer integration tests; ignored progress/coverage logs.

- [ ] Add a dedicated integration test using the actual ABI-3 descriptor and released samplerate adapter. Exercise all five approved policy shapes, real PCM conversion, shortfall, recovery, reset and invalid geometry. The test must consume the real shared object, not replace the ring with a fake.
- [ ] Compare sample-by-sample and variable-block playout for identical source samples and loss events. Check normalized finite output, chronological real samples, intentional delay, shortfall counters and unchanged disabled-path duration.
- [ ] Exercise sustained positive/negative clock drift and producer bursts at declared boundaries. Use deterministic simulated clocks; this verifies finite tested conditions, not immunity to arbitrary jitter.
- [ ] Run each affected complete test suite, staged-install checks and formatting/lint/static analysis. Record every failing test, including pre-existing failures. Close changed-source coverage gaps incrementally without repeatedly running the entire coverage suite.
- [ ] Obtain an independent final diff review focused on algorithm fidelity, allocation-free callback operation, count units and ABI safety. Fix findings with a failing regression test first.
- [ ] Report exact tests run, unresolved failures and modified repositories. Do not claim the full GitHub platform gate passed unless a corresponding run actually completed. Stop before push/release/deployment unless separately authorized.

## Plan review record

1. Scope: covers selector removal, selectable Appendix I PLC, strict geometry, five approved caller policies, developer docs and packaging. No Hybrid repair, node change or wishlist item.
2. Boundaries: includes independent history, startup/recovery timing, zero-delay disabled paths, actual adapter conversion, maximum blocks, overflow, cursor wrap, wrong ABI, allocation failures and composed checks.
3. Simplification: one private PLC module, existing ring/settings/error machinery and existing test containers; no new service, dependency, compatibility layer or duplicate resampler. The final integration test depends on Tasks 1 through 6.

Status: implementation plan ready for user review; no production implementation has started.
