"""Windows-side SDK archive stream; verbatim paths preserve long Linux names."""
import os
import io
import json
from pathlib import Path
import sys
import tarfile

source = Path('\\\\?\\' + os.path.abspath(os.path.join(os.environ.get('RUSSET_DEV_HOME', os.path.expanduser('~')), '.swiftpm', 'swift-sdks', 'darwin.artifactbundle')))
selected = [
    'Developer/Platforms/iPhoneOS.platform',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/clang',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift',
    'Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift_static',
    'toolset', 'OpenAppleMacrosServer',
]
def metadata(info):
    info.uid = info.gid = 1000
    info.uname = info.gname = 'russet'
    executable = info.name == 'OpenAppleMacrosServer' or info.name.startswith('toolset/bin/')
    info.mode = 0o755 if info.isdir() or executable else 0o644
    if info.issym() or info.islnk():
        # Windows readlink exposes backslash separators even for WinBtrfs POSIX
        # links. Restore POSIX spelling before Linux tar materializes the link.
        info.linkname = info.linkname.replace('\\', '/')
    return info

with tarfile.open(fileobj=sys.stdout.buffer, mode='w|', dereference=False) as archive:
    for relative in selected:
        archive.add(source/relative, arcname=relative, filter=metadata)
        print('archived', relative, file=sys.stderr, flush=True)
    for relative in ['info.json','toolset.json','toolset-swb.json','darwin-sdk-version.txt',
                     'Xcode.app/Contents/version.plist',
                     'Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/version.plist']:
        archive.add(source/relative, arcname=relative, filter=metadata)
    descriptor = json.loads((source/'swift-sdk.json').read_text())
    descriptor['targetTriples'] = {'arm64-apple-ios': descriptor['targetTriples']['arm64-apple-ios']}
    data = json.dumps(descriptor, indent=2).encode()
    info = tarfile.TarInfo('swift-sdk.json')
    info.size = len(data)
    archive.addfile(metadata(info), io.BytesIO(data))
