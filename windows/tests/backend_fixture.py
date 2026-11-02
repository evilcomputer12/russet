"""Transport fixture, deliberately not an application debugger acceptance test."""
import argparse
import os
from pathlib import Path
import signal
import socket
import sys
import threading
import json

parser = argparse.ArgumentParser()
parser.add_argument('directory')
parser.add_argument('--insecure', action='store_true')
parser.add_argument('--inspect-source', action='store_true')
args = parser.parse_args()
directory = Path(args.directory)
directory.mkdir(mode=0o700, parents=True, exist_ok=True)
endpoint = directory/'dap.sock'
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(str(endpoint))
os.chmod(endpoint, 0o666 if args.insecure else 0o600)
server.listen()
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
def watch_host():
    sys.stdin.buffer.read()
    os.kill(os.getpid(), signal.SIGTERM)
threading.Thread(target=watch_host, daemon=True).start()
print('ready', flush=True)
try:
    while True:
        connection, _ = server.accept()
        with connection:
            if args.inspect_source:
                stream = connection.makefile('rwb', buffering=0)
                while header := stream.readline():
                    length = int(header.split(b':')[1])
                    while stream.readline() != b'\r\n':
                        pass
                    body = bytearray()
                    while len(body) < length:
                        part = stream.read(length - len(body))
                        if not part:
                            raise EOFError('truncated fixture message')
                        body.extend(part)
                    message = json.loads(body)
                    message['backendObservedPath'] = message['arguments']['source']['path']
                    # The whole message as the backend saw it; a string, so the reply leg leaves it as is.
                    message['backendObservedMessage'] = body.decode()
                    response = json.dumps(message, ensure_ascii=False).encode()
                    connection.sendall(f'Content-Length: {len(response)}\r\n\r\n'.encode() + response)
                stream.close()
                continue
            # Probe connects and closes without sending protocol bytes.
            while data := connection.recv(65536):
                connection.sendall(data)
finally:
    server.close()
    endpoint.unlink(missing_ok=True)
