"""Fresh engine build with the repository gate and owner's physical C: budget."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

# The developer's Rust toolchain and cargo home (RUSSET_DEV_HOME, default: this user's home).
dev_home = Path(os.environ.get('RUSSET_DEV_HOME', str(Path.home())))
toolchain = str(dev_home/'.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin')
source = Path(os.environ.get('RUSSET_ENGINE_SOURCE', '/home/russet/src/engine-w11'))
slot = Path('/home/russet/.cache/russet-dev/target-b')
slot.mkdir(parents=True, exist_ok=True)
python = '/home/russet/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu'
isolated_home = Path('/home/russet/.russet-dev/ci-home')
(isolated_home/'.config').mkdir(mode=0o700, parents=True, exist_ok=True)
environment = dict(os.environ, PATH=f'{toolchain}:{dev_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin',
    HOME=str(isolated_home), XDG_CONFIG_HOME=str(isolated_home/'.config'),
    RUSTC=f'{toolchain}/rustc', RUSTDOC=f'{toolchain}/rustdoc', RUSTFMT=f'{toolchain}/rustfmt',
    CARGO_HOME=os.environ.get('RUSSET_CARGO_HOME', '/home/russet/.cargo'), CARGO_TARGET_DIR=str(slot), CARGO_BUILD_JOBS=os.environ.get('RUSSET_CARGO_JOBS', '6'), CARGO_INCREMENTAL='0',
    CARGO_NET_OFFLINE='true', RUSSET_OFFLINE='1', RUSSET_DEVICE_BACKEND='none', RUSSET_SECRETS_BACKEND='memory',
    RUSSET_HOME='/home/russet/.russet-dev/engine-build',
    RUSSET_TEST_LLDB_DAP='/usr/lib/swift/usr/bin/lldb-dap',
    RUSSET_TEST_LLDB_ENV=json.dumps({'PYTHONHOME':python,
        'LD_LIBRARY_PATH':f'{python}/lib:/usr/lib/swift/usr/lib/swift/linux:/usr/lib/swift/usr/lib/swift/host/compiler'}))
environment.pop('RUSSET_LANE', None)
environment.pop('USBMUXD_SOCKET_ADDRESS', None)
baseline_free = 379717488640
def disk_limit():
    """Russet's disk limit as the owner set it (Russet: Change WSL Limits), less a 4.5 GB reserve;
    the approved ~200 GB of temporary C: storage (owner, 2026-11-01) until they set one."""
    for path in Path('/mnt/c/Users').glob('*/AppData/Local/Russet/limits.json'):
        try:
            return int(json.loads(path.read_text())['diskGB'])*1_000_000_000 - 4_500_000_000
        except (OSError, ValueError, KeyError, TypeError):
            pass
    return 195_500_000_000
limit = disk_limit()
mode = sys.argv[1] if len(sys.argv)>1 else 'test'
commands = {
    'test':['cargo','xtask','test','--locked','--offline','-p','russet-debug','--test','dap_lldb','--','--ignored','--nocapture'],
    'unit':['cargo','xtask','test','--locked','--offline','-p','russet-debug','--lib','dap::tests'],
    # The CLI/daemon plus W22's sandboxed ADI helper, which the daemon finds beside itself.
    'build':['cargo','xtask','build','--locked','--offline','-p','russet-cli','-p','russet-adi-host'],
    'ci':['cargo','xtask','ci','--profile','pr'],
    'gates':['cargo','xtask','ci','--profile','pr','--skip','test'],
    # CI's nextest command for chosen packages: `nextest -p crate…`.
    'nextest':['cargo','xtask','test','--locked','--profile','ci']+sys.argv[2:],
    # Any cargo command in the gated environment: `cargo xtask schema`, `cargo clippy -p crate…`.
    'cargo':['cargo']+sys.argv[2:],
}
# One --workspace nextest build links about 120 test executables, beyond the 6 GiB slot cap.
# ci-split runs every other PR gate unchanged, then the CI's exact nextest command over
# disjoint target selections. --workspace keeps feature unification identical to CI.
nextest = ['cargo','xtask','test','--workspace','--locked','--profile','ci']
# Crates declare no [[test]] tables, so integration targets are their tests/*.rs stems.
# Eight linked integration executables fit beside the shared rlibs under the cap.
stems = sorted({path.stem for path in source.glob('crates/*/tests/*.rs')})
chunks = [stems[start:start+8] for start in range(0, len(stems), 8)]
split = [('ci-gates',['cargo','xtask','ci','--profile','pr','--skip','test']),
    ('ci-test-units',nextest+['--lib','--bins','--examples'])]+[
    (f'ci-test-{chunk[0]}',nextest+[arg for stem in chunk for arg in ('--test',stem)]) for chunk in chunks]

def drop_reclaimable():
    """Unlink test/bin executables and check-only metadata; rlibs, proc macros and build scripts stay."""
    freed = 0
    for directory in (slot/'debug', slot/'debug'/'deps', slot/'debug'/'examples'):
        for file in directory.iterdir() if directory.is_dir() else ():
            executable = file.is_file() and '.' not in file.name and os.access(file, os.X_OK)
            checked = file.suffix=='.rmeta' and not file.with_suffix('.rlib').exists()
            if executable or checked:
                freed += file.stat().st_blocks*512
                file.unlink()
    return freed

def guarded(label, command):
    log = Path(f'/home/russet/engine-w11-{label}.log')
    peak = 0
    with log.open('wb') as output:
        process = subprocess.Popen(command, cwd=source, env=environment, stdout=output, stderr=output, start_new_session=True)
        try:
            while process.poll() is None:
                filesystem = os.statvfs('/mnt/c')
                used = baseline_free-filesystem.f_bavail*filesystem.f_frsize
                seen = set()
                allocated = 0
                for file in slot.rglob('*'):
                    try:
                        if not file.is_file():
                            continue
                        info = file.stat()
                    except FileNotFoundError:
                        continue  # Cargo atomically replaces temporary artifacts.
                    identity = (info.st_dev, info.st_ino)
                    if identity not in seen:
                        seen.add(identity)
                        allocated += info.st_blocks*512
                peak = max(peak, allocated)
                if used>limit or allocated>6*1024**3:
                    raise RuntimeError(f'Owner storage guard stopped build: C delta {used}, target allocation {allocated}')
                print(json.dumps({'stage':label,'cBudgetBytesUsed':used,'targetAllocatedBytes':allocated}),flush=True)
                # Physical/runtime allocations can jump; do not leave a ten-second
                # blind interval. Logging stays sparse while storage checks are fast.
                for _ in range(50):
                    if process.poll() is not None:
                        break
                    filesystem = os.statvfs('/mnt/c')
                    if baseline_free-filesystem.f_bavail*filesystem.f_frsize>limit:
                        raise RuntimeError('Owner physical storage guard stopped build between progress samples')
                    time.sleep(.2)
            if process.returncode:
                raise RuntimeError(f'Engine {label} failed ({process.returncode}); inspect owned backend log')
            print(json.dumps({'stage':label,'result':'PASS','peakTargetAllocatedBytes':peak}),flush=True)
        finally:
            if process.poll() is None:
                os.killpg(process.pid,signal.SIGTERM)
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid,signal.SIGKILL)
                    process.wait(timeout=5)

if mode=='touched':
    # CI's nextest command for the crates an integration merge touched, in groups that
    # fit the slot cap: the sign-in crates, the debugger, then russetd in chunks.
    base = ['cargo','xtask','test','--locked','--profile','ci']
    daemon_tests = sorted(path.stem for path in source.glob('crates/russetd/tests/*.rs'))
    # One crate per group: six crates' test binaries together outgrow the cap. The sign-in
    # crates' tests need their `fake` features, which a workspace run gets by unification.
    groups = [(crate, ['-p', f'russet-{crate}', '--all-features']) for crate in
              ('gsa', 'anisette', 'adi-host', 'account', 'asc', 'protocol')]
    groups += [('debug', ['-p', 'russet-debug'])]
    groups += [('russetd-lib',['-p','russetd','--lib','--bins'])]
    groups += [(f'russetd-{chunk[0]}',['-p','russetd']+[arg for stem in chunk for arg in ('--test',stem)])
               for chunk in (daemon_tests[start:start+4] for start in range(0, len(daemon_tests), 4))]
    def make_room():
        # --all-features leaves a library variant per crate; past half the cap, start clean.
        freed = drop_reclaimable()
        if sum(f.stat().st_blocks*512 for f in slot.rglob('*') if f.is_file()) > 3*1024**3:
            shutil.rmtree(slot/'debug', ignore_errors=True)
            freed = -1
        return freed
    labels = [label for label, _ in groups]
    start = labels.index(sys.argv[2]) if len(sys.argv)>2 else 0
    print(json.dumps({'stage':'resume','stages':labels[start:],'freedBytes':make_room()}),flush=True)
    for label, selection in groups[start:]:
        guarded(f'touched-{label}', base+selection)
        print(json.dumps({'stage':label,'freedBytes':make_room()}),flush=True)
    print('PASS engine touched',flush=True)
elif mode=='ci-split':
    # Optional resume label: earlier stages already passed on this same commit.
    labels = [label for label, _ in split]
    start = labels.index(sys.argv[2]) if len(sys.argv)>2 else 0
    print(json.dumps({'integrationTargets':len(stems),'stages':labels[start:]}),flush=True)
    print(json.dumps({'stage':'resume','freedBytes':drop_reclaimable()}),flush=True)
    for label, command in split[start:]:
        guarded(label, command)
        print(json.dumps({'stage':label,'freedBytes':drop_reclaimable()}),flush=True)
    print('PASS engine ci-split',flush=True)
else:
    guarded(mode, commands[mode])
