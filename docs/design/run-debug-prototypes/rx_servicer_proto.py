#!/usr/bin/env python3
"""Prototype of the native RX-page servicer loop (host-only, x86_64 Linux).

Talks gdb-remote to `lldb-server gdbserver ... -- jitdemo N`, sets a software
breakpoint on NOTIFY_DEBUGGER_ABOUT_RX_PAGES, and at each hit:
  1. reads (base, size) from the expedited argument registers,
  2. pipelines the page "touch" writes (IHELPED! on page 0, then one 2-byte
     write straddling each pair of page boundaries, Frida-style),
  3. emulates the function's return instead of stepping over the trap
     (x86_64: pc = [sp], sp += 8; on iOS arm64 it is simply pc = lr),
  4. resumes.
Usage: rx_servicer_proto.py PORT SYMBOL_OFFSET_HEX
"""
import re
import socket
import sys
import time

PAGE = 16384
VERIFY = True  # write 0x5a instead of 0x00 so the demo can check every page


class Gdb:
    def __init__(self, port):
        self.s = socket.create_connection(("127.0.0.1", port))
        self.buf = bytearray()
        self.noack = False
        self.sent = 0

    def _frame(self, payload: bytes) -> bytes:
        return b"$" + payload + b"#%02x" % (sum(payload) & 0xFF)

    def send(self, payload: bytes):
        self.s.sendall(self._frame(payload))
        self.sent += 1

    def send_many(self, payloads):
        # One TCP write for the whole batch: this is the pipelining under test.
        self.s.sendall(b"".join(self._frame(p) for p in payloads))
        self.sent += len(payloads)

    def recv(self) -> bytes:
        while True:
            while self.buf[:1] in (b"+", b"-"):
                del self.buf[:1]
            if b"$" in self.buf:
                s = self.buf.index(b"$")
                h = self.buf.find(b"#", s)
                if h >= 0 and len(self.buf) >= h + 3:
                    payload = bytes(self.buf[s + 1:h])
                    del self.buf[:h + 3]
                    if not self.noack:
                        self.s.sendall(b"+")
                    return payload
            chunk = self.s.recv(65536)
            if not chunk:
                raise EOFError
            self.buf += chunk

    def req(self, payload: bytes) -> bytes:
        self.send(payload)
        while True:
            r = self.recv()
            if r.startswith(b"O") and r != b"OK":  # inferior output
                sys.stdout.write(bytes.fromhex(r[1:].decode()).decode(errors="replace"))
                continue
            return r


def le64(hexstr: bytes) -> int:
    return int.from_bytes(bytes.fromhex(hexstr.decode()), "little")


def main():
    port, sym_off = int(sys.argv[1]), int(sys.argv[2], 16)
    g = Gdb(port)
    g.s.sendall(b"+")
    assert g.req(b"QStartNoAckMode") == b"OK"
    g.noack = True
    g.req(b"qSupported:swbreak+;hwbreak+")
    g.req(b"QThreadSuffixSupported")
    g.req(b"QListThreadsInStopReply")
    regs = {}
    for n in range(0, 200):
        r = g.req(b"qRegisterInfo%x" % n)
        if not r.startswith(b"name:"):
            break
        name = re.search(rb"name:([^;]+);", r).group(1).decode()
        regs[name] = n
    pid = int(re.search(rb"pid:([0-9a-f]+);", g.req(b"qProcessInfo")).group(1), 16)
    # Host-only shortcut for the image base (on iOS: jGetLoadedDynamicLibrariesInfos).
    exe = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")[0].decode()
    base = None
    for line in open(f"/proc/{pid}/maps"):
        if line.rstrip().endswith(exe.split("/")[-1]) and base is None:
            base = int(line.split("-")[0], 16)
    bp = base + sym_off
    assert g.req(b"Z0,%x,1" % bp) == b"OK"
    rip, rsp, rdi, rsi = regs["rip"], regs["rsp"], regs["rdi"], regs["rsi"]
    hits = pages_total = 0
    t_service = 0.0
    g.send(b"vCont;c")
    while True:
        r = g.recv()
        if r.startswith(b"O") and r != b"OK":
            sys.stdout.write(bytes.fromhex(r[1:].decode()).decode(errors="replace"))
            continue
        if r[:1] in (b"W", b"X"):
            print(f"[servicer] process exited: {r.decode()}")
            break
        assert r[:1] == b"T", r
        fields = dict(kv.split(b":", 1) for kv in r[3:].split(b";") if b":" in kv)
        tid = fields[b"thread"]
        pc = le64(fields[b"%02x" % rip]) if (b"%02x" % rip) in fields else le64(g.req(b"p%x;thread:%s;" % (rip, tid)))
        if pc != bp:
            # Not ours: pass signals through untouched.
            sig = r[1:3]
            g.send(b"vCont;C%s:%s" % (sig, tid) if sig not in (b"00", b"05") else b"vCont;c")
            continue
        t0 = time.perf_counter()
        hits += 1

        def reg(n):
            key = b"%02x" % n
            return le64(fields[key]) if key in fields else le64(g.req(b"p%x;thread:%s;" % (n, tid)))

        a_base, a_size, sp = reg(rdi), reg(rsi), reg(rsp)
        npages = (a_size + PAGE - 1) // PAGE
        pages_total += npages
        fill = b"5a5a" if VERIFY else b"0000"   # zeros in production (pages are fresh)
        writes = [b"M%x,8:%s" % (a_base, b"IHELPED!".hex().encode())]   # page 0
        k = 1
        while k < npages:
            if k + 1 < npages:                  # straddle boundary: end of page k + start of page k+1
                writes.append(b"M%x,2:%s" % (a_base + (k + 1) * PAGE - 1, fill))
                k += 2
            else:                               # odd tail: last byte of page k
                writes.append(b"M%x,1:%s" % (a_base + (k + 1) * PAGE - 1, fill[:2]))
                k += 1
        ret = le64(g.req(b"m%x,8" % sp))        # emulate `ret`
        writes.append(b"P%x=%s;thread:%s;" % (rip, ret.to_bytes(8, "little").hex().encode(), tid))
        writes.append(b"P%x=%s;thread:%s;" % (rsp, (sp + 8).to_bytes(8, "little").hex().encode(), tid))
        g.send_many(writes)
        for _ in writes:
            resp = g.recv()
            assert resp == b"OK", resp
        t_service += time.perf_counter() - t0
        print(f"[servicer] hit {hits}: base=0x{a_base:x} size={a_size} pages={npages} packets={len(writes)}")
        g.send(b"vCont;c")
    print(f"[servicer] hits={hits} pages={pages_total} packets_sent={g.sent} avg_service_ms={1000 * t_service / max(hits, 1):.2f}")


if __name__ == "__main__":
    main()
