.DEFAULT_GOAL := all

CARGO ?= cargo
CARGO_FMT ?= cargo fmt
CARGO_CLIPPY ?= cargo clippy
CARGO_LLVM_COV ?= cargo llvm-cov
AUTOPKGTEST ?= autopkgtest
CC ?= cc
CFLAGS ?= -O2 -g
CLANG_FORMAT ?= clang-format
CLANG_TIDY ?= clang-tidy
CPPCHECK ?= cppcheck
DOXYGEN ?= doxygen
GCOVR ?= gcovr
PKG_CONFIG ?= pkg-config
READELF ?= readelf
SHELLCHECK ?= shellcheck
PYTHON ?= python3

# Older Cppcheck releases do not implement --check-level. Use exhaustive
# analysis when available so the strict error exit does not mask a truncated
# branch analysis as a successful project check.
CPPHECK_EXHAUSTIVE := $(shell $(CPPCHECK) --help 2>&1 | grep -q -- '--check-level' && printf '%s' '--check-level=exhaustive')

PACKAGE := rate-adjusting-pcm-ring
CRATE := rate_adjusting_pcm_ring2
PACKAGE_VERSION ?= 2.0.0-alpha.1
SOVERSION := 2
PREFIX ?= /usr/local
DESTDIR ?=
LIBDIR ?= $(PREFIX)/lib
CARGO_TARGET_DIR ?= target
TARGET_RELEASE := $(CARGO_TARGET_DIR)/release
LIBRARY_BASENAME := lib$(CRATE)
TARGET_LIBRARY := $(TARGET_RELEASE)/$(LIBRARY_BASENAME).so
LIBRARY_VERSIONED := build/$(LIBRARY_BASENAME).so.$(SOVERSION).$(PACKAGE_VERSION)
LIBRARY_SONAME := build/$(LIBRARY_BASENAME).so.$(SOVERSION)
LIBRARY_LINK := build/$(LIBRARY_BASENAME).so
HEADER := include/rate_adjusting_pcm_ring2/rate_adjusting_pcm_ring2.h
RUST_SOURCES := $(wildcard src/*.rs)
RUST_PRODUCTION_SOURCES := $(filter-out src/tests.rs,$(RUST_SOURCES))
RUST_DOXYGEN_INPUT := build/doxygen-input
RUST_DOXYGEN_GENERATOR := tools/rust_to_doxygen.py
PC_TEMPLATE := rate_adjusting_pcm_ring2.pc.in
PC_FILE := build/rate_adjusting_pcm_ring2.pc
C_SMOKE_SOURCE := tests/descriptor_smoke.c
C_SMOKE_BINARY := build/descriptor-smoke
V1_LIBRARY_BASENAME := librate_adjusting_pcm_ring
# ABI-major 1 retains its released C ABI and SONAME. Its versioned filename
# must therefore continue with `.so.1` so Debian's ABI-major-one package owns
# the real shared object as well as its `.so.1` linker symlink.
V1_LIBRARY_VERSION := 1.0.2
V1_SOVERSION := 1
V1_LIBRARY_VERSIONED := build/$(V1_LIBRARY_BASENAME).so.$(V1_LIBRARY_VERSION)
V1_LIBRARY_SONAME := build/$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)
V1_LIBRARY_LINK := build/$(V1_LIBRARY_BASENAME).so
V1_SOURCE := src/rate_adjusting_pcm_ring.c
V1_HEADER := include/rate_adjusting_pcm_ring.h
V1_TEST_SOURCE := tests/test_ring.c
V1_CONSUMER_SOURCE := tests/test_consumer.c
V1_OBJECT := build/rate_adjusting_pcm_ring_abi1.o
V1_TEST_BINARY := build/test-ring-abi1
V1_PC_TEMPLATE := rate_adjusting_pcm_ring.pc.in
V1_PC_FILE := build/rate_adjusting_pcm_ring.pc
DEBIAN_VERSION = $(shell dpkg-parsechangelog -S Version)
DEBIAN_ARCH = $(shell dpkg-architecture -qDEB_HOST_ARCH)
DEBIAN_MULTIARCH = $(shell dpkg-architecture -qDEB_HOST_MULTIARCH)
DEBIAN_SOURCE_PARENT = build/debian-source
DEBIAN_OUTPUT_DIR = $(abspath $(DEBIAN_SOURCE_PARENT))
DEBIAN_RUNTIME_DEB = $(DEBIAN_OUTPUT_DIR)/librate-adjusting-pcm-ring2_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_DEV_DEB = $(DEBIAN_OUTPUT_DIR)/librate-adjusting-pcm-ring2-dev_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_V1_RUNTIME_DEB = $(DEBIAN_OUTPUT_DIR)/librate-adjusting-pcm-ring1_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_V1_DEV_DEB = $(DEBIAN_OUTPUT_DIR)/librate-adjusting-pcm-ring-dev_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_STAGE = build/debian-package-stage
AUTOPKGTEST_DIR = build/autopkgtest
# Use the project-owned Debian 13 quality base as an isolated package-test
# testbed.  The derived quality image remains the build environment.
AUTOPKGTEST_TESTBED_IMAGE ?= $(QUALITY_BASE_IMAGE)
AUTOPKGTEST_PROJECT_LABEL = org.rptadvanced.test.project=$(PACKAGE)
AUTOPKGTEST_SCOPE_LABEL = org.rptadvanced.test.scope=autopkgtest
COVERAGE_TOOLCHAIN ?= nightly-2025-02-20
COVERAGE_DIR = build/coverage
COVERAGE_TARGET_DIR = build/llvm-cov-target
COVERAGE_JSON = $(COVERAGE_DIR)/coverage.json
COVERAGE_PRODUCTION_ROOT = $(CURDIR)/src
V1_COVERAGE_DIR = build/compat-coverage
QUALITY_BASE_IMAGE ?= ghcr.io/cpeter1207/rpt-advanced-quality-debian13:latest
QUALITY_IMAGE ?= $(PACKAGE)-quality:local
QUALITY_LAUNCHER = tools/run-in-quality-container.sh
SAMPLERATE_ADAPTER_DEB_DIR ?=

# The published adapter normally lives in the system multiarch library path.
# A local staged adapter can override this path for deterministic testing.
SAMPLERATE_ADAPTER_LIBDIR ?= $(shell $(PKG_CONFIG) --variable=libdir rptadv_samplerate_adapter 2>/dev/null || printf '%s' /usr/lib)
export SAMPLERATE_ADAPTER_LIBDIR

SONAME_RUSTFLAGS = $(RUSTFLAGS) -L native=$(SAMPLERATE_ADAPTER_LIBDIR) -C link-arg=-Wl,-soname,$(LIBRARY_BASENAME).so.$(SOVERSION)
C_WARNINGS = -std=c11 -Wall -Wextra -Wpedantic -Werror


.PHONY: all compat-test compat-coverage quality lint static-analysis docs test coverage install adapter-debs install-check \
	debian-package-check autopkgtest dist distcheck platform-verify ci quality-image container-ci container-coverage clean FORCE

all: $(V1_LIBRARY_VERSIONED) $(V1_LIBRARY_SONAME) $(V1_LIBRARY_LINK) \
	$(LIBRARY_VERSIONED) $(LIBRARY_SONAME) $(LIBRARY_LINK)

build:
	mkdir -p $@

$(TARGET_LIBRARY): Cargo.toml Cargo.lock $(RUST_SOURCES)
	RUSTFLAGS="$(SONAME_RUSTFLAGS)" $(CARGO) build --release --locked

$(LIBRARY_VERSIONED): $(TARGET_LIBRARY) | build
	cp $(TARGET_LIBRARY) $@

$(LIBRARY_SONAME): $(LIBRARY_VERSIONED)
	ln -sf $(notdir $<) $@

$(LIBRARY_LINK): $(LIBRARY_SONAME)
	ln -sf $(notdir $<) $@

$(V1_OBJECT): $(V1_SOURCE) $(V1_HEADER) | build
	$(CC) $(CFLAGS) $(C_WARNINGS) -Iinclude -fPIC -c $< -o $@

$(V1_LIBRARY_VERSIONED): $(V1_OBJECT) $(LIBRARY_LINK) | build
	$(CC) -shared -Wl,-soname,$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION) \
		-Wl,-rpath-link,$(CURDIR)/build -L$(CURDIR)/build -o $@ $< \
		-l$(CRATE)

$(V1_LIBRARY_SONAME): $(V1_LIBRARY_VERSIONED)
	ln -sf $(notdir $<) $@

$(V1_LIBRARY_LINK): $(V1_LIBRARY_SONAME)
	ln -sf $(notdir $<) $@

$(PC_FILE): $(PC_TEMPLATE) FORCE | build
	sed -e 's|@PREFIX@|$(PREFIX)|' -e 's|@LIBDIR@|$(LIBDIR)|' \
		-e 's|@VERSION@|$(PACKAGE_VERSION)|' $< > $@

$(V1_PC_FILE): $(V1_PC_TEMPLATE) FORCE | build
	sed -e 's|@PREFIX@|$(PREFIX)|' -e 's|@LIBDIR@|$(LIBDIR)|' \
		-e 's|@VERSION@|$(V1_LIBRARY_VERSION)|' $< > $@

quality: lint static-analysis docs

lint:
	$(CARGO_FMT) --check
	$(CLANG_FORMAT) --dry-run --Werror $(V1_SOURCE) $(V1_HEADER) $(V1_TEST_SOURCE) \
		$(V1_CONSUMER_SOURCE) $(HEADER) $(C_SMOKE_SOURCE)
	$(SHELLCHECK) tools/run-in-quality-container.sh debian/tests/install-check

static-analysis:
	RUSTFLAGS="-L native=$(SAMPLERATE_ADAPTER_LIBDIR)" $(CARGO_CLIPPY) --all-targets --all-features -- -D warnings
	$(PYTHON) -m py_compile $(RUST_DOXYGEN_GENERATOR)
	$(CPPCHECK) $(CPPHECK_EXHAUSTIVE) --force --enable=warning,style,performance,portability \
		--error-exitcode=1 --std=c11 -Iinclude $(V1_SOURCE) $(V1_TEST_SOURCE) \
		$(V1_CONSUMER_SOURCE) $(C_SMOKE_SOURCE)
	$(CLANG_TIDY) $(V1_SOURCE) $(C_SMOKE_SOURCE) --warnings-as-errors='*' -- $(C_WARNINGS) -Iinclude

docs: | build
	rm -rf $(RUST_DOXYGEN_INPUT)
	$(PYTHON) $(RUST_DOXYGEN_GENERATOR) $(RUST_DOXYGEN_INPUT) $(RUST_PRODUCTION_SOURCES)
	$(DOXYGEN) Doxyfile
	test ! -s build/doxygen-warnings.log
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --document-private-items --locked

compat-test: $(V1_TEST_BINARY)
	find build -name '*.gcda' -delete
	./$(V1_TEST_BINARY)

$(V1_TEST_BINARY): $(V1_TEST_SOURCE) $(V1_SOURCE) $(V1_HEADER) $(V1_LIBRARY_LINK) | build
	$(CC) $(C_WARNINGS) -Iinclude -O0 -g --coverage $(V1_TEST_SOURCE) $(V1_SOURCE) \
		-L$(CURDIR)/build -Wl,-rpath,'$$ORIGIN' -Wl,-rpath-link,$(CURDIR)/build \
		-Wl,--wrap=calloc,--wrap=rpcr1_bridge_descriptor \
		-l$(CRATE) -o $@

test: all compat-test
	LD_LIBRARY_PATH="$(CURDIR)/build:$(SAMPLERATE_ADAPTER_LIBDIR):$$LD_LIBRARY_PATH" \
		RUSTFLAGS="-L native=$(SAMPLERATE_ADAPTER_LIBDIR)" $(CARGO) test --all-targets --locked
	$(MAKE) $(C_SMOKE_BINARY)
	LD_LIBRARY_PATH="$(CURDIR)/build:$(SAMPLERATE_ADAPTER_LIBDIR):$$LD_LIBRARY_PATH" ./$(C_SMOKE_BINARY)

$(C_SMOKE_BINARY): $(C_SMOKE_SOURCE) $(HEADER) $(LIBRARY_LINK) | build
	$(CC) $(C_WARNINGS) -Iinclude $< -Lbuild -L$(SAMPLERATE_ADAPTER_LIBDIR) \
		-l$(CRATE) -Wl,-rpath,'$$ORIGIN' \
		-Wl,-rpath-link,$(SAMPLERATE_ADAPTER_LIBDIR) -o $@

compat-coverage: compat-test
	rm -rf $(V1_COVERAGE_DIR)
	mkdir -p $(V1_COVERAGE_DIR)
	$(GCOVR) --root . --filter '(.*/)?src/rate_adjusting_pcm_ring\.c$$' \
		--exclude 'tests/' --fail-under-line 100 --fail-under-branch 100 \
		--xml-pretty -o $(V1_COVERAGE_DIR)/coverage.xml --print-summary

