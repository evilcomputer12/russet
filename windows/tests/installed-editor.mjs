import { runTests } from '../../russet-w14-windows-bridge/extension/node_modules/@vscode/test-electron/out/index.js';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const installation = path.join(process.env.LOCALAPPDATA, 'Programs/Russet Development');
const home = `/home/russet/.russet-dev/installed-test-${randomUUID()}`;
const wsl = path.join(process.env.SystemRoot, 'System32/wsl.exe');
const env = ['env', `RUSSET_HOME=${home}`, 'RUSSET_DEVICE_BACKEND=none', 'RUSSET_SECRETS_BACKEND=memory'];
const daemon = spawn(wsl, ['-d','Russet-Dev','-u','russet','--exec',...env,
  '/opt/russet/bin/russet-recovered','daemon','run','--foreground','--idle-timeout','60s'], {windowsHide:true,stdio:'ignore'});
try {
  await runTests({
    vscodeExecutablePath: path.join(installation,'editor/Russet.exe'),
    extensionDevelopmentPath: path.join(root,'tests/installed-smoke'),
    extensionTestsPath: path.join(root,'tests/installed-smoke/index.cjs'),
    extensionTestsEnv: { RUSSET_WINDOWS_HOST:path.join(installation,'host/Russet.Host.exe'), RUSSET_BACKEND_HOME:home },
    launchArgs: [path.join(root,'../russet-w14-windows-bridge/extension/test/fixtures/HelloProject'),
      '--user-data-dir',path.join(root,'.runtime/installed-editor-test'),
      '--extensions-dir',path.join(root,'.runtime/installed-editor-test-extensions'),
      '--skip-welcome','--skip-release-notes','--disable-workspace-trust'],
  });
} finally {
  const stop = spawn(wsl,['-d','Russet-Dev','-u','russet','--exec',...env,'/opt/russet/bin/russet-recovered','daemon','stop'],{windowsHide:true,stdio:'ignore'});
  await once(stop,'exit');
  if(daemon.exitCode === null) daemon.kill();
}
