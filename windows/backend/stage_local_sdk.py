"""Stage the owner's already-obtained iOS SDK subset into the managed backend."""
import os
import json
from pathlib import Path
import shutil

# The reference SDK in the developer's home (RUSSET_DEV_HOME, default: this user's home).
source = Path(os.environ.get('RUSSET_DEV_HOME', str(Path.home())))/'.swiftpm/swift-sdks/darwin.artifactbundle'
target = Path('/home/russet/.swiftpm/swift-sdks/darwin.artifactbundle')
if target.exists():
    raise FileExistsError('Refusing to overwrite a staged SDK')
target.mkdir(parents=True)
selected = [
    'Developer/Platforms/iPhoneOS.platform',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/clang',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift_static',
    'toolset', 'OpenAppleMacrosServer',
]
for relative in selected:
    incoming, output = source/relative, target/relative
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(incoming, output, symlinks=True)
    print('staged', relative, flush=True)
for relative in ['info.json', 'toolset.json', 'toolset-swb.json', 'darwin-sdk-version.txt', 'Xcode.app/Contents/version.plist']:
    output = target/relative
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source/relative, output)
descriptor = json.loads((source/'swift-sdk.json').read_text())
descriptor['targetTriples'] = {'arm64-apple-ios': descriptor['targetTriples']['arm64-apple-ios']}
(target/'swift-sdk.json').write_text(json.dumps(descriptor, indent=2))
(target/'migration-provenance.json').write_text(json.dumps({
    'source':str(source), 'selected':selected, 'targetTriples':['arm64-apple-ios'],
    'date':'2026-10-31', 'redistributable':False,
}, indent=2))
print('staged local iOS-only SDK; no Apple download or redistribution', flush=True)