coverage: compat-coverage
	rm -rf $(COVERAGE_DIR) $(COVERAGE_TARGET_DIR)
	mkdir -p $(COVERAGE_DIR)
	LD_LIBRARY_PATH="$(SAMPLERATE_ADAPTER_LIBDIR):$$LD_LIBRARY_PATH" \
		RUSTFLAGS="-L native=$(SAMPLERATE_ADAPTER_LIBDIR)" \
		RUSTUP_TOOLCHAIN=$(COVERAGE_TOOLCHAIN) CARGO_LLVM_COV_TARGET_DIR=$(abspath $(COVERAGE_TARGET_DIR)) \
		$(CARGO_LLVM_COV) --all-targets --locked --branch --json --output-path $(COVERAGE_JSON)
	$(PYTHON) -c 'import json, os, sys; report=json.load(open(sys.argv[1], encoding="utf-8")); root=os.path.realpath(sys.argv[2]); files={}; [files.setdefault(path, entry["summary"]) for datum in report.get("data", []) for entry in datum.get("files", []) for path in (os.path.realpath(entry["filename"]),) if os.path.commonpath((root, path)) == root and os.path.basename(path) != "tests.rs" and "{}tests{}".format(os.path.sep, os.path.sep) not in path]; failures=[(path, metric, summary.get(metric, {})) for path, summary in sorted(files.items()) for metric in ("lines", "branches") if not isinstance(summary.get(metric), dict) or summary[metric].get("covered") != summary[metric].get("count")]; print("verified production coverage for {} source files".format(len(files))); [print("{}: {} {}/{}".format(path, metric, values.get("covered", "missing"), values.get("count", "missing")), file=sys.stderr) for path, metric, values in failures]; raise SystemExit(1 if not files or failures else 0)' $(COVERAGE_JSON) $(COVERAGE_PRODUCTION_ROOT)

