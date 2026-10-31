// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

// A synthetic stand-in for libCoreADI.so (no Apple code), reached through the fake libstoreservicescore.so's
// dlopen/dlsym; its initializer proves DT_INIT_ARRAY runs.

static int ready;

__attribute__((constructor)) static void init(void) { ready = 0x21; }

int fake_core_magic(void) { return 0x41444900 | ready; }
