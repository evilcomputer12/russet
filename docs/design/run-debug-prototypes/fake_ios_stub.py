#!/usr/bin/env python3
"""Experiment: a fake, device-free debugserver that presents a *stopped arm64
iOS process* whose only image is a locally built Mach-O executable mapped at
its unslid address. It exists to exercise LLDB's Swift expression compiler
(the `po` question) on Linux without touching an iPhone. Nothing executes:
memory is served from the file, `vCont` just re-reports the same stop.

Usage: fake_ios_stub.py PORT MACHO_PATH PC_HEX DEVICE_PATH [LOGFILE]
"""
import json
import socket
import struct
import sys

PID = 0x4D2
TID = 0x1A2B
STACK_BASE = 0x16F000000
STACK_SIZE = 0x100000
HEAP_BASE = 0x280000000


def parse_macho(path):
    data = open(path, "rb").read()
    magic, cputype, cpusubtype, filetype, ncmds, sizeofcmds, flags, _ = struct.unpack_from("<IiiIIIII", data, 0)
    assert magic == 0xFEEDFACF, hex(magic)
    off = 32
    segments, uuid = [], None
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<II", data, off)
        if cmd == 0x19:  # LC_SEGMENT_64
            segname = data[off + 8:off + 24].rstrip(b"\0").decode()
            vmaddr, vmsize, fileoff, filesize, maxprot, initprot = struct.unpack_from("<QQQQii", data, off + 24)
            segments.append(dict(name=segname, vmaddr=vmaddr, vmsize=vmsize, fileoff=fileoff,
                                 filesize=filesize, maxprot=maxprot))
        elif cmd == 0x1B:  # LC_UUID
            u = data[off + 8:off + 24].hex().upper()
            uuid = f"{u[0:8]}-{u[8:12]}-{u[12:16]}-{u[16:20]}-{u[20:32]}"
        off += cmdsize
    header = dict(magic=magic, cputype=cputype, cpusubtype=cpusubtype, filetype=filetype, flags=flags)
    return data, header, segments, uuid


