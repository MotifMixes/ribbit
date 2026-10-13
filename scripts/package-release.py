"""Builds a Windows release zip of Ribbit.

Run:    python -I scripts/package-release.py [--skip-build]
Output: target/dist/Ribbit-<version>-windows-x64.zip

The zip holds only the two programs and a short read-me written here. It is
assembled in a fresh staging folder from named files, so no BIOS, ROM, save
or other local file can end up inside; the script also refuses any file that
is not on its list.
"""
import argparse
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / 'target' / 'dist'
PROGRAMS = ['ribbit-gui.exe', 'ribbit.exe']

README = """Ribbit {version} - an emulator for the original 2003 LeapFrog Leapster
=====================================================================

Ribbit includes no firmware and no games. You need a dump of the BIOS from
your own Leapster and dumps of your own cartridges.

Starting
--------
1. Run ribbit-gui.exe.
2. File > Open BIOS... and choose your BIOS dump (asked once; remembered).
3. File > Open Cartridge... (or drop a .bin onto the window).

Known BIOS dumps: US v2.1 (152-11265) works best. The Universal v1.5 BIOS
(155-10072) turns itself off after start-up unless you enable
Emulation > Advanced > "MAME power-line workaround" (an unverified guess).

Controls (Input > Key bindings... to change)
--------------------------------------------
  Arrows .......... D-pad          Z / X ......... A / B
  H ............... Hint           Enter ......... Pause (Leapster button)
  Backspace ....... Home           Mouse ......... stylus (experimental)
  Esc ............. pause          Tab (hold) .... fast-forward
  ` (hold) ........ rewind         F5 / F9 ....... quick save / load state
  F11 ............. fullscreen     F12 ........... screenshot
  Controllers (XInput etc.): D-pad/stick, A, B, X/Y = Hint, Start = Pause,
  Back = Home, LB/RB = volume, Guide = menu.

Where things go
---------------
Settings, game saves, save states and screenshots: %APPDATA%\\Ribbit
(Help > About shows the save folder.)

Accuracy
--------
Ribbit aims to be accurate before it is complete. Parts that follow MAME's
research rather than hardware measurements (sound voices, the touchscreen,
the power-line workaround) are labelled experimental in the menus; see
Help > Emulation status. ribbit.exe is the command-line tool (ribbit --help).
"""


def version():
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        sys.exit('no workspace version in Cargo.toml')
    return m.group(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--skip-build', action='store_true')
    a = ap.parse_args()
    ver = version()
    if not a.skip_build:
        subprocess.run(['cargo', 'build', '--release', '-p', 'ribbit-desktop'], cwd=ROOT, check=True)
    name = f'Ribbit-{ver}-windows-x64'
    stage = DIST / name
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    for exe in PROGRAMS:
        src = ROOT / 'target' / 'release' / exe
        if not src.exists():
            sys.exit(f'missing {src} (build first)')
        shutil.copy2(src, stage / exe)
    (stage / 'README.txt').write_bytes(README.format(version=ver).replace('\n', '\r\n').encode('utf-8'))

    allowed = set(PROGRAMS) | {'README.txt'}
    found = {p.name for p in stage.iterdir()}
    if found != allowed:
        sys.exit(f'unexpected files in the package: {sorted(found - allowed)}')

    out = DIST / f'{name}.zip'
    with zipfile.ZipFile(out, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in sorted(stage.iterdir()):
            z.write(f, f'{name}/{f.name}')
    print(f'{out} ({out.stat().st_size // 1024} KiB)')


if __name__ == '__main__':
    main()
