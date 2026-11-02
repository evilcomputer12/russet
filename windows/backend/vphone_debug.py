"""Debugging an app on the virtual iPhone, for the IDE's Debug button on the virtual destination.

    vphone_debug.py prepare BUNDLE_ID SESSION_DIR [--cid N] [--local-app PATH]

Launches the app through vphoned, starts the phone's own debugserver as a launchd job, attaches it
to the app through vphoned's port tunnel, then serves two Unix sockets in SESSION_DIR until the
debug session ends:

    gdb.sock   the attached debugserver, for LLDB's `process connect` (one connection)
    dap.sock   lldb-dap, which the IDE reaches through Russet's DAP relay

and prints one line `RESULT {json}`: the pid, the app's executable and folder in the phone, both
sockets and (with --local-app) the built app LLDB reads symbols from. debugserver attaches before LLDB connects (as Russet's broker does for a real iPhone), so
LLDB finds a stopped process. The job runs under Apple's own label: iOS takes a daemon's memory
limit from the board's jetsam properties by label, and an unknown label's 6 MB ends the session
when LLDB asks for the loaded-library list; Apple's debugserver entry allows 16 MB. The Developer
Disk Image that normally owns the label is not mounted in the virtual iPhone.
"""
import base64
import glob
import json
import os
import signal
import socket
import struct
import subprocess
import sys
import threading
import time

PORT = 1339
LABEL = "com.apple.debugserver"
GUEST_PORT = 12345
PLIST_PATH = "/var/tmp/com.apple.debugserver.plist"
PLIST = f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>/usr/libexec/debugserver</string><string>127.0.0.1:{GUEST_PORT}</string></array>
<key>RunAtLoad</key><false/>
<key>KeepAlive</key><false/>
<key>POSIXSpawnType</key><string>Interactive</string>
</dict></plist>
"""
LLDB_DAP = "/usr/lib/swift/usr/bin/lldb-dap"
# A debug session LLDB never connects to ends here (debugserver detaches and the app runs on).
CONNECT_TIMEOUT = 180


def log(message):
    print(f"==> {message}", flush=True)


def vsock(cid):
    s = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    s.settimeout(90)
    s.connect((cid, PORT))
    return s


def rpc(cid, method, params):
    body = json.dumps({"method": method, "params": params, "id": 1}).encode()
    s = vsock(cid)
    s.sendall(b"POST /v1/rpc HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
              b"Content-Length: %d\r\n\r\n" % len(body) + body)
    reply = b""
    while b"\r\n\r\n" not in reply:
        chunk = s.recv(65536)
        if not chunk:
            raise SystemExit(f"{method}: vphoned closed the connection")
        reply += chunk
    head, data = reply.split(b"\r\n\r\n", 1)
    length = next((int(l.split(b":")[1]) for l in head.split(b"\r\n") if l.lower().startswith(b"content-length")), None)
    while length is not None and len(data) < length:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    answer = json.loads(data)
    if "error" in answer:
        raise SystemExit(f"{method} failed: {answer['error'].get('message', answer['error'])}")
    return answer.get("result", {})


# --- vphoned's WebSocket port tunnel (RFC 6455; the client masks its frames) ---------------------

def tunnel(cid, guest_port):
    s = vsock(cid)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET /v1/ports/{guest_port} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"
               f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = s.recv(1)
        if not chunk:
            raise OSError("vphoned closed the tunnel")
        head += chunk
    if b" 101 " not in head.split(b"\r\n", 1)[0]:
        raise OSError(head.split(b"\r\n", 1)[0].decode(errors="replace"))
    s.settimeout(None)
    return s


def frame(opcode, payload):
    mask, n = os.urandom(4), len(payload)
    head = bytes([0x80 | opcode])
    if n < 126:
        head += bytes([0x80 | n])
    elif n < 65536:
        head += bytes([0x80 | 126]) + struct.pack(">H", n)
    else:
        head += bytes([0x80 | 127]) + struct.pack(">Q", n)
    return head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))


def exact(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def message(ws, lock):
    """The next data message from the phone; pings are answered, a close ends the stream."""
    while True:
        b0, b1 = exact(ws, 2)
        opcode, n = b0 & 0x0F, b1 & 0x7F
        if n == 126:
            n = struct.unpack(">H", exact(ws, 2))[0]
        elif n == 127:
            n = struct.unpack(">Q", exact(ws, 8))[0]
        data = exact(ws, n) if n else b""
        if opcode in (0, 2):
            return data
        if opcode == 9:
            with lock:
                ws.sendall(frame(10, data))
        elif opcode == 8:
            raise EOFError


def packet(body):
    return b"$" + body + b"#%02x" % (sum(body) % 256)


def attach(ws, lock, pid):
    """vAttach in ack mode (LLDB expects ack mode and turns it off itself); answers the stop reply."""
    with lock:
        ws.sendall(frame(2, b"+" + packet(b"vAttach;%x" % pid)))
    buf = b""
    while True:
        start = buf.find(b"$")
        end = buf.find(b"#", start) if start >= 0 else -1
        if start >= 0 and end >= 0 and len(buf) >= end + 3:
            reply = buf[start + 1:end]
            break
        buf += message(ws, lock)
    with lock:
        ws.sendall(frame(2, b"+"))
    if not reply.startswith(b"T"):
        raise SystemExit(f"debugserver did not attach to the app ({reply[:40].decode(errors='replace')})")


def serve(ws, lock, gdb_path, dap):
    """One LLDB connection over the attached tunnel; then lldb-dap stops and the helper exits."""
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(gdb_path)
    os.chmod(gdb_path, 0o600)
    server.listen(1)
    server.settimeout(CONNECT_TIMEOUT)
    try:
        client, _ = server.accept()
    except socket.timeout:
        with lock:
            ws.sendall(frame(2, packet(b"D")))
        return
    finally:
        server.close()
        os.unlink(gdb_path)
    done = threading.Event()

    def phone_to_lldb():
        try:
            while True:
                client.sendall(message(ws, lock))
        except (EOFError, OSError):
            pass
        done.set()

    def lldb_to_phone():
        try:
            while True:
                data = client.recv(65536)
                if not data:
                    break
                if data.startswith(b"$QEnableCompression"):
                    # LLDB drops debugserver's compressed loaded-library list; plain replies only
                    # (LLDB is in no-ack mode by then, so the answer carries no "+").
                    client.sendall(b"$#00")
                    continue
                with lock:
                    ws.sendall(frame(2, data))
        except OSError:
            pass
        done.set()

    threading.Thread(target=phone_to_lldb, daemon=True).start()
    threading.Thread(target=lldb_to_phone, daemon=True).start()
    done.wait()
    for s in (client, ws):
        try:
            s.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
    stop(dap)


def stop(dap):
    """lldb-dap's accept mode keeps serving after a session and ignores SIGTERM; its connection to
    the IDE closes only when it exits, which ends the editor's debug session."""
    dap.terminate()
    try:
        dap.wait(5)
    except subprocess.TimeoutExpired:
        dap.kill()