class Stub:
    def __init__(self, macho, pc, device_path, log):
        self.data, self.header, self.segments, self.uuid = parse_macho(macho)
        self.device_path = device_path
        self.log = log
        self.overlay = {}
        self.regs = [0] * 34
        self.regs[31] = STACK_BASE + STACK_SIZE - 0x1000  # sp
        self.regs[29] = self.regs[31] + 0x40               # fp
        self.regs[30] = pc + 0x100                         # lr (bogus)
        self.regs[32] = pc
        self.regs[33] = 0x60000000
        self.heap_next = HEAP_BASE
        self.heap = []  # (start, size)
        self.noack = False
        self.saved = {}

    # ---- memory -------------------------------------------------------
    def read_byte(self, addr):
        if addr in self.overlay:
            return self.overlay[addr]
        for s in self.segments:
            if s["vmaddr"] <= addr < s["vmaddr"] + s["vmsize"] and s["name"] != "__PAGEZERO":
                rel = addr - s["vmaddr"]
                if rel < s["filesize"]:
                    return self.data[s["fileoff"] + rel]
                return 0
        if STACK_BASE <= addr < STACK_BASE + STACK_SIZE:
            return 0
        for start, size in self.heap:
            if start <= addr < start + size:
                return 0
        return None

    def region(self, addr):
        regions = [(s["vmaddr"], s["vmsize"], {5: "rx", 3: "rw", 1: "r", 7: "rwx"}.get(s["maxprot"], "r"))
                   for s in self.segments if s["name"] != "__PAGEZERO"]
        regions.append((STACK_BASE, STACK_SIZE, "rw"))
        regions += [(a, n, "rwx") for a, n in self.heap]
        regions.sort()
        for start, size, perm in regions:
            if start <= addr < start + size:
                return f"start:{start:x};size:{size:x};permissions:{perm};"
        nxt = [r for r in regions if r[0] > addr]
        end = nxt[0][0] if nxt else 0xFFFFFFFFFFFFFFFF
        return f"start:{addr:x};size:{end - addr:x};"

    # ---- packets ------------------------------------------------------
    def reg_hex(self, n):
        size = 4 if n == 33 else 8
        return self.regs[n].to_bytes(size, "little").hex()

    def stop_reply(self):
        return f"T11thread:{TID:x};threads:{TID:x};thread-pcs:{self.regs[32]:x};reason:signal;"

    def images_json(self):
        segs = [dict(name=s["name"], vmaddr=s["vmaddr"], vmsize=s["vmsize"], fileoff=s["fileoff"],
                     filesize=s["filesize"], maxprot=s["maxprot"]) for s in self.segments]
        return json.dumps({"images": [{
            "load_address": self.segments[1]["vmaddr"], "mod_date": 0, "pathname": self.device_path,
            "uuid": self.uuid, "min_version_os_name": "iphoneos", "min_version_os_sdk": "17.0",
            "mach_header": self.header, "segments": segs}]}, separators=(",", ":"))

    def handle(self, p: str):
        if p == "QStartNoAckMode":
            self.noack_pending = True
            return "OK"
        if p.startswith("qSupported"):
            return "PacketSize=20000;qEcho+;QThreadSuffixSupported+;QListThreadsInStopReply+;qXfer:features:read-"
        if p in ("QThreadSuffixSupported", "QListThreadsInStopReply", "QEnableErrorStrings") or p.startswith("QSetDetachOnError") \
                or p.startswith("QSetSTDIO") or p.startswith("QSetWorkingDir") or p.startswith("QEnvironment") \
                or p.startswith("QSetDisableASLR") or p.startswith("QLaunchArch") or p.startswith("QSetMaxPacketSize"):
            return "OK"
        if p == "qHostInfo":
            return ("cputype:16777228;cpusubtype:2;addressing_bits:47;ostype:ios;watchpoint_exceptions_received:before;"
                    "vendor:apple;os_version:27.0.0;endian:little;ptrsize:8;vm-page-size:16384;")
        if p == "qProcessInfo":
            return (f"pid:{PID:x};parent-pid:1;real-uid:1f5;real-gid:1f5;effective-uid:1f5;effective-gid:1f5;"
                    f"cputype:100000c;cpusubtype:0;ptrsize:8;ostype:ios;vendor:apple;endian:little;")
        if p == "qC":
            return f"QC{TID:x}"
        if p == "?" or p.startswith("qThreadStopInfo"):
            return self.stop_reply()
        if p == "qfThreadInfo":
            return f"m{TID:x}"
        if p == "qsThreadInfo":
            return "l"
        if p.startswith("qRegisterInfo"):
            n = int(p[len("qRegisterInfo"):], 16)
            if n > 33:
                return "E45"
            names = {29: ("fp", "generic:fp;"), 30: ("lr", "generic:ra;"), 31: ("sp", "generic:sp;"),
                     32: ("pc", "generic:pc;"), 33: ("cpsr", "generic:flags;")}
            name, gen = names.get(n, (f"x{n}", f"generic:arg{n + 1};" if n < 8 else ""))
            bits = 32 if n == 33 else 64
            off = n * 8
            dwarf = n if n <= 31 else (32 if n == 32 else 33)
            alt = f"alt-name:x{n};" if n in (29, 30) else ""
            return (f"name:{name};{alt}bitsize:{bits};offset:{off};encoding:uint;format:hex;"
                    f"set:General Purpose Registers;ehframe:{dwarf};dwarf:{dwarf};{gen}")
        if p.startswith("p"):
            n = int(p[1:].split(";")[0], 16)
            return self.reg_hex(n) if n < 34 else "E45"
        if p.startswith("P"):
            n, val = p[1:].split(";")[0].split("=")
            n = int(n, 16)
            self.regs[n] = int.from_bytes(bytes.fromhex(val), "little")
            return "OK"
        if p.startswith("QSaveRegisterState"):
            self.saved[1] = list(self.regs)
            return "1"
        if p.startswith("QRestoreRegisterState"):
            self.regs = list(self.saved.get(1, self.regs))
            return "OK"
        if p.startswith("m"):
            addr, length = (int(x, 16) for x in p[1:].split(","))
            out = bytearray()
            for a in range(addr, addr + length):
                b = self.read_byte(a)
                if b is None:
                    break
                out.append(b)
            return out.hex() if out else "E08"
        if p.startswith("M"):
            head, payload = p[1:].split(":")
            addr, _ = (int(x, 16) for x in head.split(","))
            for i, b in enumerate(bytes.fromhex(payload)):
                self.overlay[addr + i] = b
            return "OK"
        if p.startswith("qMemoryRegionInfo:"):
            return self.region(int(p.split(":")[1], 16))
        if p == "qMemoryRegionInfo":
            return "OK"
        if p.startswith("_M"):
            size = int(p[2:].split(",")[0], 16)
            size = (size + 0x3FFF) & ~0x3FFF
            start = self.heap_next
            self.heap.append((start, size))
            self.heap_next += size + 0x4000
            return f"{start:x}"
        if p.startswith("_m"):
            return "OK"
        if p in ("jGetLoadedDynamicLibrariesInfos:", "jGetSharedCacheInfo:"):
            return "OK"  # support probes
        if p.startswith("jGetLoadedDynamicLibrariesInfos"):
            return self.images_json()
        if p.startswith("jGetSharedCacheInfo"):
            return json.dumps({"shared_cache_base_address": 0, "shared_cache_uuid": "00000000-0000-0000-0000-000000000000",
                               "no_shared_cache": True, "shared_cache_private_cache": False})
        if p.startswith("qWatchpointSupportInfo"):
            return "num:4;"
        if p == "vCont?":
            return "vCont;c;C;s;S"
        if p.startswith("vCont") or p in ("c", "s"):
            return self.stop_reply()  # nothing really runs
        if p.startswith("Z") or p.startswith("z"):
            return "OK"
        if p == "k":
            return "X09"
        if p.startswith("D"):
            return "OK"
        if p.startswith("qEcho"):
            return p
        return ""  # unsupported


