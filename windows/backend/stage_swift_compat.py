"""Keep exact legacy library ABIs private to the recovered Swift toolchain."""
from pathlib import Path
import shutil
import subprocess

source = Path('/mnt/d/@/usr/lib')
target = Path('/usr/lib/swift/usr/lib/swift/linux')
for name in ['libxml2.so.2', 'libicuuc.so.78', 'libicudata.so.78', 'libedit.so.0']:
    output = target/name
    if output.exists():
        continue  # Keep previously staged private libraries intact.
    incoming = (source/name).resolve(strict=True)
    if not incoming.is_relative_to(source):
        raise ValueError('Unexpected compatibility library source')
    shutil.copy2(incoming, output)
    subprocess.run(['patchelf', '--set-rpath', '$ORIGIN', str(output)], check=True)
    print('staged private ABI:', name, flush=True)