def lldb_dap(dap_path):
    homes = sorted(glob.glob(os.path.expanduser("~/.local/share/uv/python/cpython-3.12*-linux-x86_64-gnu")))
    env = dict(os.environ)
    libs = ["/usr/lib/swift/usr/lib/swift/linux", "/usr/lib/swift/usr/lib/swift/host/compiler"]
    if homes:
        # Swift's LLDB embeds Python 3.12.
        env["PYTHONHOME"] = homes[-1]
        libs.insert(0, f"{homes[-1]}/lib")
    env["LD_LIBRARY_PATH"] = ":".join(libs)
    # lldb-dap's own protocol log, beside the sockets (the session folder is private to the user).
    env["LLDBDAP_LOG"] = os.path.join(os.path.dirname(dap_path), "lldb-dap.log")
    # A session of its own, like the helper's: the step's WSL session ends when the IDE's runner
    # exits and takes its processes with it. The helper ends lldb-dap when the debug session ends.
    # Russet's relay opens only same-user sockets with mode 0600: lldb-dap creates its socket under
    # a private umask.
    return subprocess.Popen([LLDB_DAP, "--connection", f"accept://{dap_path}"], env=env, start_new_session=True,
                            preexec_fn=lambda: os.umask(0o077),
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def prepare(bundle, session, cid, local_app=None):
    os.makedirs(session, mode=0o700, exist_ok=True)
    gdb_path, dap_path = os.path.join(session, "gdb.sock"), os.path.join(session, "dap.sock")
    log("starting the virtual iPhone's debugserver")
    rpc(cid, "files.write", {"path": PLIST_PATH, "content": PLIST})
    rpc(cid, "services.load", {"paths": [PLIST_PATH]})
    log(f"launching {bundle}")
    pid = int(rpc(cid, "apps.launch", {"bundle_id": bundle})["pid"])
    exe = next((p.get("executable") for p in rpc(cid, "processes.list", {})["processes"] if p.get("pid") == pid), None)
    if not exe:
        raise SystemExit(f"{bundle} is not running after its launch")
    # A fresh listener: a previous session's debugserver (if any) ends with its app detached.
    rpc(cid, "services.kickstart", {"label": LABEL, "kill": True, "force": True})
    log(f"attaching to {bundle} (pid {pid})")
    lock = threading.Lock()
    # vphoned accepts the tunnel before it reaches the guest port, and closes it when nothing listens
    # there yet: retry until the new debugserver listens.
    for attempt in range(40):
        try:
            ws = tunnel(cid, GUEST_PORT)
            attach(ws, lock, pid)
            break
        except (OSError, EOFError):
            time.sleep(0.5)
    else:
        raise SystemExit("debugserver did not start listening in the virtual iPhone")
    dap = lldb_dap(dap_path)
    for _ in range(50):
        if os.path.exists(dap_path):
            break
        time.sleep(0.1)
    if not os.path.exists(dap_path):
        dap.kill()
        raise SystemExit("lldb-dap did not start")
    os.chmod(dap_path, 0o600)
    result = {"pid": pid, "executable": exe, "appDir": os.path.dirname(exe), "gdbSocket": gdb_path, "dapSocket": dap_path}
    if local_app:
        result["localApp"] = local_app
    if os.fork():
        # The child keeps the attached tunnel; this process reports and ends the IDE's step.
        print("RESULT " + json.dumps(result), flush=True)
        os._exit(0)
    os.setsid()
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(devnull, fd)
    try:
        serve(ws, lock, gdb_path, dap)
    finally:
        stop(dap)
        os._exit(0)


def main():
    args = sys.argv[1:]
    options = {"--cid": "3", "--local-app": None}
    for name in options:
        if name in args:
            i = args.index(name)
            options[name] = args[i + 1]
            del args[i:i + 2]
    if len(args) != 3 or args[0] != "prepare":
        raise SystemExit(__doc__)
    prepare(args[1], args[2], int(options["--cid"]), options["--local-app"])


if __name__ == "__main__":
    main()