def main():
    port, macho, pc, device_path = int(sys.argv[1]), sys.argv[2], int(sys.argv[3], 16), sys.argv[4]
    logf = open(sys.argv[5], "w") if len(sys.argv) > 5 else None
    stub = Stub(macho, pc, device_path, logf)
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(1)
    print("STUB_READY", flush=True)
    conn, _ = srv.accept()
    buf = bytearray()
    while True:
        chunk = conn.recv(65536)
        if not chunk:
            break
        buf += chunk
        while True:
            while buf[:1] in (b"+", b"-"):
                del buf[:1]
            if buf[:1] == b"\x03":
                del buf[:1]
                conn.sendall(b"$" + stub.stop_reply().encode() + b"#00")
                continue
            if b"$" not in buf:
                buf.clear()
                break
            s = buf.index(b"$")
            if b"#" not in buf[s:] or len(buf) < buf.index(b"#", s) + 3:
                break
            e = buf.index(b"#", s)
            pkt = bytes(buf[s + 1:e]).decode("latin-1")
            del buf[:e + 3]
            reply = stub.handle(pkt)
            if logf:
                logf.write(f"<- {pkt[:120]}\n-> {reply[:120]}\n")
                logf.flush()
            if not stub.noack:
                conn.sendall(b"+")
            body = reply.encode("latin-1")
            # escape '#', '$', '}' and '*' in the payload (binary-safe for JSON)
            esc = bytearray()
            for b in body:
                if b in (0x23, 0x24, 0x7D, 0x2A):
                    esc += bytes([0x7D, b ^ 0x20])
                else:
                    esc.append(b)
            conn.sendall(b"$" + bytes(esc) + b"#%02x" % (sum(esc) & 0xFF))
            if getattr(stub, "noack_pending", False):
                stub.noack = True
                stub.noack_pending = False
    if logf:
        logf.close()


if __name__ == "__main__":
    main()
