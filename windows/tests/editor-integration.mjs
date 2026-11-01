import { runTests } from '../../russet-w14-windows-bridge/extension/node_modules/@vscode/test-electron/out/index.js';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const extension = path.resolve(root, '../russet-w14-windows-bridge/extension');
const home = `/home/russet/.russet-dev/editor-${randomUUID()}`;
const daemon = spawn(path.join(process.env.SystemRoot, 'System32/wsl.exe'), [
  '-d', 'Russet-Dev', '-u', 'russet', '--exec', 'env', `RUSSET_HOME=${home}`,
  'RUSSET_DEVICE_BACKEND=none', 'RUSSET_SECRETS_BACKEND=memory',
  '/opt/russet/bin/russet-recovered', 'daemon', 'run', '--foreground', '--idle-timeout', '60s',
], { windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] });
daemon.stderr.on('data', () => {});
try {
  await runTests({
    vscodeExecutablePath: path.join(process.env.LOCALAPPDATA, 'Programs/Microsoft VS Code/Code.exe'),
    extensionDevelopmentPath: extension,
    extensionTestsPath: path.join(extension, 'dist-test/windows/windows-backend.test.js'),
    extensionTestsEnv: {
      RUSSET_WINDOWS_HOST: path.join(root, 'src/Russet.Host/bin/Release/net10.0/Russet.Host.exe'),
      RUSSET_BACKEND_HOME: home,
      RUSSET_DEVICE_BACKEND: 'none', RUSSET_SECRETS_BACKEND: 'memory',
      RUSSET_TEST_HOLD_UI: process.env.RUSSET_TEST_HOLD_UI ?? '0',
    },
    launchArgs: [
      path.join(extension, 'test/fixtures/HelloProject'),
      '--user-data-dir', path.join(root, '.runtime/editor-test-user'),
      '--extensions-dir', path.join(root, '.runtime/editor-test-extensions'),
      '--disable-extensions', '--skip-welcome', '--skip-release-notes', '--disable-workspace-trust', '--log', 'debug',
    ],
  });
} finally {
  // Target only the exact daemon home created by this harness, never another session.
  const shutdown = spawn(path.join(process.env.SystemRoot, 'System32/wsl.exe'), [
    '-d', 'Russet-Dev', '-u', 'russet', '--exec', 'env', `RUSSET_HOME=${home}`,
    'RUSSET_DEVICE_BACKEND=none', 'RUSSET_SECRETS_BACKEND=memory',
    '/opt/russet/bin/russet-recovered', 'daemon', 'stop',
  ], { windowsHide: true, stdio: 'ignore' });
  await new Promise(resolve => shutdown.once('exit', resolve));
  if (daemon.exitCode === null) daemon.kill();
}
