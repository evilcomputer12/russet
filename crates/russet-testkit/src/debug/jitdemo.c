// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Host-only stand-in for the Dart VM side of the RX-page protocol (RD V8, H3), ported from
// docs/design/run-debug-prototypes/jitdemo.c. Each region is mapped r-x, announced through
// NOTIFY_DEBUGGER_ABOUT_RX_PAGES, and then checked: page 0 must start with "IHELPED!" and every other page must
// have a non-zero first or last byte (a verifying servicer writes 0x5a where the real one writes zeros).
// Regions are aligned to the 16 KiB iOS page, as Dart's are, even on a host with 4 KiB pages.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

__attribute__((noinline, visibility("default"))) void NOTIFY_DEBUGGER_ABOUT_RX_PAGES(void *base, size_t size) {
  __asm__ volatile("" ::"r"(base), "r"(size) : "memory");
}

int main(int argc, char **argv) {
  const size_t page = 16384; /* iOS arm64 page size */
  int regions = argc > 1 ? atoi(argv[1]) : 5;
  int ok = 1;
  for (int r = 0; r < regions; r++) {
    size_t npages = 1 + (size_t)r * 7; /* 1, 8, 15, ... pages */
    size_t size = npages * page;
    unsigned char *raw = mmap(NULL, size + page, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (raw == MAP_FAILED) {
      printf("region %d: mmap failed\n", r);
      return 2;
    }
    unsigned char *p = (unsigned char *)(((uintptr_t)raw + page - 1) & ~(uintptr_t)(page - 1));
    NOTIFY_DEBUGGER_ABOUT_RX_PAGES(p, size);
    int marker = memcmp(p, "IHELPED!", 8) == 0;
    size_t untouched = 0;
    for (size_t k = 1; k < npages; k++)
      if (p[k * page] == 0 && p[k * page + page - 1] == 0) untouched++;
    if (untouched) {
      printf("region %d: %zu untouched pages\n", r, untouched);
      marker = 0;
    }
    printf("region %d: %zu pages, debugger assist: %s\n", r, npages, marker ? "ok" : "not detected");
    ok &= marker;
    munmap(raw, size + page);
  }
  printf("RESULT %s\n", ok ? "PASS" : "FAIL");
  fflush(stdout);
  return ok ? 0 : 1;
}
