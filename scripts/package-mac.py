"""Builds a macOS release of Ribbit on a Mac, and optionally uploads it.

Run (on the Mac, from the repository):
        python3 -I scripts/package-mac.py [--skip-build] [--arch universal|arm64|x86_64] [--upload]
Output: target/dist/Ribbit-<version>-macos-<arch>.zip
        (Ribbit.app, the `ribbit` command-line tool, a read-me and the
        third-party licence notices)

Needs: Rust (rustup), the Xcode command-line tools (xcode-select --install).
A universal build adds both targets itself (rustup target add ...).
--upload attaches the zip to the GitHub release v<version> (gh CLI, logged
in with `gh auth login`); the release must exist already, e.g. made on the
Windows PC with `gh release create`. A zip already on the release with the
same name is replaced.

The app is signed ad hoc only (no Apple Developer ID, not notarised), so
macOS asks once before opening it; the read-me says how.

As with the Windows package, the zip is assembled in a fresh staging folder
from named files, so no BIOS, ROM, save or other local file can end up in it.
"""
import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / 'target' / 'dist'
TARGETS = {'arm64': 'aarch64-apple-darwin', 'x86_64': 'x86_64-apple-darwin'}
PROGRAMS = ['ribbit-gui', 'ribbit']
BUNDLE_ID = 'io.github.motifmixes.ribbit'

INFO_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Ribbit</string>
  <key>CFBundleDisplayName</key><string>Ribbit</string>
  <key>CFBundleIdentifier</key><string>{bundle_id}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleExecutable</key><string>ribbit-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"""

README = """Ribbit {version} for macOS - an emulator for the LeapFrog Leapster (and the
Zippity and RockIt Twist)
=====================================================================

Ribbit includes no firmware and no games. You need a dump of the BIOS from
your own Leapster and dumps of your own cartridges (and, for the other
consoles, their own system files).

macOS support is new and less tested than Windows.

Opening it the first time
-------------------------
Ribbit is not signed with an Apple Developer ID, so macOS stops it the first
time. Drag Ribbit.app to Applications, then right-click (or Control-click)
it and choose Open, then Open again. If macOS says the app "is damaged",
run this once in Terminal:

    xattr -dr com.apple.quarantine /Applications/Ribbit.app

Starting
--------
1. Open Ribbit.
2. File > Open BIOS... and choose your BIOS dump (asked once; remembered).
3. Add your games folder. The game library shows every game in it; click a
   game to play. File > Open Cartridge... (or dropping a file on the
   window) also works.

Controls (Input > Key bindings... to change)
--------------------------------------------
  Arrows .......... D-pad          Z / X ......... A / B
  H ............... Hint           Return ........ Pause (Leapster button)
  Delete .......... Home           Mouse ......... stylus (experimental)
  Esc ............. pause          Tab (hold) .... fast-forward
  ` (hold) ........ rewind         F5 / F9 ....... quick save / load state
  F11 ............. fullscreen     F12 ........... screenshot
  (On a Mac keyboard, hold fn for the F keys.)
  Help > Controls (F1) lists the keys of the Zippity and the RockIt Twist.

Where things go
---------------
Settings, game saves, save states and screenshots: ~/.config/ribbit
(Help > About shows the save folder.)

`ribbit` next to the app is the command-line tool (./ribbit --help).
Help > Check for updates tells you when a new version is out.
"""


def version():
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        sys.exit('no workspace version in Cargo.toml')
    return m.group(1)


def run(*cmd, env=None):
    print('+', ' '.join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], cwd=ROOT, check=True, env=env)


def release_rustflags(*extra):
    """Compiler flags for release builds, as CARGO_ENCODED_RUSTFLAGS (paths
    with spaces stay whole): the build machine's home folder and the
    repository's place on it are not kept in the programs (rustc keeps
    source paths for panic messages; they would show the account name).
    When several prefixes match, rustc uses the last one."""
    home = Path.home().resolve()
    flags = [f'--remap-path-prefix={home}=~', f'--remap-path-prefix={ROOT}=ribbit', *extra]
    return {'CARGO_ENCODED_RUSTFLAGS': ''.join(flags)}


def build(arches):
    for arch in arches:
        target = TARGETS[arch]
        run('rustup', 'target', 'add', target)
        run('cargo', 'build', '--release', '-p', 'ribbit-desktop', '--target', target,
            env=dict(os.environ, **release_rustflags()))


def program(arches, name, out):
    """One program: copied for one architecture, merged with lipo for two."""
    parts = [ROOT / 'target' / TARGETS[a] / 'release' / name for a in arches]
    for p in parts:
        if not p.exists():
            sys.exit(f'missing {p} (build first)')
    if len(parts) == 1:
        shutil.copy2(parts[0], out)
    else:
        run('lipo', '-create', '-output', out, *parts)
    out.chmod(0o755)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--skip-build', action='store_true')
    ap.add_argument('--arch', choices=['universal', 'arm64', 'x86_64'], default='universal')
    ap.add_argument('--upload', action='store_true', help='attach the zip to the GitHub release v<version>')
    a = ap.parse_args()
    if platform.system() != 'Darwin':
        sys.exit('this script builds the macOS app and must run on a Mac')
    ver = version()
    arches = list(TARGETS) if a.arch == 'universal' else [a.arch]
    if not a.skip_build:
        build(arches)

    name = f'Ribbit-{ver}-macos-{a.arch}'
    stage = DIST / name
    if stage.exists():
        shutil.rmtree(stage)
    app = stage / 'Ribbit.app' / 'Contents'
    (app / 'MacOS').mkdir(parents=True)
    (app / 'Resources').mkdir()
    program(arches, 'ribbit-gui', app / 'MacOS' / 'ribbit-gui')
    (app / 'Info.plist').write_text(INFO_PLIST.format(version=ver, bundle_id=BUNDLE_ID), encoding='utf-8')
    program(arches, 'ribbit', stage / 'ribbit')
    (stage / 'README.txt').write_text(README.format(version=ver), encoding='utf-8')
    shutil.copy2(ROOT / 'THIRD-PARTY-NOTICES.txt', stage / 'THIRD-PARTY-NOTICES.txt')

    allowed = {'Ribbit.app', 'ribbit', 'README.txt', 'THIRD-PARTY-NOTICES.txt'}
    found = {p.name for p in stage.iterdir()}
    if found != allowed:
        sys.exit(f'unexpected files in the package: {sorted(found - allowed)}')
    inside = {str(p.relative_to(stage)) for p in (stage / 'Ribbit.app').rglob('*') if p.is_file()}
    if inside != {'Ribbit.app/Contents/Info.plist', 'Ribbit.app/Contents/MacOS/ribbit-gui'}:
        sys.exit(f'unexpected files in the app: {sorted(inside)}')

    # Ad-hoc signatures: Apple silicon refuses to run unsigned code at all.
    run('codesign', '--force', '--sign', '-', stage / 'ribbit')
    run('codesign', '--force', '--deep', '--sign', '-', stage / 'Ribbit.app')

    out = DIST / f'{name}.zip'
    if out.exists():
        out.unlink()
    # ditto keeps the bundle's permissions and signature intact (zipfile does not).
    run('ditto', '-c', '-k', '--keepParent', stage, out)
    print(f'{out} ({out.stat().st_size // 1024} KiB)')

    if a.upload:
        run('gh', 'release', 'upload', f'v{ver}', out, '--clobber')
        print(f'uploaded to release v{ver}')


if __name__ == '__main__':
    main()
