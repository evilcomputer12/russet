const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vscode = require('vscode');

exports.run = async () => {
  assert.equal(vscode.env.appName, 'Russet Development');
  const product = JSON.parse(fs.readFileSync(path.join(vscode.env.appRoot, 'product.json'), 'utf8').replace(/^\uFEFF/, ''));
  assert.equal(product.enableTelemetry, false);
  assert.equal(product.updateUrl, '');
  assert.equal(product.extensionsGallery.serviceUrl, 'https://open-vsx.org/vscode/gallery');
  const extension = vscode.extensions.getExtension('russet.russet');
  assert.ok(extension);
  assert.equal(path.resolve(extension.extensionPath).toLowerCase(), path.join(vscode.env.appRoot, 'extensions', 'russet').toLowerCase());
  const api = await extension.activate();
  const deadline = Date.now() + 30000;
  while (api.client.state !== 'connected' && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal(api.client.state, 'connected');
  assert.deepEqual(await api.client.call('daemon.ping', {}), {});
  const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
  const result = await api.client.call('project.detect', { dir: root });
  assert.equal(result.stack, 'swiftpm');
  assert.equal(result.root.toLowerCase(), root.toLowerCase());
  console.log('PASS installed Russet: bundled extension, Open VSX, telemetry/update settings, RPC and Windows project');
};
