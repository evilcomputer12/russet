#!/usr/bin/env python3
"""Experiment: pre-attach broker + transparent relay (host-only, no iPhone).

1. Connect to a process-less gdb-remote stub (lldb-server gdbserver / debugserver).
2. Make the stub own a stopped process ourselves (vRun here; vAttach on iOS).
3. Listen on a local port and relay bytes verbatim between the next client
   (lldb / lldb-dap) and the stub.

Usage: preattach_relay.py STUB_PORT (LISTEN_PORT | /abs/path/to.sock) PROGRAM
"""
import socket
import sys
import threading


def checksum(payload: bytes) -> bytes:
    return b"%02x" % (sum(payload) & 0xFF)


def send_packet(sock, payload: bytes):
    sock.sendall(b"$" + payload + b"#" + checksum(payload))


def read_packet(sock, buf: bytearray) -> bytes:
    """Read one $...#xx packet in ack mode; skip '+' acks; ack the reply."""
    while True:
        while b"#" not in buf or len(buf) < buf.index(b"#") + 3:
            chunk = sock.recv(65536)
            if not chunk:
                raise EOFError
            buf += chunk
        start = buf.index(b"$")
        end = buf.index(b"#", start)
        payload = bytes(buf[start + 1:end])
        del buf[:end + 3]
        sock.sendall(b"+")
        return payload


def main():
    stub_port, program = int(sys.argv[1]), sys.argv[3]
    listen_port = sys.argv[2] if sys.argv[2].startswith("/") else int(sys.argv[2])
    stub = socket.create_connection(("127.0.0.1", stub_port))
    buf = bytearray()
    send_packet(stub, b"vRun;" + program.encode().hex().encode())
    reply = read_packet(stub, buf)
    print(f"BROKER: stub replied to vRun: {reply[:60]!r}", flush=True)
    # Drain any leftover '+' acks so the client starts on a clean stream.
    leftover = bytes(b for b in buf if b != ord("+"))
    assert not leftover, leftover

    srv = socket.socket(socket.AF_UNIX) if isinstance(listen_port, str) else socket.socket()
    pass
    srv.bind(listen_port if isinstance(listen_port, str) else ("127.0.0.1", listen_port))
    srv.listen(1)
    print("BROKER_READY", flush=True)
    client, _ = srv.accept()

    def pump(src, dst, tag):
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except OSError:
            pass
        finally:
            try:
                dst.shutdown(socket.SHUT_WR)
            except OSError:
                pass

    t1 = threading.Thread(target=pump, args=(client, stub, "c->s"), daemon=True)
    t2 = threading.Thread(target=pump, args=(stub, client, "s->c"), daemon=True)
    t1.start(); t2.start(); t1.join(); t2.join()


if __name__ == "__main__":
    main()
