"""Profile-guided release build of Ribbit (Windows; used by package-release.py).

Run:    python -I scripts/pgo_build.py
Result: target/release/ribbit-gui.exe and ribbit.exe (or CARGO_TARGET_DIR's
        release/), optimised with a
        profile of the emulator core running the user's own BIOS and game
        dumps (roms/, never committed). The core runs 10-16% faster per
        instruction (measured 2026-10-16: Pet Pals 10.6%, Cosmic Math 15.9%)
        with identical results (same state digests).

Steps: build the `bench` example instrumented (-Cprofile-generate), run it on
the training pairs below that exist, merge the profiles with llvm-profdata,
then build ribbit-desktop with -Cprofile-use. Without dumps or a matching
llvm-profdata (same LLVM major version as rustc: rustup's llvm-tools, PATH, or
C:/Program Files/LLVM) it says so and returns False; the caller then builds
normally.
"""
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# The final build goes to CARGO_TARGET_DIR (default target/), as cargo's would.
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
GEN = TARGET / 'pgo-gen'
DATA = TARGET / 'pgo-data'

# (BIOS, cartridge, steps): the Leapster 2 (two CPU-bound games), the
# original Leapster and the L-MAX, so every model's paths are profiled.
TRAINING = [
    ('roms/bios/leapster2/152-12206_0.6.bin', 'roms/Pet-Pals-Adopt-a-Playful-New-Learning-Pal_Leapster_EN.zip', 400_000_000),
    ('roms/bios/leapster2/152-12206_0.6.bin', 'roms/Cosmic-Math_Leapster_EN.zip', 400_000_000),
    ('roms/bios/152-11265_2.1.bin', 'roms/Sonic-X_Leapster_EN.zip', 400_000_000),
    ('roms/bios/leapsterlmx/152-11476_v2.2.bin', 'roms/Disney-Pixar-Cars-2_Leapster_EN.zip', 400_000_000),
]


def llvm_major(text):
    m = re.search(r'LLVM version:?\s*(\d+)', text)
    return m.group(1) if m else None


def find_profdata():
    rustc = subprocess.run(['rustc', '-vV'], capture_output=True, text=True, check=True).stdout
    want = llvm_major(rustc)
    host = re.search(r'^host:\s*(\S+)', rustc, re.M).group(1)
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True, check=True).stdout.strip()
    candidates = [Path(sysroot) / 'lib' / 'rustlib' / host / 'bin' / 'llvm-profdata.exe']
    if shutil.which('llvm-profdata'):
        candidates.append(Path(shutil.which('llvm-profdata')))
    candidates.append(Path('C:/Program Files/LLVM/bin/llvm-profdata.exe'))
    for c in candidates:
        if c.exists():
            out = subprocess.run([str(c), '--version'], capture_output=True, text=True).stdout
            if llvm_major(out) == want:
                return c
            print(f'pgo: {c} is LLVM {llvm_major(out)}, rustc uses {want}; skipped')
    return None


def build_pgo():
    profdata = find_profdata()
    if profdata is None:
        print('pgo: no llvm-profdata matching rustc (rustup component add llvm-tools); normal build')
        return False
    pairs = [(b, c, n) for b, c, n in TRAINING if (ROOT / b).exists() and (ROOT / c).exists()]
    if not pairs:
        print('pgo: none of the training BIOS/game dumps are in roms/; normal build')
        return False
    if DATA.exists():
        shutil.rmtree(DATA)
    DATA.mkdir(parents=True)
    env = dict(os.environ, CARGO_TARGET_DIR=str(GEN), RUSTFLAGS=f'-Cprofile-generate={DATA.as_posix()}')
    subprocess.run(['cargo', 'build', '--release', '-p', 'ribbit-core', '--example', 'bench'], cwd=ROOT, env=env, check=True)
    bench = GEN / 'release' / 'examples' / 'bench.exe'
    for bios, cart, steps in pairs:
        print(f'pgo: training on {Path(bios).name} + {Path(cart).name}')
        run_env = dict(os.environ, RIBBIT_CART=str(ROOT / cart), RIBBIT_NO_TAIL='1', RIBBIT_ANY_MODEL='1')
        subprocess.run([str(bench), str(ROOT / bios), str(steps)], cwd=ROOT, env=run_env, check=True, stdout=subprocess.DEVNULL)
    merged = DATA / 'merged.profdata'
    subprocess.run([str(profdata), 'merge', '-o', str(merged), *map(str, DATA.glob('*.profraw'))], check=True)
    env = dict(os.environ, RUSTFLAGS=f'-Cprofile-use={merged.as_posix()}')
    subprocess.run(['cargo', 'build', '--release', '-p', 'ribbit-desktop'], cwd=ROOT, env=env, check=True)
    print(f'pgo: optimised build in {TARGET / "release"}')
    return True


if __name__ == '__main__':
    sys.exit(0 if build_pgo() else 1)
