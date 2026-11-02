"""Repair only migrated symlinks, using their actual Linux reference targets."""
import os
from pathlib import Path

# The reference SDK in the developer's home (RUSSET_DEV_HOME, default: this user's home).
source = Path(os.environ.get('RUSSET_DEV_HOME', str(Path.home())))/'.swiftpm/swift-sdks/darwin.artifactbundle'
target = Path('/home/russet/.swiftpm/swift-sdks/darwin.artifactbundle')
count = 0
for directory, dirs, files in os.walk(target, followlinks=False):
    for name in dirs + files:
        file = Path(directory)/name
        if not file.is_symlink():
            continue
        link = os.readlink(file)
        if '\\' not in link:
            continue
        original = source/file.relative_to(target)
        original_link = os.readlink(original)
        if original_link != link.replace('\\', '/'):
            raise ValueError('Unexpected SDK link migration; refusing speculative rewrite')
        file.unlink()
        file.symlink_to(original_link)
        count += 1
broken = sum(1 for directory, dirs, files in os.walk(target, followlinks=False)
    for name in dirs + files for p in [Path(directory)/name] if p.is_symlink() and not p.exists())
print('SDK symlinks restored from reference:', count, '; remaining broken links:', broken)
if broken:
    raise RuntimeError('Incomplete SDK links')
