# rate_adjusting_pcm_ring development rules

## Shared rpt_advanced project baseline

This baseline applies to every production, shared-library, and workflow
repository in the rpt_advanced project. Repository-specific rules may add
constraints but must not weaken it.

Run platform-independent formatting, lint, static analysis—including
Cppcheck—and Doxygen once, concurrently where independent. Do not run Cppcheck
in each platform job. Run platform-dependent build, tests, packaging, and
staged-install checks concurrently on native Debian 13 amd64 and arm64. Require
100% line and branch coverage of production code only on Debian 13 amd64; test
code is excluded from the coverage requirement. Debian 12 support is
aspirational: do not run automated Debian 12 tests or build Debian 12 packages
as part of ordinary pushes, pull requests, or releases. Build Debian 12
packages manually only when explicitly requested. Automated releases publish
Debian 13 packages only; node installations use Debian 13 arm64 packages.
Quality checks must not rewrite source files.

Complete the full quality gate before pushing, opening or updating a pull
request, merging, tagging, or releasing. Local recovery commits may follow
affected targeted checks, but must not be represented as fully verified or used
for a push, pull request, merge, tag, or release until the full gate passes.
Treat compiler warnings as errors and fail applicable formatting, Ruff,
ShellCheck, Cppcheck, Clang-Tidy, Doxygen, tests, installation checks, and 100%
line and branch coverage of production code on Debian 13 amd64. Remove
unreachable or dead code instead of suppressing diagnostics or excluding it
from coverage.

Update concise Doxygen comments, tests, user documentation, examples, and
build, install, and package artifacts whenever an interface changes. Consumers
of a shared project library must use its released, versioned dynamic shared
object rather than vendor or statically link a duplicate implementation.
Preserve published ABI/API compatibility whenever practical; when a change is
necessary, document its compatibility, SONAME/package consequences, and
migration. Start and clean only project-owned, labeled test containers
deterministically. Never deploy to a node or alter its configuration without
explicit approval.

rate_adjusting_pcm_ring uses `make ci` as its complete local quality gate.

The producer and consumer audio operations must remain allocation-free and
lock-free after `rpcr_init`. Update Doxygen, tests, README, and packaging with
every affected public interface.
