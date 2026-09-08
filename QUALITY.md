# Quality requirements

Every change must pass `make ci` locally where practical and the required
native GitHub matrix before merge or release. The gate requires:

- warnings-as-errors compilation;
- Clang formatting, Cppcheck, and Clang-Tidy without diagnostics;
- Doxygen without warnings;
- 100% line and branch coverage;
- static/shared-library build, staged install, installed-consumer, and
  unpacked-source-archive tests;
- Debian 12 and 13 on native amd64 and arm64 runners.

The real-time API remains allocation-free and lock-free after initialization.
Any interface change updates public documentation, tests, install artifacts,
and generated documentation.
