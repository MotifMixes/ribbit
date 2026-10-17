"""Builds a Linux (x86-64) release of Ribbit, and optionally uploads it.

Run on Linux:   python3 -I scripts/package-linux.py [--skip-build] [--upload]
Run on Windows: python -I scripts/package-linux.py [--skip-build] [--upload]
        (builds inside WSL, Ubuntu-22.04 by default or --distro; uploads
        with the Windows gh)
Output: target/dist/Ribbit-<version>-linux-x64.tar.gz
        (ribbit-gui, the `ribbit` command-line tool, a .desktop file, a
        read-me and the third-party licence notices)

Needs on the Linux side: Rust, pkg-config, libasound2-dev and libudev-dev.
Built on Ubuntu 22.04, the programs run on distributions with glibc 2.35 or
later. --upload attaches the archive to the GitHub release v<version> (gh
CLI, logged in); the release must exist already. An archive already on the
release with the same name is replaced.

When the repository is on a Windows drive (/mnt/c/...), the build goes to
~/.cache/ribbit/target-linux in the Linux home folder (much faster than the
Windows drive, and kept apart from the Windows build); RIBBIT_LINUX_TARGET
overrides it.

As with the Windows package, the archive is assembled in a fresh staging
folder from named files, so no BIOS, ROM, save or other local file can end
up in it.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / 'target' / 'dist'
PROGRAMS = ['ribbit-gui', 'ribbit']

DESKTOP = """[Desktop Entry]
Type=Application
Name=Ribbit
Comment=Emulator for LeapFrog handhelds and TV consoles
Exec=ribbit-gui
Terminal=false
Categories=Game;Emulator;
"""

README = """Ribbit {version} for Linux - an emulator for the LeapFrog Leapster (and the
Zippity and RockIt Twist)
=====================================================================

Ribbit includes no firmware and no games. You need a dump of the BIOS from
your own Leapster and dumps of your own cartridges (and, for the other
consoles, their own system files).

Linux support is new and less tested than Windows.

Running
-------
  ./ribbit-gui

Needs ALSA (libasound2) and libudev, present on almost every desktop
distribution, and a Vulkan or OpenGL driver. Works on X11 and Wayland.
To add it to your application menu, copy ribbit-gui somewhere on your PATH
(e.g. ~/.local/bin) and ribbit.desktop to ~/.local/share/applications.

Controllers: if a controller is not found, your user may need read access
to /dev/input/event* (most distributions give it to the logged-in user).

Starting
--------
1. File > Open BIOS... and choose your BIOS dump (asked once; remembered).
2. Add your games folder. The game library shows every game in it; click a
   game to play. File > Open Cartridge... (or dropping a file on the
   window) also works.

Controls (Input > Key bindings... to change)
--------------------------------------------
  Arrows .......... D-pad          Z / X ......... A / B
  H ............... Hint           Enter ......... Pause (Leapster button)
  Backspace ....... Home           Mouse ......... stylus (experimental)
  Esc ............. pause          Tab (hold) .... fast-forward
  ` (hold) ........ rewind         F5 / F9 ....... quick save / load state
  F11 ............. fullscreen     F12 ........... screenshot
  Help > Controls (F1) lists the keys of the Zippity and the RockIt Twist.

Where things go
---------------
Settings, game saves, save states and screenshots: ~/.config/ribbit
(or $XDG_CONFIG_HOME/ribbit; Help > About shows the save folder.)

`ribbit` is the command-line tool (./ribbit --help).
Help > Check for updates tells you when a new version is out.
"""


def version():
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        sys.exit('no workspace version in Cargo.toml')
    return m.group(1)


def run(*cmd, **kw):
    print('+', ' '.join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def upload(ver, out):
    run('gh', 'release', 'upload', f'v{ver}', out, '--clobber', cwd=ROOT)
    print(f'uploaded to release v{ver}')


def from_windows(a, ver):
    """Runs this script inside WSL, then uploads from Windows."""
    script = subprocess.run(
        ['wsl', '-d', a.distro, '--', 'wslpath', '-a', str(Path(__file__).resolve()).replace('\\', '/')],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    args = ['--skip-build'] if a.skip_build else []
    # A login shell, so the Linux user's Rust (~/.cargo/bin) is on PATH.
    run('wsl', '-d', a.distro, '--', 'bash', '-lc', ' '.join(['python3', '-I', f"'{script}'", *args]))
    out = DIST / f'Ribbit-{ver}-linux-x64.tar.gz'
    if not out.exists():
        sys.exit(f'missing {out}')
    if a.upload:
        upload(ver, out)


def release_rustflags(*extra):
    """Compiler flags for release builds, as CARGO_ENCODED_RUSTFLAGS (paths
    with spaces stay whole): the build machine's home folder and the
    repository's place on it are not kept in the programs (rustc keeps
    source paths for panic messages; they would show the account name).
    When several prefixes match, rustc uses the last one."""
    home = Path.home().resolve()
    flags = [f'--remap-path-prefix={home}=~', f'--remap-path-prefix={ROOT}=ribbit', *extra]
    return {'CARGO_ENCODED_RUSTFLAGS': ''.join(flags)}


def target_dir():
    if os.environ.get('RIBBIT_LINUX_TARGET'):
        return Path(os.environ['RIBBIT_LINUX_TARGET'])
    if str(ROOT).startswith('/mnt/'):
        return Path.home() / '.cache' / 'ribbit' / 'target-linux'
    return ROOT / 'target' / 'linux'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--skip-build', action='store_true')
    ap.add_argument('--upload', action='store_true', help='attach the archive to the GitHub release v<version>')
    ap.add_argument('--distro', default='Ubuntu-22.04', help='WSL distribution (when run on Windows)')
    a = ap.parse_args()
    ver = version()
    if sys.platform == 'win32':
        return from_windows(a, ver)
    if not sys.platform.startswith('linux'):
        sys.exit('this script builds the Linux programs and must run on Linux (or Windows with WSL)')

    tdir = target_dir()
    if not a.skip_build:
        env = dict(os.environ, **release_rustflags())
        run('cargo', 'build', '--release', '-p', 'ribbit-desktop', '--target-dir', tdir, cwd=ROOT, env=env)

    name = f'Ribbit-{ver}-linux-x64'
    stage = DIST / name
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    for prog in PROGRAMS:
        src = tdir / 'release' / prog
        if not src.exists():
            sys.exit(f'missing {src} (build first)')
        shutil.copy2(src, stage / prog)
        (stage / prog).chmod(0o755)
    (stage / 'ribbit.desktop').write_text(DESKTOP, encoding='utf-8')
    (stage / 'README.txt').write_text(README.format(version=ver), encoding='utf-8')
    shutil.copy2(ROOT / 'THIRD-PARTY-NOTICES.txt', stage / 'THIRD-PARTY-NOTICES.txt')

    allowed = set(PROGRAMS) | {'ribbit.desktop', 'README.txt', 'THIRD-PARTY-NOTICES.txt'}
    found = {p.name for p in stage.iterdir()}
    if found != allowed:
        sys.exit(f'unexpected files in the package: {sorted(found - allowed)}')

    out = DIST / f'{name}.tar.gz'
    with tarfile.open(out, 'w:gz', compresslevel=9) as tar:
        for f in sorted(stage.iterdir()):
            tar.add(f, f'{name}/{f.name}')
    print(f'{out} ({out.stat().st_size // 1024} KiB)')
    if a.upload:
        if shutil.which('gh') is None:
            sys.exit('gh is not installed here; upload the archive with gh release upload from another machine')
        upload(ver, out)


if __name__ == '__main__':
    main()
