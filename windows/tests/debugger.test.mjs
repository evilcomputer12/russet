import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import net from 'node:net';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { mkdir, writeFile, rm } from 'node:fs/promises';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const wsl = path.join(process.env.SystemRoot, 'System32/wsl.exe');
const linuxPath = value => `/mnt/${value[0].toLowerCase()}/${value.slice(3).replaceAll('\\', '/')}`;
function child(command, args) {
  const process = spawn(command, args, { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  process.diagnostic = '';
  process.stderr.on('data', b => { process.diagnostic = (process.diagnostic + b).slice(-4000); });
  return process;
}
function backend(args) { return child(wsl, ['-d', 'Russet-Dev', '-u', 'russet', '--exec', ...args]); }
async function line(process) {
  return new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => done(new Error(`Readiness timeout: ${process.diagnostic}`)), 30000);
    function done(error, value) {
      clearTimeout(timer); process.stdout.off('data', data); process.off('exit', exited);
      error ? reject(error) : resolve(value);
    }
    function data(b) { text += b; if (text.includes('\n')) done(null, text.split('\n')[0]); }
    function exited(code) { done(new Error(`Readiness exit ${code}: ${process.diagnostic}`)); }
    process.stdout.on('data', data); process.once('exit', exited);
  });
}
function dap(socket) {
  let pending = Buffer.alloc(0), sequence = 0;
  const queued = [], waiters = [];
  socket.on('data', chunk => {
    pending = Buffer.concat([pending, chunk]);
    while (true) {
      const end = pending.indexOf('\r\n\r\n');
      if (end < 0) return;
      const size = Number(/Content-Length:\s*(\d+)/i.exec(pending.subarray(0, end).toString())[1]);
      if (pending.length < end + 4 + size) return;
      const message = JSON.parse(pending.subarray(end + 4, end + 4 + size));
      pending = pending.subarray(end + 4 + size);
      const index = waiters.findIndex(waiter => waiter.predicate(message));
      if (index >= 0) waiters.splice(index, 1)[0].resolve(message); else queued.push(message);
    }
  });
  function wait(predicate) {
    const index = queued.findIndex(predicate);
    if (index >= 0) return Promise.resolve(queued.splice(index, 1)[0]);
    return new Promise((resolve, reject) => {
      const entry = { predicate, resolve: message => { clearTimeout(timer); resolve(message); } };
      const timer = setTimeout(() => { waiters.splice(waiters.indexOf(entry), 1); reject(new Error('DAP semantic response timeout')); }, 30000);
      waiters.push(entry);
    });
  }
  async function request(command, args = {}) {
    const seq = ++sequence;
    const bytes = Buffer.from(JSON.stringify({ seq, type: 'request', command, arguments: args }));
    socket.write(Buffer.concat([Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`), bytes]));
    const result = await wait(message => message.type === 'response' && message.request_seq === seq);
    assert.equal(result.success, true, JSON.stringify(result));
    return result.body;
  }
  return { request, event: name => wait(message => message.type === 'event' && message.event === name) };
}

test('Real LLDB over Windows pipe: source breakpoint, variables, next and continue (Linux host target)', { timeout: 120000 }, async () => {
  const id = randomUUID();
  const home = `/home/russet/.russet-dev/lldb-${id}`;
  const work = path.join(root, '.runtime', `Debug Проект ${id}`);
  await mkdir(work, { recursive: true });
  const source = path.join(work, 'probe.c'), program = path.join(work, 'probe');
  await writeFile(source, '#include <stdio.h>\nint main(void) {\n  int counter = 41;\n  counter += 1;\n  printf("%d\\n", counter);\n  return counter == 42 ? 0 : 1;\n}\n');
  const compile = backend(['gcc', '-g', '-O0', '-fno-omit-frame-pointer', linuxPath(source), '-o', linuxPath(program)]);
  assert.equal((await once(compile, 'exit'))[0], 0, compile.diagnostic);
  const endpoint = backend(['python3', '-I', linuxPath(path.join(root, 'tests/lldb_endpoint.py')), home]);
  let relay, socket;
  try {
    assert.equal(await line(endpoint), 'ready');
    relay = child('dotnet', [path.join(root, 'src/Russet.Host/bin/Release/net10.0/Russet.Host.dll'),
      '--distribution', 'Russet-Dev', '--socket', `${home}/dap.sock`, '--helper', '/opt/russet/lib/socket_relay.py', '--protocol', 'dap']);
    const ready = JSON.parse(await line(relay));
    socket = net.connect(ready.endpoint);
    await once(socket, 'connect');
    const client = dap(socket);
    const capabilities = await client.request('initialize', { adapterID: 'lldb', clientID: 'russet-windows-test', pathFormat: 'path', linesStartAt1: true, columnsStartAt1: true });
    assert.equal(capabilities.supportsConfigurationDoneRequest, true);
    await client.request('launch', { program, cwd: work, disableASLR: false, initCommands: ['settings set target.disable-aslr false'] });
    await client.event('initialized');
    const breakpoints = await client.request('setBreakpoints', { source: { path: source }, breakpoints: [{ line: 4 }] });
    assert.equal(breakpoints.breakpoints[0].verified, true);
    await client.request('configurationDone');
    const stopped = await client.event('stopped');
    assert.equal(stopped.body.reason, 'breakpoint');
    const threadId = stopped.body.threadId;
    async function state() {
      const stack = await client.request('stackTrace', { threadId, startFrame: 0, levels: 1 });
      const top = stack.stackFrames[0];
      assert.equal(top.source.path.toLowerCase(), source.toLowerCase());
      const scopes = await client.request('scopes', { frameId: top.id });
      const locals = scopes.scopes.find(scope => scope.name === 'Locals');
      assert.ok(locals, JSON.stringify(scopes));
      const variables = await client.request('variables', { variablesReference: locals.variablesReference });
      return { line: top.line, counter: variables.variables.find(variable => variable.name === 'counter')?.value };
    }
    assert.deepEqual(await state(), { line: 4, counter: '41' });
    await client.request('next', { threadId });
    assert.equal((await client.event('stopped')).body.reason, 'step');
    assert.deepEqual(await state(), { line: 5, counter: '42' });
    await client.request('continue', { threadId });
    assert.equal((await client.event('exited')).body.exitCode, 0);
    await client.event('terminated');
    await client.request('disconnect', { terminateDebuggee: true });
  } finally {
    socket?.destroy();
    if (relay && relay.exitCode === null) {
      await Promise.race([once(relay, 'exit'), new Promise(resolve => setTimeout(() => { relay.kill(); resolve(); }, 5000).unref())]);
    }
    endpoint.stdin.end();
    if (endpoint.exitCode === null) await once(endpoint, 'exit');
    await rm(work, { recursive: true, force: true });
  }
});
