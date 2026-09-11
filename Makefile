.DEFAULT_GOAL := all
CC ?= cc
AR ?= ar
CPPFLAGS += -Iinclude
CFLAGS ?= -O2 -g
WARNINGS := -std=c11 -Wall -Wextra -Wpedantic -Werror
# Debian 12's Cppcheck predates --check-level.  Analyse every supported
# configuration there, and ask newer releases for their exhaustive pass too.
CPPHECK_EXHAUSTIVE := $(shell cppcheck --help 2>&1 | grep -q -- '--check-level' && printf '%s' '--check-level=exhaustive')
SOURCE := src/rate_adjusting_pcm_ring.c
HEADER := include/rate_adjusting_pcm_ring.h
TEST := tests/test_ring.c
CONSUMER_TEST := tests/test_consumer.c
# The public ring structure gained consumer staging state in the per-sample
# API, so development artifacts deliberately carry the new ABI major.
VERSION ?= 1.0.0-dev
LIB_VERSION := $(VERSION)
LIB_SOVERSION := $(word 1,$(subst ., ,$(LIB_VERSION)))
LIB_SHARED := build/librate_adjusting_pcm_ring.so.$(LIB_VERSION)
LIB_SONAME := build/librate_adjusting_pcm_ring.so.$(LIB_SOVERSION)
prefix ?= /usr/local
DESTDIR ?=
LIBDIR ?= $(prefix)/lib
PC_TEMPLATE := rate_adjusting_pcm_ring.pc.in
PC_FILE := build/rate_adjusting_pcm_ring.pc

.PHONY: all quality lint static-analysis docs test coverage install install-check distcheck platform-verify ci clean FORCE
all: build/librate_adjusting_pcm_ring.a $(LIB_SHARED) $(LIB_SONAME) build/librate_adjusting_pcm_ring.so
build:
	mkdir -p $@
build/rate_adjusting_pcm_ring.o: $(SOURCE) $(HEADER) | build
	$(CC) $(CPPFLAGS) $(CFLAGS) $(WARNINGS) -fPIC -c $< -o $@
build/librate_adjusting_pcm_ring.a: build/rate_adjusting_pcm_ring.o
	$(AR) rcs $@ $^
$(LIB_SHARED): build/rate_adjusting_pcm_ring.o
	$(CC) -shared -Wl,-soname,librate_adjusting_pcm_ring.so.$(LIB_SOVERSION) -o $@ $^ -lsamplerate -lm
$(LIB_SONAME): $(LIB_SHARED)
	ln -sf $(notdir $<) $@
build/librate_adjusting_pcm_ring.so: $(LIB_SONAME)
	ln -sf $(notdir $<) $@
# The install prefix is a Make variable, not a file dependency.  Regenerate
# the pkg-config metadata on every invocation so staged package installs do
# not retain a prior default prefix.
$(PC_FILE): $(PC_TEMPLATE) FORCE | build
	sed -e 's|@PREFIX@|$(prefix)|' -e 's|@LIBDIR@|$(LIBDIR)|' \
		-e 's|@VERSION@|$(VERSION)|' $< > $@
build/test_ring: $(TEST) $(SOURCE) $(HEADER) | build
	$(CC) $(CPPFLAGS) $(WARNINGS) -O0 -g --coverage $(TEST) $(SOURCE) -lsamplerate -lm -Wl,--wrap=calloc,--wrap=src_new,--wrap=src_process -o $@
quality: lint static-analysis docs
lint:
	clang-format --dry-run --Werror $(SOURCE) $(HEADER) $(TEST) $(CONSUMER_TEST)
static-analysis:
	cppcheck $(CPPHECK_EXHAUSTIVE) --force --enable=warning,style,performance,portability --error-exitcode=1 --std=c11 -Iinclude $(SOURCE) $(TEST) $(CONSUMER_TEST)
	clang-tidy $(SOURCE) --warnings-as-errors='*' -- -std=c11 -Iinclude
docs: | build
	doxygen Doxyfile
	test ! -s build/doxygen-warnings.log
test: build/test_ring
	find build -name '*.gcda' -delete
	./build/test_ring
coverage: test
	mkdir -p build/coverage
	gcovr --root . --filter 'src/|include/' --fail-under-line 100 --fail-under-branch 100 --xml-pretty -o build/coverage/coverage.xml --print-summary
install: all $(PC_FILE)
	install -d $(DESTDIR)$(LIBDIR) $(DESTDIR)$(prefix)/include/rate_adjusting_pcm_ring $(DESTDIR)$(LIBDIR)/pkgconfig
	install -m 0644 build/librate_adjusting_pcm_ring.a $(DESTDIR)$(LIBDIR)/
	install -m 0755 $(LIB_SHARED) $(DESTDIR)$(LIBDIR)/
	ln -sf $(notdir $(LIB_SHARED)) $(DESTDIR)$(LIBDIR)/$(notdir $(LIB_SONAME))
	ln -sf $(notdir $(LIB_SONAME)) $(DESTDIR)$(LIBDIR)/librate_adjusting_pcm_ring.so
	install -m 0644 $(HEADER) $(DESTDIR)$(prefix)/include/rate_adjusting_pcm_ring/
	install -m 0644 $(PC_FILE) $(DESTDIR)$(LIBDIR)/pkgconfig/
install-check: all
	$(MAKE) DESTDIR=$(CURDIR)/build/stage prefix=/usr install
	cmp build/librate_adjusting_pcm_ring.a build/stage/usr/lib/librate_adjusting_pcm_ring.a
	cmp $(LIB_SHARED) build/stage/usr/lib/$(notdir $(LIB_SHARED))
	cmp $(HEADER) build/stage/usr/include/rate_adjusting_pcm_ring/rate_adjusting_pcm_ring.h
	PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/build/stage $(CC) $(WARNINGS) $(CONSUMER_TEST) \
		$$(PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/build/stage pkg-config --cflags --libs rate_adjusting_pcm_ring) \
		-Wl,-rpath,$(CURDIR)/build/stage/usr/lib -o build/test_consumer
	./build/test_consumer
distcheck: install-check
	rm -rf build/distcheck
	mkdir -p build/distcheck/rate_adjusting_pcm_ring-$(VERSION)
	cp -a Makefile Doxyfile README.md QUALITY.md AGENTS.md $(PC_TEMPLATE) include src tests build/distcheck/rate_adjusting_pcm_ring-$(VERSION)/
	tar -C build/distcheck -czf build/rate_adjusting_pcm_ring-$(VERSION).tar.gz rate_adjusting_pcm_ring-$(VERSION)
	rm -rf build/distcheck/unpacked
	mkdir -p build/distcheck/unpacked
	tar -C build/distcheck/unpacked -xzf build/rate_adjusting_pcm_ring-$(VERSION).tar.gz
	$(MAKE) -C build/distcheck/unpacked/rate_adjusting_pcm_ring-$(VERSION) install-check
platform-verify: coverage install-check distcheck
ci: quality platform-verify
clean:
	rm -rf build

FORCE:
