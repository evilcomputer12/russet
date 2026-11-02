"""Synthetic proof that two legacy backends share one writable qcow2 node."""
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time

build = Path('/home/russet/src/qemu/build')
with tempfile.TemporaryDirectory(prefix='block-probe-', dir='/home/russet') as work:
    work = Path(work)
    base, overlay, qmp = work/'base.raw', work/'overlay.qcow2', work/'qmp.sock'
    base.write_bytes(bytes(1024 * 1024))
    subprocess.run([str(build/'qemu-img'), 'create', '-q', '-f', 'qcow2', '-F', 'raw', '-b', str(base), str(overlay)], check=True)
    args = [str(build/'qemu-system-aarch64'), '-machine', 'none', '-nodefaults', '-display', 'none', '-S',
        '-qmp', f'unix:{qmp},server=on,wait=off',
        '-blockdev', json.dumps({'driver':'file', 'node-name':'basefile', 'filename':str(base), 'read-only':True}),
        '-blockdev', json.dumps({'driver':'raw', 'node-name':'base', 'file':'basefile', 'read-only':True}),
        '-blockdev', json.dumps({'driver':'file', 'node-name':'overlayfile', 'filename':str(overlay)}),
        '-blockdev', json.dumps({'driver':'qcow2', 'node-name':'disk', 'file':'overlayfile', 'backing':'base'}),
        '-drive', 'if=none,id=one,driver=raw,file.driver=raw,file.file=disk',
        '-drive', 'if=none,id=two,driver=raw,file.driver=raw,file.file=disk']
    process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        with socket.socket(socket.AF_UNIX) as connection:
            for _ in range(100):
                try:
                    connection.connect(str(qmp))
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    if process.poll() is not None:
                        raise RuntimeError(process.stderr.read().decode()[:1500])
                    time.sleep(.05)
            else:
                raise TimeoutError('QMP did not accept connections')
            stream = connection.makefile('rwb', buffering=0)
            stream.readline()
            def call(command):
                stream.write(json.dumps({'execute':command}).encode() + b'\n')
                while True:
                    reply = json.loads(stream.readline())
                    if 'error' in reply:
                        raise RuntimeError(reply['error'])
                    if 'return' in reply:
                        return reply['return']
            call('qmp_capabilities')
            nodes = call('query-named-block-nodes')
            assert next(n for n in nodes if n['node-name'] == 'basefile')['ro'] is True
            assert next(n for n in nodes if n['node-name'] == 'base')['ro'] is True
            assert len(call('query-block')) == 2
            print('PASS shared qcow2 node with two legacy block backends and read-only backing')
            call('quit')
    finally:
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=5)
