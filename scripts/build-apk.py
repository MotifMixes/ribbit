"""Builds a debug-signed Ribbit APK (arm64-v8a) without Gradle.

Needs: Android SDK (build-tools, platform), NDK, cargo-ndk, Java (keytool).
Run:   python -I scripts/build-apk.py [--sdk DIR] [--ndk DIR]
Output: target/android/ribbit-debug.apk, and the release copy
        target/dist/Ribbit-<version>-android-arm64.apk

Steps: cargo-ndk builds libribbit.so; the frog launcher icon is generated at
each density (by the launcher_icons example, from the same frog as the desktop); aapt2 compiles resources and links the manifest; the native
library is added; zipalign; apksigner with a debug key kept in local/ (so
updates install over the previous build).
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
OUT = ROOT / 'target' / 'android'
PACKAGE_DIR = ROOT / 'crates' / 'ribbit-android' / 'android'
KEYSTORE = ROOT / 'local' / 'android-debug.keystore'

def version():
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    m = re.search(r'^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"', text, re.M)
    if not m:
        sys.exit('no workspace version in Cargo.toml')
    major, minor, patch = map(int, m.groups())
    return f'{major}.{minor}.{patch}', major * 10000 + minor * 100 + patch


def release_rustflags():
    """CARGO_ENCODED_RUSTFLAGS without the build machine's home folder or
    the repository's place on it in the library (rustc keeps source paths
    for panic messages; they would show the account name)."""
    home = Path.home().resolve()
    return {'CARGO_ENCODED_RUSTFLAGS': '\x1f'.join([f'--remap-path-prefix={home}=~', f'--remap-path-prefix={ROOT}=ribbit'])}


def personal_info(apk):
    """Entries of `apk` holding the build machine's home folder, any user
    folder path or the git email (the names are found here, never stored)."""
    home = str(Path.home().resolve())
    needles = {home, home.replace('\\', '/'), 'C:\\Users\\', '/Users/', '/home/'}
    email = subprocess.run(['git', 'config', 'user.email'], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    if email and not email.endswith('@users.noreply.github.com'):
        needles.add(email)
    with zipfile.ZipFile(apk) as z:
        return [(i.filename, n) for i in z.infolist() for n in sorted(needles) if n.encode() in z.read(i)]


def run(cmd, **kw):
    print('>', ' '.join(str(c) for c in cmd))
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def latest(dir_: Path) -> Path:
    versions = sorted((p for p in dir_.iterdir() if p.is_dir()), key=lambda p: [int(x) if x.isdigit() else x for x in p.name.replace('-', '.').split('.')])
    if not versions:
        sys.exit(f'nothing installed in {dir_}')
    return versions[-1]


def main():
    ap = argparse.ArgumentParser()
    default_sdk = Path(os.environ.get('ANDROID_HOME') or Path(os.environ['LOCALAPPDATA']) / 'Android' / 'Sdk')
    ap.add_argument('--sdk', type=Path, default=default_sdk)
    ap.add_argument('--ndk', type=Path)
    ap.add_argument('--skip-build', action='store_true')
    a = ap.parse_args()
    ver, code = version()
    sdk = a.sdk
    ndk = a.ndk or latest(sdk / 'ndk')
    tools = latest(sdk / 'build-tools')
    platform = latest(sdk / 'platforms')
    exe = '.exe' if os.name == 'nt' else ''
    bat = '.bat' if os.name == 'nt' else ''
    OUT.mkdir(parents=True, exist_ok=True)

    # 1. Native library.
    jni_libs = OUT / 'jniLibs'
    if not a.skip_build:
        env = dict(os.environ, ANDROID_NDK_HOME=str(ndk), ANDROID_HOME=str(sdk), **release_rustflags())
        run(['cargo', 'ndk', '-t', 'arm64-v8a', '-P', '26', '-o', jni_libs, 'build', '--release', '-p', 'ribbit-android'], cwd=ROOT, env=env)
    lib = jni_libs / 'arm64-v8a' / 'libribbit.so'
    if not lib.exists():
        sys.exit(f'missing {lib}')

    # 2. Launcher icons.
    res = OUT / 'res'
    run(['cargo', 'run', '-q', '--release', '-p', 'ribbit-frontend', '--example', 'launcher_icons', '--', res], cwd=ROOT)

    # 3. Resources + manifest.
    flat = OUT / 'res.zip'
    run([tools / f'aapt2{exe}', 'compile', '--dir', res, '-o', flat])
    unsigned = OUT / 'ribbit-unaligned.apk'
    run([tools / f'aapt2{exe}', 'link', '-o', unsigned, '-I', platform / 'android.jar', '--manifest', PACKAGE_DIR / 'AndroidManifest.xml',
         '--min-sdk-version', '26', '--target-sdk-version', '36', '--version-name', ver, '--version-code', str(code), '--replace-version', flat])

    # 4. Native library into the APK.
    # (A UTC time: zip stores local times, which would give away the zone.)
    with zipfile.ZipFile(unsigned, 'a', compression=zipfile.ZIP_DEFLATED) as z:
        info = zipfile.ZipInfo('lib/arm64-v8a/libribbit.so', date_time=time.gmtime(lib.stat().st_mtime)[:6])
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, lib.read_bytes())

    # 5. Align and sign.
    aligned = OUT / 'ribbit-aligned.apk'
    run([tools / f'zipalign{exe}', '-p', '-f', '4', unsigned, aligned])
    if not KEYSTORE.exists():
        KEYSTORE.parent.mkdir(parents=True, exist_ok=True)
        run(['keytool', '-genkeypair', '-keystore', KEYSTORE, '-alias', 'ribbitdebug', '-storepass', 'android', '-keypass', 'android',
             '-dname', 'CN=Ribbit Debug,O=Ribbit,C=US', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000'])
    final = OUT / 'ribbit-debug.apk'
    run([tools / f'apksigner{bat}', 'sign', '--ks', KEYSTORE, '--ks-pass', 'pass:android', '--key-pass', 'pass:android', '--out', final, aligned],
        shell=os.name == 'nt')
    run([tools / f'apksigner{bat}', 'verify', final], shell=os.name == 'nt')
    found = personal_info(final)
    if found:
        for f, n in found:
            print(f'  {f}: contains {n!r}')
        sys.exit('the APK holds personal information (paths or email); no release copy made')
    dist = ROOT / 'target' / 'dist' / f'Ribbit-{ver}-android-arm64.apk'
    dist.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(final, dist)
    print(f'\nAPK: {final} ({final.stat().st_size // 1024} KiB)\nRelease copy: {dist}')


if __name__ == '__main__':
    main()
