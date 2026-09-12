# Quality checks

[AGENTS.md](AGENTS.md) defines the project baseline. Fast source checks are:

```sh
make lint static-analysis
```

The Debian 13 pull-request gate runs formatting, lint, static analysis, and
Doxygen once, then runs native amd64 and arm64 build, test, packaging, staged
install, and archive checks. Production Rust code and the frozen ABI-major-1
C compatibility source must each have 100% line and branch coverage on Debian
13 amd64; test code is excluded.

The ring's quality container is a disposable, labeled container. Its launcher
first removes only stopped stale containers with the exact project and workspace
labels, then removes its own container on every exit path. Doxygen generates
the public C ABI and Rust declaration references; the documentation generator
fails when a production Rust declaration lacks a Doxygen comment. Rustdoc
supplements that reference, and the source build leaves publication to release
automation rather than embedding a host-specific deployment policy. The
quality-image target requires a directory containing the already-built
versioned adapter Debian packages; it installs those packages into the image
and fails before building unless it finds exactly one runtime package and one
development package.
