"""Adapt the copied Arch toolchain's unversioned ncurses dependency to Ubuntu ABI 6.

The reference host's libncursesw.so points to libncursesw.so.6. Ubuntu's unversioned
name is a linker script, which the ELF loader cannot load. Only managed copies
are changed; the runtime ABI and reference source stay unchanged.
"""
import os
from pathlib import Path
import subprocess

for directory, _, names in os.walk('/usr/lib/swift'):
    for name in names:
        file = Path(directory)/name
        if file.is_symlink() or not file.is_file():
            continue
        with file.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                continue
        dynamic = subprocess.run(['readelf', '-d', str(file)], capture_output=True, text=True, check=True).stdout
        if 'Shared library: [libncursesw.so]' in dynamic:
            subprocess.run(['patchelf', '--replace-needed', 'libncursesw.so', 'libncursesw.so.6', str(file)], check=True)
            print('adapted ncurses ABI-6 dependency:', name, flush=True)
