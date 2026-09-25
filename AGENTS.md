# rate_adjusting_pcm_ring development rules

## Shared rpt_advanced project baseline

This baseline applies to every production, shared-library, and workflow
repository in the rpt_advanced project. Repository-specific rules may add
constraints but must not weaken it.

Before a push, run only the platform-independent formatting, lint, and static
analysis checks—including Cppcheck—without rewriting source files. GitHub
repeats only those fast checks for ordinary pushes. Do not run Cppcheck in each
platform job.

The full quality gate is required for a pull request to merge. It runs
platform-independent formatting, lint, static analysis, and Doxygen once,
concurrently where independent; then it runs platform-dependent build, tests,
packaging, and staged-install checks concurrently on native Debian 13 amd64
and arm64. It requires 100% line and branch coverage of production code only
on Debian 13 amd64; test code is excluded from coverage. Treat compiler
warnings as errors and fail applicable formatting, Ruff, ShellCheck, Cppcheck,
Clang-Tidy, Doxygen, tests, installation checks, and coverage. Remove
unreachable or dead code instead of suppressing diagnostics or excluding it
from coverage.

Debian 12 support is aspirational: do not run automated Debian 12 tests or
build Debian 12 packages as part of ordinary pushes, pull requests, or
releases. Build Debian 12 packages manually only when explicitly requested.
Automated releases publish Debian 13 packages only; node installations use
Debian 13 arm64 packages. The release workflow performs artifact-specific
build and packaging validation, but does not repeat the full quality gate: its
main-branch input has already passed the required pull-request gate.

Update concise Doxygen comments, tests, user documentation, examples, and
build, install, and package artifacts whenever an interface changes. Consumers
of a shared project library must use its released, versioned dynamic shared
object rather than vendor or statically link a duplicate implementation.
Preserve published ABI/API compatibility whenever practical; when a change is
necessary, document its compatibility, SONAME/package consequences, and
migration. Start and clean only project-owned, labeled test containers
deterministically. Never deploy to a node or alter its configuration without
explicit approval.

`make lint static-analysis` is the local pre-push check. `make ci` remains
available for an explicit full local verification, but GitHub's required
pull-request gate is the merge authority.

The producer and consumer audio operations must remain allocation-free and
lock-free after `rpcr_init`. Update Doxygen, tests, README, and packaging with
every affected public interface.
