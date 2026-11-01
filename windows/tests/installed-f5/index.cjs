// Runs inside the installed Russet editor, started by ../installed-f5.mjs with a
// lane-A backend. Drives the commands bound to F5/F10/Shift+F5 and the editor's
// own breakpoint model, then checks what the editor's debug client received.
const assert = require('node:assert/strict');
const path = require('node:path');
const vscode = require('vscode');

const messages = [];
let cursor = 0;
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, label, timeout = 180000) {
  for (const deadline = Date.now() + timeout; Date.now() < deadline; await delay(100)) {
    const value = check();
    if (value) return value;
  }
  throw new Error(`Timed out waiting for ${label}`);
}
// Next adapter/editor DAP message after the previous match, in protocol order.
async function expect(predicate, label, timeout) {
  return until(() => {
    const index = messages.findIndex((message, i) => i >= cursor && predicate(message));
    if (index < 0) return undefined;
    cursor = index + 1;
    return messages[index];
  }, label, timeout);
}
const response = command => m => m.from === 'adapter' && m.type === 'response' && m.command === command;
const stopped = m => m.from === 'adapter' && m.type === 'event' && m.event === 'stopped';

// On failure, a private trace of the DAP conversation's shape: commands, events,
// thread ids and reasons, never arguments, values or source contents.
exports.run = async () => {
  try { await run(); } catch (error) {
    if (process.env.RUSSET_F5_TRACE) require('node:fs').writeFileSync(process.env.RUSSET_F5_TRACE, messages.map(m => JSON.stringify({
      from: m.from, at: m.at, type: m.type, command: m.command, event: m.event, success: m.success, message: m.message,
      threadId: m.arguments?.threadId ?? m.body?.threadId, reason: m.body?.reason,
      allThreadsStopped: m.body?.allThreadsStopped, threads: m.body?.threads?.map(t => t.id),
    })).join('\n'));
    throw error;
  }
};

async function run() {
  assert.equal(process.env.RUSSET_LOCAL_DEVICE_TESTS, '1');
  const extension = vscode.extensions.getExtension('russet.russet');
  assert.ok(extension);
  assert.equal(path.resolve(extension.extensionPath).toLowerCase(),
    path.join(vscode.env.appRoot, 'extensions', 'russet').toLowerCase(), 'Not the installed bundled extension');
  const api = await extension.activate();
  await until(() => api.client.state === 'connected', 'daemon connection', 30000);
  await until(() => api.getActiveDevice(), 'active physical device', 60000);

  const folder = vscode.workspace.workspaceFolders[0];
  const source = vscode.Uri.file(path.join(folder.uri.fsPath, 'Sources/WindowsProbe/ContentView.swift'));
  await vscode.window.showTextDocument(source);
  vscode.debug.registerDebugAdapterTrackerFactory('russet', { createDebugAdapterTracker: () => ({
    onWillReceiveMessage: message => messages.push({ from: 'editor', at: Date.now(), ...message }),
    onDidSendMessage: message => messages.push({ from: 'adapter', at: Date.now(), ...message }),
  }) });
  const breakpoint = new vscode.SourceBreakpoint(new vscode.Location(source, new vscode.Position(5, 0)));
  vscode.debug.addBreakpoints([breakpoint]);

  await vscode.commands.executeCommand('workbench.action.debug.start'); // F5
  const session = await until(() => vscode.debug.activeDebugSession, 'debug session', 300000);
  // Backend dapConfig carries type lldb-dap; the resolved session must stay russet.
  assert.equal(session.type, 'russet');
  assert.equal(session.configuration.type, 'russet');
  const attach = await expect(m => m.from === 'editor' && m.command === 'attach', 'attach request');
  assert.equal(attach.arguments.type, 'russet');
  assert.ok(Array.isArray(attach.arguments.attachCommands), 'Backend attach recipe was not merged');

  const points = await expect(m => response('setBreakpoints')(m) && m.body?.breakpoints?.length === 1, 'breakpoint response');
  assert.equal(points.body.breakpoints[0].verified, true, 'Editor source breakpoint unresolved');
  let stop = await expect(stopped, 'first stop');
  if (stop.body.reason !== 'breakpoint') {
    await vscode.commands.executeCommand('workbench.action.debug.continue');
    stop = await expect(stopped, 'breakpoint stop');
  }
  assert.equal(stop.body.reason, 'breakpoint');
  const threadId = stop.body.threadId;
  // The editor focuses the frame and reveals the Windows source line.
  const item = await until(() => vscode.debug.activeStackItem?.frameId !== undefined && vscode.debug.activeStackItem, 'focused frame', 30000);
  const revealed = await until(() => {
    const editor = vscode.window.activeTextEditor;
    return editor?.document.uri.fsPath.toLowerCase() === source.fsPath.toLowerCase() && editor.selection.active.line === 5;
  }, 'revealed breakpoint line', 30000);
  const stack = await session.customRequest('stackTrace', { threadId, startFrame: 0, levels: 1 });
  assert.equal(stack.stackFrames[0].source.path.toLowerCase(), source.fsPath.toLowerCase());
  assert.equal(stack.stackFrames[0].line, 6);
  const scopes = await session.customRequest('scopes', { frameId: item.frameId });
  const locals = scopes.scopes.find(scope => scope.name === 'Locals');
  assert.ok(locals);
  const variables = await session.customRequest('variables', { variablesReference: locals.variablesReference });
  assert.equal(variables.variables.find(variable => variable.name === 'probeValue')?.value, '42');

  // Swift's return expression has several locations for one source line; toggle
  // the breakpoint off in the editor before stepping, as the scripted test does.
  vscode.debug.removeBreakpoints([breakpoint]);
  await expect(m => response('setBreakpoints')(m) && m.body?.breakpoints?.length === 0, 'breakpoint removal');
  await vscode.commands.executeCommand('workbench.action.debug.stepOver'); // F10
  const stepped = await expect(stopped, 'step stop');
  assert.equal(stepped.body.reason, 'step');
  await vscode.commands.executeCommand('workbench.action.debug.continue'); // F5 while paused
  await expect(response('continue'), 'continue response');
  await delay(250);
  await vscode.commands.executeCommand('workbench.action.debug.pause'); // F6
  const paused = await expect(stopped, 'pause stop');
  const interruptSignal = /SIGSTOP|SIGINT/.test(String(paused.body.description ?? paused.body.text ?? ''));
  assert.ok(['pause', 'signal'].includes(paused.body.reason) || paused.body.reason === 'exception' && interruptSignal,
    'Expected a pause or explicit interrupt signal after continued execution');
  await vscode.commands.executeCommand('workbench.action.debug.continue');
  await expect(response('continue'), 'second continue response');

  const terminated = new Promise(resolve => vscode.debug.onDidTerminateDebugSession(ended => ended.id === session.id && resolve()));
  await vscode.commands.executeCommand('workbench.action.debug.stop'); // Shift+F5
  await Promise.race([terminated, delay(30000).then(() => { throw new Error('Session did not terminate'); })]);
  await expect(m => m.from === 'editor' && m.command === 'disconnect', 'disconnect request', 1000);
  console.log(JSON.stringify({ installedEditorF5: true, sessionType: session.type, attachMergedRecipe: true,
    breakpointVerified: true, revealedLine: revealed, probeValue: 42, stepped: true, pausedAfterContinue: paused.body.reason,
    interruptSignal, continuedAgain: true, terminated: true }));
}
