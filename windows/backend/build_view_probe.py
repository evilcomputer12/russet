"""Explicit development projection, not a production sync or workspace manager.

Only ignored outputs are redirected. Package.swift and Sources remain links to
the authoritative Windows project, so backend builders using relative paths see
the correct project layout. No source copies or reverse synchronization occur.
"""
from pathlib import Path
import re
import sys

name = sys.argv[1] if len(sys.argv) > 1 else 'WindowsProbe'
assert re.fullmatch(r'Windows(?:Physical)?Probe', name)
source = Path(__file__).resolve().parents[1]/'.runtime'/'projects'/name
view = Path('/home/russet/builds')/('probe-view' if name == 'WindowsProbe' else name)
view.mkdir(parents=True, mode=0o700, exist_ok=True)
for name in ['Package.swift', 'Sources', 'russet.toml', 'russet.local.toml']:
    target = view/name
    original = source/name
    if name == 'russet.local.toml' and not original.exists():
        continue
    assert original.exists()
    if target.is_symlink():
        assert target.readlink() == original
    else:
        assert not target.exists()
        target.symlink_to(original, target_is_directory=original.is_dir())
outputs = view/'.russet'
outputs.mkdir(mode=0o700, exist_ok=True)
link = source/'.russet'
if link.is_symlink():
    assert str(link.readlink()).startswith('/home/russet/builds/')
    link.unlink()
else:
    assert not link.exists(), 'Preserve existing project outputs before redirecting'
link.symlink_to(outputs, target_is_directory=True)
print('Explicit source-link build view ready; Windows source remains authoritative')
