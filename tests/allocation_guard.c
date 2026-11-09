/** @file allocation_guard.c
 * @brief Native glibc interposition includes allocations inside the linked SRC.
 */
#include <assert.h>
#include <errno.h>
#include <stddef.h>
#include <stdlib.h>

extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void *__libc_memalign(size_t, size_t);
extern void __libc_free(void *);

static _Thread_local unsigned armed, allocations, frees;

void *malloc(size_t size) {
  allocations += armed;
  return __libc_malloc(size);
}

void *calloc(size_t count, size_t size) {
  allocations += armed;
  return __libc_calloc(count, size);
}

void *realloc(void *pointer, size_t size) {
  allocations += armed;
  return __libc_realloc(pointer, size);
}

void free(void *pointer) {
  frees += armed;
  __libc_free(pointer);
}

void *aligned_alloc(size_t alignment, size_t size) {
  allocations += armed;
  return __libc_memalign(alignment, size);
}

int posix_memalign(void **pointer, size_t alignment, size_t size) {
  allocations += armed;
  if (alignment < sizeof(void *) || (alignment & (alignment - 1)))
    return EINVAL;
  void *result = __libc_memalign(alignment, size);
  if (!result)
    return ENOMEM;
  *pointer = result;
  return 0;
}

void ring_test_realtime_begin(void) {
  allocations = frees = 0;
  armed = 1;
}

void ring_test_realtime_end(void) {
  armed = 0;
  assert(allocations == 0 && "timed render allocated after setup");
  assert(frees == 0 && "timed render freed storage after setup");
}
