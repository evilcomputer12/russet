// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

// A synthetic stand-in for libstoreservicescore.so (no Apple code): it exports the ADI entry points under their
// interoperability names, reaches its companion library through dlopen/dlsym as the real one does, keeps its
// provisioning in <provisioning path>/adi.pb, and checks that the helper's sandbox refuses sockets.

#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

typedef int (*core_fn)(void);
static core_fn core_magic;
static char prov_path[512];
static char android_id[17];

static void pb_path(char *out, size_t n) { snprintf(out, n, "%s/adi.pb", prov_path); }

int kq56gsgHG6(const char *dir) {
    char path[600];
    snprintf(path, sizeof path, "%slibCoreADI.so", dir);
    void *h = dlopen(path, RTLD_NOW);
    if (!h) return -1;
    core_magic = (core_fn)dlsym(h, "fake_core_magic");
    return core_magic ? 0 : -2;
}

int Sph98paBcz(const char *id, unsigned len) {
    if (len != 16) return -3;
    memcpy(android_id, id, 16);
    return 0;
}

int nf92ngaK92(const char *path) {
    strncpy(prov_path, path, sizeof prov_path - 1);
    return 0;
}

int aslgmuibau(unsigned long long dsid) {
    char p[600];
    pb_path(p, sizeof p);
    return (dsid == (unsigned long long)-2 && access(p, F_OK) == 0) ? 0 : -45061;
}

int rsegvyrt87(unsigned long long dsid, const unsigned char *spim, unsigned len, unsigned char **cpim,
               unsigned *cpim_len, unsigned *session) {
    (void)dsid;
    *cpim = malloc(len + 5);
    memcpy(*cpim, "cpim:", 5);
    memcpy(*cpim + 5, spim, len);
    *cpim_len = len + 5;
    *session = 7;
    return 0;
}

int uv5t6nhkui(unsigned session, const unsigned char *ptm, unsigned ptm_len, const unsigned char *tk,
               unsigned tk_len) {
    if (session != 7 || ptm_len == 0 || tk_len == 0 || ptm[0] != 'p' || tk[0] != 't') return -45054;
    char p[600];
    pb_path(p, sizeof p);
    int fd = open(p, O_CREAT | O_WRONLY | O_TRUNC, 0600);
    if (fd < 0) return -1;
    if (write(fd, android_id, 16) != 16) return -1;
    close(fd);
    return 0;
}

int fy34trz2st(unsigned session) { return session == 7 ? 0 : -1; }

int p435tmhbla(unsigned long long dsid) {
    (void)dsid;
    char p[600];
    pb_path(p, sizeof p);
    unlink(p);
    return 0;
}

int jk24uiwqrg(void *p) {
    free(p);
    return 0;
}

int qi864985u0(unsigned long long dsid, unsigned char **mid, unsigned *mid_len, unsigned char **otp,
               unsigned *otp_len) {
    if (aslgmuibau(dsid) != 0) return -45061;
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s >= 0) {
        close(s);
        return -99; /* the sandbox must refuse sockets */
    }
    int magic = core_magic();
    *mid = malloc(4);
    memcpy(*mid, "MID!", 4);
    *mid_len = 4;
    *otp = malloc(7);
    memcpy(*otp, "OTP", 3);
    memcpy(*otp + 3, &magic, 4);
    *otp_len = 7;
    return 0;
}