install: all $(PC_FILE) $(V1_PC_FILE)
	install -d $(DESTDIR)$(LIBDIR) \
		$(DESTDIR)$(PREFIX)/include/rate_adjusting_pcm_ring \
		$(DESTDIR)$(PREFIX)/include/rate_adjusting_pcm_ring2 \
		$(DESTDIR)$(LIBDIR)/pkgconfig
	install -m 0755 $(V1_LIBRARY_VERSIONED) $(DESTDIR)$(LIBDIR)/
	ln -sf $(notdir $(V1_LIBRARY_VERSIONED)) $(DESTDIR)$(LIBDIR)/$(notdir $(V1_LIBRARY_SONAME))
	ln -sf $(notdir $(V1_LIBRARY_SONAME)) $(DESTDIR)$(LIBDIR)/$(notdir $(V1_LIBRARY_LINK))
	install -m 0644 $(V1_HEADER) $(DESTDIR)$(PREFIX)/include/rate_adjusting_pcm_ring/
	install -m 0644 $(V1_PC_FILE) $(DESTDIR)$(LIBDIR)/pkgconfig/
	install -m 0755 $(LIBRARY_VERSIONED) $(DESTDIR)$(LIBDIR)/
	ln -sf $(notdir $(LIBRARY_VERSIONED)) $(DESTDIR)$(LIBDIR)/$(notdir $(LIBRARY_SONAME))
	ln -sf $(notdir $(LIBRARY_SONAME)) $(DESTDIR)$(LIBDIR)/$(notdir $(LIBRARY_LINK))
	install -m 0644 $(HEADER) $(DESTDIR)$(PREFIX)/include/rate_adjusting_pcm_ring2/
	install -m 0644 $(PC_FILE) $(DESTDIR)$(LIBDIR)/pkgconfig/

