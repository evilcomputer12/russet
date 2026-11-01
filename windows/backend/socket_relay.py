"""Private stdio -> same-user Unix socket relay, shared by RPC and DAP."""
import argparse
import os
from pathlib import Path
import selectors
import socket
import stat
import struct
import sys

def connect(path):
    target = Path(path)
    # No symlink redirection of runtime directories or socket endpoints.
    for part in [target, *target.parents]:
        info = part.lstat()
        if stat.S_ISLNK(info.st_mode):
            raise PermissionError('Symlink in backend endpoint')
        if info.st_mode & stat.S_IWOTH:
            raise PermissionError('World-writable backend endpoint parent')
    info = target.stat()
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise PermissionError('Backend socket must be same-user and mode 0600')
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        connection.settimeout(5)
        connection.connect(path)
        _, uid, _ = struct.unpack('3i', connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        if uid != os.getuid():
            raise PermissionError('Backend peer has a different uid')
        return connection
    except BaseException:
        connection.close()
        raise

def relay(connection):
    # Bound both directions independently: a slow reader cannot block traffic
    # in the other direction, and Content-Length bytes are never interpreted.
    selector = selectors.DefaultSelector()
    input_fd, output_fd = sys.stdin.fileno(), sys.stdout.fileno()
    for fd in (input_fd, output_fd):
        os.set_blocking(fd, False)
    connection.setblocking(False)
    to_socket, to_host = bytearray(), bytearray()
    socket_open = True
    limit = 256 * 1024
    try:
        while True:
            # Rebuild the small readiness set as buffer capacity changes.
            for key in list(selector.get_map().values()):
                selector.unregister(key.fileobj)
            if socket_open:
                if len(to_socket) < limit:
                    selector.register(input_fd, selectors.EVENT_READ, 'input')
                events = (selectors.EVENT_READ if len(to_host) < limit else 0) | (selectors.EVENT_WRITE if to_socket else 0)
                if events:
                    selector.register(connection, events, 'socket')
            if to_host:
                selector.register(output_fd, selectors.EVENT_WRITE, 'output')
            elif not socket_open:
                return
            for key, _ in selector.select():
                if key.data == 'input':
                    data = os.read(input_fd, min(65536, limit - len(to_socket)))
                    if not data:
                        return
                    to_socket.extend(data)
                elif key.data == 'output':
                    count = os.write(output_fd, to_host)
                    del to_host[:count]
                elif key.data == 'socket':
                    if to_socket:
                        try:
                            count = connection.send(to_socket)
                            del to_socket[:count]
                        except BlockingIOError:
                            pass
                    if len(to_host) < limit:
                        try:
                            data = connection.recv(min(65536, limit - len(to_host)))
                            if not data:
                                socket_open = False
                            to_host.extend(data)
                        except BlockingIOError:
                            pass
    finally:
        selector.close()

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--socket', required=True)
    parser.add_argument('--probe', action='store_true')
    args = parser.parse_args()
    try:
        with connect(args.socket) as connection:
            if args.probe:
                print('ready', flush=True)
            else:
                relay(connection)
    except (OSError, ValueError):
        print('backend endpoint unavailable or rejected', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
