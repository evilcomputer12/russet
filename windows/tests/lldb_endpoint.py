"""Test-only same-user Unix endpoint for the recovered real LLDB DAP binary.

This launches a Linux host program, not an iPhone application. No device backend,
signing material or network listener is involved.
"""
import os
from pathlib import Path
import selectors
import signal
import socket
import subprocess
import sys
import threading

directory = Path(sys.argv[1])
directory.mkdir(parents=True, mode=0o700, exist_ok=False)
endpoint = directory/'dap.sock'
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(str(endpoint))
endpoint.chmod(0o600)
server.listen(1)
adapter = None

def stop(*_):
    raise SystemExit(0)

def watch_host():
    sys.stdin.buffer.read()
    os.kill(os.getpid(), signal.SIGTERM)

signal.signal(signal.SIGTERM, stop)
threading.Thread(target=watch_host, daemon=True).start()
print('ready', flush=True)
try:
    while True:
        connection, _ = server.accept()
        with connection:
            # Readiness probes must not launch a debugger.
            if not connection.recv(1, socket.MSG_PEEK):
                continue
            python = '/home/russet/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu'
            environment = dict(os.environ, PYTHONHOME=python,
                LD_LIBRARY_PATH=f'{python}/lib:/usr/lib/swift/usr/lib/swift/linux:/usr/lib/swift/usr/lib/swift/host/compiler')
            with (directory/'adapter.stderr').open('wb') as diagnostic:
                adapter = subprocess.Popen(['/usr/lib/swift/usr/bin/lldb-dap'], env=environment,
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostic, start_new_session=True)
                with selectors.DefaultSelector() as selector:
                    selector.register(connection, selectors.EVENT_READ, adapter.stdin)
                    selector.register(adapter.stdout, selectors.EVENT_READ, connection)
                    running = True
                    while running:
                        for key, _ in selector.select():
                            data = os.read(key.fd, 65536)
                            if not data:
                                running = False
                                break
                            if isinstance(key.data, socket.socket):
                                key.data.sendall(data)
                            else:
                                key.data.write(data)
                                key.data.flush()
            break
finally:
    if adapter is not None:
        adapter.stdin.close()
        try:
            adapter.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(adapter.pid, signal.SIGKILL)
            adapter.wait(timeout=5)
    server.close()
    endpoint.unlink(missing_ok=True)
