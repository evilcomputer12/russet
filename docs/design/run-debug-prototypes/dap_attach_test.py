#!/usr/bin/env python3
"""Experiment: drive lldb-dap over stdio with an attachCommands-only attach.

Usage: dap_attach_test.py LLDB_DAP_PATH ATTACH_PORT [stopOnEntry]
Prints every DAP message (abridged) with a millisecond timestamp.
"""
import json
import subprocess
import sys
import threading
import time

T0 = time.monotonic()


def log(msg):
    print(f"[{(time.monotonic() - T0) * 1000:7.0f} ms] {msg}", flush=True)


def main():
    dap_path, port = sys.argv[1], int(sys.argv[2])
    stop_on_entry = len(sys.argv) > 3 and sys.argv[3] == "stopOnEntry"
    proc = subprocess.Popen([dap_path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    seq = [0]
    pending = {}
    events = []
    lock = threading.Condition()

    def send(command, arguments=None):
        seq[0] += 1
        msg = {"seq": seq[0], "type": "request", "command": command}
        if arguments is not None:
            msg["arguments"] = arguments
        body = json.dumps(msg).encode()
        proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        proc.stdin.flush()
        log(f"--> {command}")
        return seq[0]

    def reader():
        f = proc.stdout
        while True:
            header = b""
            while not header.endswith(b"\r\n\r\n"):
                ch = f.read(1)
                if not ch:
                    return
                header += ch
            length = int(header.split(b"Content-Length:")[1].split(b"\r\n")[0])
            msg = json.loads(f.read(length))
            with lock:
                if msg["type"] == "response":
                    pending[msg["request_seq"]] = msg
                    body = json.dumps(msg.get("body", {}))[:300]
                    log(f"<-- response {msg['command']} success={msg['success']} {msg.get('message', '')} {body}")
                else:
                    events.append(msg)
                    body = json.dumps(msg.get("body", {}))[:200]
                    log(f"<-- event {msg.get('event')} {body}")
                lock.notify_all()

    threading.Thread(target=reader, daemon=True).start()

    def wait_response(s, timeout=30):
        deadline = time.monotonic() + timeout
        with lock:
            while s not in pending:
                if not lock.wait(timeout=max(0, deadline - time.monotonic())):
                    raise TimeoutError(f"no response to seq {s}")
            return pending[s]

    def wait_event(name, timeout=30):
        deadline = time.monotonic() + timeout
        with lock:
            while not any(e.get("event") == name for e in events):
                if not lock.wait(timeout=max(0, deadline - time.monotonic())):
                    raise TimeoutError(f"no event {name}")

    s = send("initialize", {"clientID": "russet-exp", "adapterID": "lldb-dap",
                            "linesStartAt1": True, "columnsStartAt1": True,
                            "pathFormat": "path"})
    wait_response(s)
    attach_seq = send("attach", {
        "name": "russet-exp",
        "stopOnEntry": stop_on_entry,
        "initCommands": ["settings set target.memory-module-load-level minimal"],
        "attachCommands": [f"process connect connect://127.0.0.1:{port}"],
    })
    wait_event("initialized")
    s = send("configurationDone")
    wait_response(s)
    resp = wait_response(attach_seq, timeout=30)
    log(f"attach success={resp['success']}")
    time.sleep(1.0)
    s = send("threads")
    wait_response(s)
    s = send("disconnect", {"terminateDebuggee": True})
    try:
        wait_response(s, timeout=10)
    except TimeoutError as e:
        log(str(e))
    proc.wait(timeout=10)
    log(f"lldb-dap exited rc={proc.returncode}")


if __name__ == "__main__":
    main()