install-check: all
	rm -rf build/stage
	$(MAKE) PREFIX=$(CURDIR)/build/stage/usr LIBDIR=$(CURDIR)/build/stage/usr/lib install
	test -f build/stage/usr/lib/$(notdir $(V1_LIBRARY_VERSIONED))
	test -L build/stage/usr/lib/$(notdir $(V1_LIBRARY_SONAME))
	test -L build/stage/usr/lib/$(notdir $(V1_LIBRARY_LINK))
	test ! -e build/stage/usr/lib/$(V1_LIBRARY_BASENAME).a
	$(READELF) -d build/stage/usr/lib/$(notdir $(V1_LIBRARY_VERSIONED)) | \
		grep -F '$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)'
	$(READELF) -d build/stage/usr/lib/$(notdir $(V1_LIBRARY_VERSIONED)) | \
		grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	! $(READELF) -d build/stage/usr/lib/$(notdir $(V1_LIBRARY_VERSIONED)) | grep -F 'libsamplerate.so'
	test -f build/stage/usr/include/rate_adjusting_pcm_ring/$(notdir $(V1_HEADER))
	test -f build/stage/usr/lib/pkgconfig/rate_adjusting_pcm_ring.pc
	PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig \
		$(CC) $(C_WARNINGS) $(V1_CONSUMER_SOURCE) \
		$$(PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig $(PKG_CONFIG) --cflags --libs rate_adjusting_pcm_ring) \
		-Wl,-rpath,$(CURDIR)/build/stage/usr/lib -o build/stage/consumer-smoke-abi1
	LD_LIBRARY_PATH="$(CURDIR)/build/stage/usr/lib:$$LD_LIBRARY_PATH" \
		build/stage/consumer-smoke-abi1
	test -f build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED))
	test -L build/stage/usr/lib/$(notdir $(LIBRARY_SONAME))
	test -L build/stage/usr/lib/$(notdir $(LIBRARY_LINK))
	test ! -e build/stage/usr/lib/$(LIBRARY_BASENAME).a
	$(READELF) -d build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED)) | \
		grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	$(READELF) -d build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED)) | \
		grep -F 'librptadv_samplerate_adapter.so.1'
	! $(READELF) -d build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED)) | grep -F 'libsamplerate.so'
	test -f build/stage/usr/include/rate_adjusting_pcm_ring2/$(notdir $(HEADER))
	test -f build/stage/usr/lib/pkgconfig/rate_adjusting_pcm_ring2.pc
	PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig \
		$(CC) $(C_WARNINGS) $(C_SMOKE_SOURCE) \
		$$(PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig $(PKG_CONFIG) --cflags --libs rate_adjusting_pcm_ring2) \
		-Wl,-rpath,$(CURDIR)/build/stage/usr/lib -Wl,-rpath,$(SAMPLERATE_ADAPTER_LIBDIR) \
		-Wl,-rpath-link,$(SAMPLERATE_ADAPTER_LIBDIR) \
		-o build/stage/descriptor-smoke
	LD_LIBRARY_PATH="$(CURDIR)/build/stage/usr/lib:$(SAMPLERATE_ADAPTER_LIBDIR):$$LD_LIBRARY_PATH" \
		build/stage/descriptor-smoke

