"""Builds a Windows release zip of Ribbit.

Run:    python -I scripts/package-release.py [--skip-build] [--no-pgo]
Output: target/dist/Ribbit-<version>-windows-x64.zip

The build is profile-guided when your BIOS and game dumps are in roms/
(scripts/pgo_build.py: 10-16% faster emulation, identical results).

The zip holds only the two programs, a short read-me written here and the
third-party licence notices. It is
assembled in a fresh staging folder from named files, so no BIOS, ROM, save
or other local file can end up inside; the script also refuses any file that
is not on its list.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / 'target' / 'dist'
PROGRAMS = ['ribbit-gui.exe', 'ribbit.exe']

README = """Ribbit {version} - an emulator for the LeapFrog Leapster (and the Zippity
and RockIt Twist)
=====================================================================

Ribbit includes no firmware and no games. You need a dump of the BIOS from
your own Leapster and dumps of your own cartridges (and, for the other
consoles, their own system files).

Starting
--------
1. Run ribbit-gui.exe.
2. File > Open BIOS... and choose your BIOS dump (asked once; remembered).
3. Choose your games folder. The game library shows every Leapster game in
   it (plain or zipped dumps) with its title screen; click a game to play.
   File > Open Cartridge... (or dropping a file on the window) also works.

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

Picture
-------
Pixel perfect is the default. Video > AI upscale (HD) runs the ArtCNN
neural network on the 160x160 picture (Video > AI upscale settings).
Window size: Fit screen (default), 2x, 3x, 4x or 16x.

Other consoles
--------------
Choose the console in the library's console menu.
  * LeapFrog Zippity (TV console): needs a dump of its system NAND; its
    built-in games and cartridges play.
  * LeapFrog RockIt Twist (early emulation): needs its system packages
    (Bootloader, Base, Firmware-Bulk) and content unpacked from your own
    packs (File > Import RockIt Twist games). The menu, the built-in games
    (Nom-Nom tested), the Sneak Peeks videos and sound work; the console
    keeps its profiles and progress in the save folder. Encrypted .lf3 packs
    are not run. Keys: WASD/arrows = D-pad, Q/E = Switch, number pad = button
    grid, U/J I/K O/L = dials, number pad * and - = spinner, Page Up/Down =
    slider, Enter = check mark, Home = Home, Shift/Tab = Hint.

Discord
-------
While you play, Discord shows the game's name (File > Show my game on
Discord to turn it off).

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


def personal_info(stage):
    """Files in `stage` holding the build machine's home folder, any user
    folder path or the git email: (file, what) pairs. A release must have
    none (the names are found on the machine running this, never stored)."""
    home = str(Path.home().resolve())
    needles = {home, home.replace('\\', '/'), 'C:\\Users\\', '/Users/', '/home/'}
    email = subprocess.run(['git', 'config', 'user.email'], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    if email and not email.endswith('@users.noreply.github.com'):
        needles.add(email)
    found = []
    for f in sorted(p for p in Path(stage).rglob('*') if p.is_file()):
        data = f.read_bytes()
        found += [(f.relative_to(stage), n) for n in sorted(needles) if n.encode() in data]
    return found


def refuse_personal_info(stage):
    found = personal_info(stage)
    if found:
        for f, n in found:
            print(f'  {f}: contains {n!r}')
        sys.exit('the package holds personal information (paths or email); not packaged')


def version():
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        sys.exit('no workspace version in Cargo.toml')
    return m.group(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--skip-build', action='store_true')
    ap.add_argument('--no-pgo', action='store_true', help='plain build, no profile-guided optimisation')
    a = ap.parse_args()
    ver = version()
    if not a.skip_build:
        sys.path.insert(0, str(ROOT / 'scripts'))
        import pgo_build
        if a.no_pgo or not pgo_build.build_pgo():
            env = dict(os.environ, **pgo_build.release_rustflags())
            subprocess.run(['cargo', 'build', '--release', '-p', 'ribbit-desktop'], cwd=ROOT, env=env, check=True)
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

    shutil.copy2(ROOT / 'THIRD-PARTY-NOTICES.txt', stage / 'THIRD-PARTY-NOTICES.txt')

    allowed = set(PROGRAMS) | {'README.txt', 'THIRD-PARTY-NOTICES.txt'}
    found = {p.name for p in stage.iterdir()}
    if found != allowed:
        sys.exit(f'unexpected files in the package: {sorted(found - allowed)}')

    refuse_personal_info(stage)

    out = DIST / f'{name}.zip'
    with zipfile.ZipFile(out, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in sorted(stage.iterdir()):
            # Times in UTC: zip stores the zipping PC's local time, which
            # would give away its time zone.
            info = zipfile.ZipInfo(f'{name}/{f.name}', date_time=time.gmtime(f.stat().st_mtime)[:6])
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (f.stat().st_mode & 0xFFFF) << 16
            z.writestr(info, f.read_bytes(), compresslevel=9)
    print(f'{out} ({out.stat().st_size // 1024} KiB)')


if __name__ == '__main__':
    main()
