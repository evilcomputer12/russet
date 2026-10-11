// Host-only stand-in for the Dart VM side of the RX-page protocol.
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdlib.h>
__attribute__((noinline, visibility("default"))) void
NOTIFY_DEBUGGER_ABOUT_RX_PAGES(void *base, size_t size) {
  __asm__ volatile("" ::"r"(base), "r"(size) : "memory");
}
int main(int argc, char **argv) {
  const size_t page = 16384;               /* iOS arm64 page size */
  int regions = argc > 1 ? atoi(argv[1]) : 5;
  int ok = 1;
  for (int r = 0; r < regions; r++) {
    size_t npages = 1 + (size_t)r * 7;    /* 1, 8, 15, ... pages */
    size_t size = npages * page;
    unsigned char *p = mmap(NULL, size, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    NOTIFY_DEBUGGER_ABOUT_RX_PAGES(p, size);
    int marker = memcmp(p, "IHELPED!", 8) == 0;
    size_t untouched = 0;
    for (size_t k = 1; k < npages; k++)
      if (p[k * page] == 0 && p[k * page + page - 1] == 0) untouched++;
    if (untouched) { printf("region %d: %zu untouched pages\n", r, untouched); marker = 0; }
    printf("region %d: %zu pages, debugger assist: %s\n", r, npages, marker ? "ok" : "not detected");
    ok &= marker;
    munmap(p, size);
  }
  printf("RESULT %s\n", ok ? "PASS" : "FAIL");
  return ok ? 0 : 1;
}
