# Quality requirements

Before an ordinary push, run `make lint static-analysis`. GitHub repeats only
those formatting, lint, and static-analysis checks for pushes; it does not run
Doxygen or the platform matrix.

Every pull request must pass the required full GitHub quality gate before it
may merge. Branch protection must require the `Required quality gate` check.
The gate requires:

- warnings-as-errors compilation;
- Clang formatting, Cppcheck, and Clang-Tidy without diagnostics;
- Doxygen without warnings;
- 100% line and branch coverage of production code;
- static/shared-library build, staged install, installed-consumer, and
  unpacked-source-archive tests, including a Debian runtime/development
  package split and extracted-package shared-library consumer check;
- native Debian 13 amd64 and arm64 runners, with coverage on amd64 only.

`make ci` remains available for an explicit full local verification, but is
not required before a push. Debian 12 packages and tests are manual-only while
support remains aspirational. A release assumes its main-branch revision
already passed the merge gate, so it runs artifact-specific build and packaging
validation without repeating the full quality gate.

The real-time API remains allocation-free and lock-free after initialization.
Any interface change updates public documentation, tests, install artifacts,
and generated documentation.
