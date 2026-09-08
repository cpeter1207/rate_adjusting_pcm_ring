# rate_adjusting_pcm_ring development rules

Keep `make ci` passing. Do not commit, merge, tag, or release code that fails
warnings-as-errors compilation, formatting, Cppcheck, Clang-Tidy, Doxygen,
tests, install checks, or 100% line and branch coverage. Remove dead code
rather than suppressing diagnostics.

The producer and consumer audio operations must remain allocation-free and
lock-free after `rpcr_init`. Update Doxygen, tests, README, and packaging with
every affected public interface.
