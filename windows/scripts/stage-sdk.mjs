import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const here = path.dirname(fileURLToPath(import.meta.url));
const python = path.join(process.env.USERPROFILE, '.cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe');
const source = spawn(python, [path.join(here, 'sdk-archive.py')], {windowsHide:true, stdio:['ignore','pipe','inherit']});
const destination = spawn(path.join(process.env.SystemRoot, 'System32/wsl.exe'), [
  '-d','Russet-Dev','-u','russet','--exec','tar','-xf','-',
  '-C','/home/russet/.swiftpm/swift-sdks/darwin.artifactbundle',
], {windowsHide:true, stdio:['pipe','ignore','inherit']});
source.stdout.pipe(destination.stdin);
destination.stdin.on('error', () => source.kill());
const [[sourceCode], [destinationCode]] = await Promise.all([once(source,'exit'),once(destination,'exit')]);
if(sourceCode || destinationCode) throw new Error(`SDK stream failed (${sourceCode}, ${destinationCode})`);
console.log('Local iOS SDK staged by Windows archive stream');
