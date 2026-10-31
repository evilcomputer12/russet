// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Host stand-in for RussetProbe (tests/fixtures/apps/RussetProbe) in the broker, session and batch tests: a
// `PROBE seq=<n>` heartbeat on stdout every 100 ms and a known breakpoint line: the statement after the marker
// comment in probe_step, where `frame variable probeValue` reads 42 [D 84]. Like RussetProbe's ContentView.swift,
// the marker appears exactly once, so `grep` finds it.
//
// With `--suspended` it does no work until a debugger has attached (it polls TracerPid), like an app that
// CoreDevice launched suspended: the broker's vAttach finds it before the first heartbeat, and the heartbeat
// starts once the debugger resumes it. (A self-raised SIGSTOP would be re-reported to LLDB after the first
// continue.) It allows any same-user tracer (Yama ptrace_scope=1), so an lldb-server that is not its parent can
// attach, as debugserver does on iOS.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#ifdef __linux__
#include <sys/prctl.h>
#endif

static volatile int counter = 42;

static int traced(void) {
#ifdef __linux__
  FILE *f = fopen("/proc/self/status", "r");
  if (!f) return 1;
  char line[256];
  int tracer = 0;
  while (fgets(line, sizeof line, f))
    if (sscanf(line, "TracerPid: %d", &tracer) == 1) break;
  fclose(f);
  return tracer != 0;
#else
  return 1;
#endif
}

__attribute__((noinline)) static int probe_step(int seq) {
  int probeValue = counter;  // RUSSET_PROBE_BREAK
  return probeValue + seq;
}

int main(int argc, char **argv) {
  int suspended = argc > 1 && strcmp(argv[1], "--suspended") == 0;
#ifdef __linux__
  prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0);
#endif
  while (suspended && !traced()) usleep(2000);
  for (int seq = 0;; seq++) {
    int value = probe_step(seq);
    printf("PROBE seq=%d value=%d\n", seq, value);
    fflush(stdout);
    usleep(100 * 1000);
  }
  return 0;
}
