import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import net from 'node:net';
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import os from 'node:os';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';

const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const host = path.join(project, 'src/Russet.Host/bin/Release/net10.0/Russet.Host.dll');
const wsl = path.join(process.env.SystemRoot, 'System32/wsl.exe');
const helper = '/opt/russet/lib/socket_relay.py';
// The fixture as WSL sees this checkout (/mnt/<drive>/...).
const backendFixture = path.join(project, 'tests', 'backend_fixture.py')
  .replace(/^([A-Za-z]):\\/, (_, d) => `/mnt/${d.toLowerCase()}/`).replace(/\\/g, '/');
const session = `/home/russet/.russet-dev/transport-${randomUUID()}`;
const children = new Set();

function child(command, args) {
  const p = spawn(command, args, { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  children.add(p);
  p.stderrText = '';
  p.stderr.on('data', b => { p.stderrText = (p.stderrText + b).slice(-4000); });
  p.once('exit', () => children.delete(p));
  return p;
}
function backend(args) { return child(wsl, ['-d', 'Russet-Dev', '-u', 'russet', '--exec', ...args]); }
async function line(p, timeout = 30000) {
  return new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => done(new Error(`Readiness timeout pid ${p.pid}: ${p.stderrText}`)), timeout);
    function done(error, value) {
      clearTimeout(timer); p.stdout.off('data', data); p.off('exit', exited);
      error ? reject(error) : resolve(value);
    }
    function data(b) { text += b; if (text.includes('\n')) done(null, text.split('\n')[0]); }
    function exited(code) { done(new Error(`Exited ${code}: ${p.stderrText}`)); }
    p.stdout.on('data', data); p.once('exit', exited);
  });
}
async function connect(endpoint) {
  const socket = net.connect(endpoint);
  await once(socket, 'connect');
  return socket;
}
async function relay(socketPath, protocol = 'raw') {
  const p = child('dotnet', [host, '--distribution', 'Russet-Dev', '--socket', socketPath, '--helper', helper, '--protocol', protocol]);
  const ready = JSON.parse(await line(p));
  assert.equal(ready.state, 'ready');
  assert.equal(ready.securityVerified, true);
  assert.match(ready.endpoint, /^\\\\\.\\pipe\\russet-dev-/);
  return { p, socket: await connect(ready.endpoint), endpoint: ready.endpoint };
}
function frame(value) {
  const bytes = Buffer.from(JSON.stringify(value));
  return Buffer.concat([Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`), bytes]);
}
function messages(socket) {
  let pending = Buffer.alloc(0);
  const queue = [], waiting = [];
  socket.on('data', chunk => {
    pending = Buffer.concat([pending, chunk]);
    while (true) {
      const end = pending.indexOf('\r\n\r\n');
      if (end < 0) return;
      const size = Number(/Content-Length:\s*(\d+)/i.exec(pending.subarray(0, end).toString())[1]);
      if (pending.length < end + 4 + size) return;
      const value = JSON.parse(pending.subarray(end + 4, end + 4 + size));
      pending = pending.subarray(end + 4 + size);
      if (waiting.length) waiting.shift()(value); else queue.push(value);
    }
  });
  return () => queue.length ? Promise.resolve(queue.shift()) : new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Protocol response timeout')), 5000);
    waiting.push(v => { clearTimeout(timer); resolve(v); });
  });
}
async function closeRelay(r) {
  const exited = once(r.p, 'exit');
  r.socket.destroy();
  const [code] = await Promise.race([exited, new Promise((_, reject) => setTimeout(() => reject(new Error('Relay cleanup timeout')), 7000).unref())]);
  assert.equal(code, 0, r.p.stderrText);
}

test('Windows named pipe -> WSL same-user Unix socket: DAP bytes and cleanup', { timeout: 90000 }, async () => {
  const fixture = backend(['python3', '-I', backendFixture, session]);
  assert.equal(await line(fixture), 'ready');
  try {
    for (let iteration = 0; iteration < 2; iteration++) {
      const r = await relay(`${session}/dap.sock`);
      const next = messages(r.socket);
      // Fragmented headers, coalesced messages, multibyte paths and large payloads.
      const values = [
        { seq: 1, type: 'request', command: 'setBreakpoints', arguments: { source: { path: '/home/russet/Проект со празни места/App.swift' }, breakpoints: [{ line: 12 }] } },
        { seq: 2, type: 'event', event: 'output', body: { output: '✓'.repeat(100000) } },
        { seq: 3, type: 'request', command: 'continue', arguments: { threadId: 1 } },
      ];
      const payload = Buffer.concat(values.map(frame));
      r.socket.write(payload.subarray(0, 7));
      r.socket.write(payload.subarray(7, 18));
      r.socket.write(payload.subarray(18));
      for (const expected of values) assert.deepEqual(await next(), expected);
      await closeRelay(r);
      assert.equal((await backendResult(['python3', '-c', 'import os; print(len(os.listdir("/proc/self/fd")))'])).code, 0);
    }
  } finally { fixture.stdin.end(); await once(fixture, 'exit'); }
});

async function backendResult(args) {
  const p = backend(args); const [code] = await once(p, 'exit'); return { code, stderr: p.stderrText };
}

test('Rejects permissive Unix socket before reporting ready', { timeout: 15000 }, async () => {
  const fixture = backend(['python3', '-I', backendFixture, `${session}-bad`, '--insecure']);
  assert.equal(await line(fixture), 'ready');
  try {
    const result = await backendResult(['python3', '-I', helper, '--socket', `${session}-bad/dap.sock`, '--probe']);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /rejected/);
  } finally { fixture.stdin.end(); await once(fixture, 'exit'); }
});

test('DAP source paths cross Windows/WSL with spaces, Unicode and preserved app arguments', { timeout: 30000 }, async () => {
  const fixture = backend(['python3', '-I', backendFixture, `${session}-paths`, '--inspect-source']);
  assert.equal(await line(fixture), 'ready');
  try {
    const r = await relay(`${session}-paths/dap.sock`, 'dap');
    try {
      const next = messages(r.socket);
      const source = 'C:\\Russet Проект\\Sources\\App.swift';
      r.socket.write(frame({ seq: 1, type: 'request', command: 'setBreakpoints', arguments: { source: { path: source }, args: [source], env: { path: source } } }));
      const result = await next();
      assert.equal(result.backendObservedPath, '/mnt/c/Russet Проект/Sources/App.swift');
      assert.equal(result.arguments.source.path, source);
      assert.deepEqual(result.arguments.args, [source]);
      assert.deepEqual(result.arguments.env, { path: source });
      // Files picked in the editor: an App Store Connect key, a secret's source file, an IPA destination.
      const key = 'c:\\Users\\Russet Проект\\Downloads\\AuthKey_TEST000001.p8';
      r.socket.write(frame({ seq: 2, type: 'request', command: 'setBreakpoints', arguments: { source: { path: source }, p8Path: key, fromFile: key, out: key, label: key } }));
      const picked = await next();
      const seen = JSON.parse(picked.backendObservedMessage).arguments;
      const linux = '/mnt/c/Users/Russet Проект/Downloads/AuthKey_TEST000001.p8';
      assert.deepEqual([seen.p8Path, seen.fromFile, seen.out], [linux, linux, linux]);
      assert.equal(seen.label, key);
      assert.equal(picked.arguments.p8Path, 'C:\\Users\\Russet Проект\\Downloads\\AuthKey_TEST000001.p8');
    } finally { await closeRelay(r); }
  } finally { fixture.stdin.end(); await once(fixture, 'exit'); }
});

test('Real Russet daemon: initialize, ping, unknown method, shutdown over host relay', { timeout: 30000 }, async () => {
  const home = `${session}-rpc`;
  const daemon = backend(['env', `RUSSET_HOME=${home}`, 'RUSSET_DEVICE_BACKEND=none', 'RUSSET_SECRETS_BACKEND=memory', '/opt/russet/bin/russet-recovered', 'daemon', 'run', '--foreground', '--idle-timeout', '30s']);
  const ready = await backendResult(['python3', '-c', 'import pathlib,time,sys; p=pathlib.Path(sys.argv[1]);\nfor i in range(100):\n if p.exists(): break\n time.sleep(.05)\nelse: sys.exit(1)', `${home}/runtime/russetd.sock`]);
  assert.equal(ready.code, 0, daemon.stderrText);
  const r = await relay(`${home}/runtime/russetd.sock`, 'rpc');
  const projectRoot = await mkdtemp(path.join(os.tmpdir(), 'Russet Проект '));
  await writeFile(path.join(projectRoot, 'Package.swift'), '// swift-tools-version: 6.0\nimport PackageDescription\nlet package = Package(name: "Probe", products: [.library(name: "Probe", type: .dynamic, targets: ["Probe"])], targets: [.target(name: "Probe")])\n');
  try {
    const next = messages(r.socket);
    r.socket.write(frame({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {
      protocol: { major: 1, minor: 0 }, client: { kind: 'ide', name: 'russet-windows-bridge-test', version: '0.1.0', pid: process.pid },
      capabilities: { confirm: false, prompt: false, openUrl: false, approvals: false, ideDebug: true, interactive: false }, workspace: { roots: [] },
    } }));
    const hello = await next();
    assert.equal(hello.result?.protocol.major, 1, JSON.stringify(hello.error));
    assert.equal(hello.result.server.deviceBackend, 'none');
    r.socket.write(Buffer.concat([
      frame({ jsonrpc: '2.0', id: 2, method: 'daemon.ping', params: {} }),
      frame({ jsonrpc: '2.0', id: 3, method: 'unknown.method', params: {} }),
    ]));
    const replies = [await next(), await next()];
    assert.deepEqual(replies.find(v => v.id === 2)?.result, {});
    assert.equal(replies.find(v => v.id === 3)?.error?.code, -32601);
    r.socket.write(frame({ jsonrpc: '2.0', id: 5, method: 'project.detect', params: { dir: projectRoot } }));
    const detected = await next();
    assert.equal(detected.result?.stack, 'swiftpm', JSON.stringify(detected.error));
    assert.equal(detected.result.root.toLowerCase(), projectRoot.toLowerCase());
    r.socket.write(frame({ jsonrpc: '2.0', id: 4, method: 'daemon.shutdown', params: { force: false } }));
    // Shutdown may broadcast before its response.
    for (let i = 0; i < 4; i++) { const message = await next(); if (message.id === 4) { assert.ok(message.result, JSON.stringify(message.error)); break; } }
  } finally { await closeRelay(r); daemon.kill(); await rm(projectRoot, { recursive: true, force: true }); }
});

process.on('exit', () => { for (const p of children) p.kill(); });
