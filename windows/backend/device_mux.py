"""Same-user Unix usbmux endpoint -> Windows-owned AMDS via WSL process stdio."""
import argparse
import asyncio
import os
from pathlib import Path
import signal
import socket
import struct
import sys

async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--socket', required=True)
    parser.add_argument('--host', required=True)
    args = parser.parse_args()
    target = Path(args.socket)
    if not str(target).startswith('/home/russet/') or '..' in target.parts:
        raise ValueError('Mux endpoint outside managed home')
    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if target.exists():
        raise FileExistsError('Refusing to replace an existing mux endpoint')
    os.umask(0o077)
    tasks = set()
    stopping = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stopping.set)

    async def copy(reader, writer):
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()

    async def client(reader, writer):
        task = asyncio.current_task()
        tasks.add(task)
        process = None
        pumps = []
        try:
            peer = writer.get_extra_info('socket')
            _, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            if uid != os.getuid():
                return
            process = await asyncio.create_subprocess_exec(args.host, '--device-mux', 'stdio',
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)
            pumps = [asyncio.create_task(copy(reader, process.stdin)), asyncio.create_task(copy(process.stdout, writer))]
            done, _ = await asyncio.wait(pumps, return_when=asyncio.FIRST_COMPLETED)
            for completed in done:
                completed.result()
        except (OSError, asyncio.CancelledError):
            pass
        finally:
            for pump in pumps:
                pump.cancel()
            await asyncio.gather(*pumps, return_exceptions=True)
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass
            if process:
                process.stdin.close()
                try:
                    await asyncio.wait_for(process.wait(), timeout=3)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()
            tasks.discard(task)

    server = await asyncio.start_unix_server(client, path=str(target))
    os.chmod(target, 0o600)
    print('ready', flush=True)
    # Host-side harness EOF controls lifecycle without killing unrelated processes.
    async def watch_host():
        reader = asyncio.StreamReader()
        protocol = asyncio.StreamReaderProtocol(reader)
        await loop.connect_read_pipe(lambda: protocol, sys.stdin.buffer)
        await reader.read()
        stopping.set()
    host = asyncio.create_task(watch_host())
    try:
        await stopping.wait()
    finally:
        host.cancel()
        server.close()
        await server.wait_closed()
        for task in list(tasks):
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        target.unlink(missing_ok=True)

if __name__ == '__main__':
    asyncio.run(main())
