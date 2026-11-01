"""Stage only locked registry inputs; never copy credentials or modify reference."""
import os
import hashlib
from pathlib import Path
import sys
import tarfile
import tomllib

root = Path(__file__).resolve().parents[2]
lock = tomllib.loads((root/'russet-w11-windows-dap/Cargo.lock').read_text())
reference = Path(os.environ.get('RUSSET_DEV_HOME', str(Path.home())))/'.cargo'/'registry'
cache_dirs = list((reference/'cache').iterdir())
entries = []
indexes = set()
for package in lock['package']:
    source = package.get('source', '')
    if not source:
        continue
    if not source.startswith('registry+'):
        raise RuntimeError('A non-registry dependency needs separate recovery')
    name, version = package['name'], package['version']
    archive_name = f'{name}-{version}.crate'
    candidates = [d/archive_name for d in cache_dirs if (d/archive_name).is_file()]
    original = next((p for p in candidates if hashlib.sha256(p.read_bytes()).hexdigest() == package['checksum']), None)
    if original is None:
        raise RuntimeError(f'Locked cache input unavailable: {name} {version}')
    registry = original.parent.name
    entries.append((original, f'registry/cache/{registry}/{archive_name}'))
    lower = name.lower()
    relative = f'1/{lower}' if len(lower)==1 else f'2/{lower}' if len(lower)==2 else f'3/{lower[0]}/{lower}' if len(lower)==3 else f'{lower[:2]}/{lower[2:4]}/{lower}'
    index = reference/'index'/registry
    record = index/'.cache'/relative
    if not record.is_file():
        raise RuntimeError(f'Locked index record unavailable: {name}')
    entries.append((record, f'registry/index/{registry}/.cache/{relative}'))
    indexes.add(index)
for index in indexes:
    entries.append((index/'config.json', f'registry/index/{index.name}/config.json'))
# Multiple versions share one index record.
entries = list({destination:(original,destination) for original,destination in entries}.values())
print(f'Locked registry inputs: {len(entries)} files, {sum(p.stat().st_size for p,_ in entries)} bytes',file=sys.stderr,flush=True)
if '--inventory' in sys.argv:
    raise SystemExit(0)
def metadata(info):
    info.uid=info.gid=1000
    info.uname=info.gname='russet'
    info.mode=0o644
    return info
with tarfile.open(fileobj=sys.stdout.buffer,mode='w|') as archive:
    for original,destination in entries:
        archive.add(original,arcname=destination,recursive=False,filter=metadata)