adapter-debs:
	test -n "$(SAMPLERATE_ADAPTER_DEB_DIR)"
	test -d "$(SAMPLERATE_ADAPTER_DEB_DIR)"
	test "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter1_*.deb' | wc -l)" -eq 1
	test "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter-dev_*.deb' | wc -l)" -eq 1

debian-package-check: dist adapter-debs
	rm -rf $(DEBIAN_SOURCE_PARENT) $(DEBIAN_STAGE)
	mkdir -p $(DEBIAN_SOURCE_PARENT)
	build_root=$$(mktemp -d); \
	trap 'rm -rf "$$build_root"' EXIT; \
	tar -C "$$build_root" -xzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz; \
	source_dir="$$build_root/$(PACKAGE)-$(PACKAGE_VERSION)"; \
	chmod 0644 "$$source_dir"/debian/changelog "$$source_dir"/debian/control \
		"$$source_dir"/debian/copyright "$$source_dir"/debian/*.docs \
		"$$source_dir"/debian/*.install "$$source_dir"/debian/source/*; \
	chmod 0755 "$$source_dir"/debian/rules; \
	cd "$$source_dir" && dpkg-buildpackage -us -uc -b; \
	cp "$$build_root"/*.deb "$(DEBIAN_OUTPUT_DIR)/"
	test -f "$(DEBIAN_RUNTIME_DEB)"
	test -f "$(DEBIAN_DEV_DEB)"
	test -f "$(DEBIAN_V1_RUNTIME_DEB)"
	test -f "$(DEBIAN_V1_DEV_DEB)"
	rm -rf $(DEBIAN_STAGE)
	mkdir -p $(DEBIAN_STAGE)
	dpkg-deb --extract "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter1_*.deb')" $(DEBIAN_STAGE)
	dpkg-deb --extract "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter-dev_*.deb')" $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_V1_RUNTIME_DEB)" $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_V1_DEV_DEB)" $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_RUNTIME_DEB)" $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_DEV_DEB)" $(DEBIAN_STAGE)
	test ! -e "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(V1_LIBRARY_BASENAME).a"
	test -f "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(notdir $(V1_LIBRARY_VERSIONED))"
	test -L "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(V1_LIBRARY_BASENAME).so"
	$(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)" | \
		grep -F '$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)'
	$(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)" | \
		grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	! $(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(V1_LIBRARY_BASENAME).so.$(V1_SOVERSION)" | grep -F 'libsamplerate.so'
	test -f "$(DEBIAN_STAGE)/usr/include/rate_adjusting_pcm_ring/$(notdir $(V1_HEADER))"
	test -f "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig/rate_adjusting_pcm_ring.pc"
	test "$$(PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig $(PKG_CONFIG) --modversion rate_adjusting_pcm_ring)" = "$(V1_LIBRARY_VERSION)"
	PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig \
		PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(CC) $(C_WARNINGS) $(V1_CONSUMER_SOURCE) \
		$$(PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(PKG_CONFIG) --cflags --libs rate_adjusting_pcm_ring) \
		-Wl,-rpath,$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH) -o $(DEBIAN_STAGE)/consumer-smoke-abi1
	LD_LIBRARY_PATH="$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH):$$LD_LIBRARY_PATH" \
		$(DEBIAN_STAGE)/consumer-smoke-abi1
	test ! -e "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).a"
	test -L "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so"
	test -f "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/librptadv_samplerate_adapter.so.1"
	$(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so.$(SOVERSION)" | \
		grep -F 'librptadv_samplerate_adapter.so.1'
	! $(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so.$(SOVERSION)" | grep -F 'libsamplerate.so'
	test -f "$(DEBIAN_STAGE)/usr/include/rate_adjusting_pcm_ring2/$(notdir $(HEADER))"
	test -f "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig/rate_adjusting_pcm_ring2.pc"
	PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig \
		PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(CC) $(C_WARNINGS) $(C_SMOKE_SOURCE) \
		$$(PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(PKG_CONFIG) --cflags --libs rate_adjusting_pcm_ring2) \
		-Wl,-rpath,$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH) \
		-Wl,-rpath-link,$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH) \
		-o $(DEBIAN_STAGE)/descriptor-smoke
	LD_LIBRARY_PATH="$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)" \
		$(DEBIAN_STAGE)/descriptor-smoke

autopkgtest: debian-package-check adapter-debs
	rm -rf $(AUTOPKGTEST_DIR)
	mkdir -p $(AUTOPKGTEST_DIR)
	set -eu; \
	cleanup() { \
		docker container ls --all --quiet --filter 'label=rpt_advanced.test=true' \
			--filter 'label=$(AUTOPKGTEST_PROJECT_LABEL)' \
			--filter 'label=$(AUTOPKGTEST_SCOPE_LABEL)' | \
			xargs -r docker container rm --force >/dev/null 2>&1 || true; \
	}; \
	cleanup; \
	trap 'status=$$?; cleanup; exit $$status' EXIT; \
	$(AUTOPKGTEST) -U --output-dir $(AUTOPKGTEST_DIR) \
		$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter1_*.deb') \
		$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter-dev_*.deb') \
		$(DEBIAN_V1_RUNTIME_DEB) $(DEBIAN_V1_DEV_DEB) \
		$(DEBIAN_RUNTIME_DEB) $(DEBIAN_DEV_DEB) . -- \
		docker --no-init $(AUTOPKGTEST_TESTBED_IMAGE) \
		--label rpt_advanced.test=true \
		--label $(AUTOPKGTEST_PROJECT_LABEL) \
		--label $(AUTOPKGTEST_SCOPE_LABEL)

dist: | build
	rm -rf build/dist
	mkdir -p build/dist/$(PACKAGE)-$(PACKAGE_VERSION)
	tar --exclude=.git --exclude=.work --exclude=build --exclude=target \
		--exclude='*/__pycache__' --exclude='*.pyc' \
		--exclude=debian/.debhelper --exclude=debian/debhelper-build-stamp \
		--exclude=debian/files --exclude=debian/tmp \
		--exclude=debian/librate-adjusting-pcm-ring1 \
		--exclude=debian/librate-adjusting-pcm-ring-dev \
		--exclude=debian/librate-adjusting-pcm-ring2 \
		--exclude=debian/librate-adjusting-pcm-ring2-dev \
		--exclude='debian/*.substvars' --exclude='debian/*.debhelper.log' \
		--transform='s|^|$(PACKAGE)-$(PACKAGE_VERSION)/|' -czf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz \
		AGENTS.md COPYING Cargo.lock Cargo.toml Doxyfile Makefile QUALITY.md README.md \
		rate_adjusting_pcm_ring.pc.in rate_adjusting_pcm_ring2.pc.in rust-toolchain.toml containers debian include src tests tools

distcheck: dist
	! tar -tzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz | \
		grep -E '/debian/(\.debhelper/|debhelper-build-stamp$$|files$$|tmp/|librate-adjusting-pcm-ring(1|-dev|2|2-dev)/|.*\.(substvars|debhelper\.log)$$)'
	rm -rf build/dist-unpacked
	mkdir -p build/dist-unpacked
	tar -C build/dist-unpacked -xzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz
	$(MAKE) -C build/dist-unpacked/$(PACKAGE)-$(PACKAGE_VERSION) \
		SAMPLERATE_ADAPTER_LIBDIR=$(SAMPLERATE_ADAPTER_LIBDIR) install-check

platform-verify: test coverage install-check autopkgtest distcheck

ci: quality platform-verify

quality-image:
	test -n "$(SAMPLERATE_ADAPTER_DEB_DIR)"
	test -d "$(SAMPLERATE_ADAPTER_DEB_DIR)"
	test "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter1_*.deb' | wc -l)" -eq 1
	test "$$(find "$(SAMPLERATE_ADAPTER_DEB_DIR)" -maxdepth 1 -type f -name 'librptadv-samplerate-adapter-dev_*.deb' | wc -l)" -eq 1
	docker image pull $(QUALITY_BASE_IMAGE)
	test "$(AUTOPKGTEST_TESTBED_IMAGE)" = "$(QUALITY_BASE_IMAGE)" || \
		docker image pull $(AUTOPKGTEST_TESTBED_IMAGE)
	base_image=$$(docker image inspect --format '{{index .RepoDigests 0}}' $(QUALITY_BASE_IMAGE)); \
	test -n "$$base_image" && test "$$base_image" != '<no value>'; \
	docker build --pull --build-context samplerate_adapter_debs=$(abspath $(SAMPLERATE_ADAPTER_DEB_DIR)) \
		--build-arg BASE_IMAGE="$$base_image" --tag $(QUALITY_IMAGE) \
		--file containers/quality.Dockerfile containers

container-coverage: quality-image
	RPTADV_CONTAINER_PULL=0 sh $(QUALITY_LAUNCHER) $(QUALITY_IMAGE) $(MAKE) coverage

# Only the complete package gate needs nested Docker for autopkgtest.  Keep
# coverage and targeted quality commands socket-free by default.
container-ci: quality-image
	RPTADV_CONTAINER_PULL=0 RPTADV_CONTAINER_DOCKER_SOCKET=1 \
		sh $(QUALITY_LAUNCHER) $(QUALITY_IMAGE) $(MAKE) ci

clean:
	rm -rf build target

FORCE:
