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

Before a push, run formatting, lint, and static analysis only; GitHub repeats
those fast checks for every push. Do not run the full platform gate locally
solely to prepare a push. The full quality gate runs for every pull request and
must pass before that pull request can merge. Releases are built only from a
merged main revision that has already passed the full pull-request gate, so the
release workflow does not repeat it. Local recovery commits may follow affected
targeted checks, but must not be represented as fully verified until the pull
request gate passes.
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
lock-free after construction. Update Doxygen, tests, README, and packaging with
every affected public interface.
