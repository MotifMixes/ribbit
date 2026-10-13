"""Builds a debug-signed Ribbit APK (arm64-v8a) without Gradle.

Needs: Android SDK (build-tools, platform), NDK, cargo-ndk, Java (keytool).
Run:   python -I scripts/build-apk.py [--sdk DIR] [--ndk DIR]
Output: target/android/ribbit-debug.apk

Steps: cargo-ndk builds libribbit.so; the frog launcher icon is generated at
each density; aapt2 compiles resources and links the manifest; the native
library is added; zipalign; apksigner with a debug key kept in local/ (so
updates install over the previous build).
"""
import argparse
import os
import struct
import subprocess
import sys
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'target' / 'android'
PACKAGE_DIR = ROOT / 'crates' / 'ribbit-android' / 'android'
FROG = ROOT / 'crates' / 'ribbit-frontend' / 'src' / 'frog.txt'
KEYSTORE = ROOT / 'local' / 'android-debug.keystore'

PALETTE = {
    'g': (0x6C, 0xCB, 0x3F), 'G': (0x2F, 0x7D, 0x1E), 'k': (0x14, 0x14, 0x14), 'w': (0xFF, 0xFF, 0xFF),
    'o': (0xFF, 0x98, 0x00), 'y': (0xF1, 0xF8, 0xC8), 'b': (0x3F, 0x7F, 0xD9), 'p': (0xFF, 0x8F, 0xB1),
}
DENSITIES = {'mdpi': 48, 'hdpi': 72, 'xhdpi': 96, 'xxhdpi': 144, 'xxxhdpi': 192}


def run(cmd, **kw):
    print('>', ' '.join(str(c) for c in cmd))
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def png_rgba(size, rows):
    n = len(rows)
    raw = bytearray()
    for y in range(size):
        raw.append(0)
        row = rows[y * n // size]
        for x in range(size):
            c = row[x * n // size]
            raw.extend(PALETTE[c] + (255,) if c in PALETTE else (0, 0, 0, 0))

    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(raw), 9)) + chunk(b'IEND', b''))


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
        env = dict(os.environ, ANDROID_NDK_HOME=str(ndk), ANDROID_HOME=str(sdk))
        run(['cargo', 'ndk', '-t', 'arm64-v8a', '-P', '26', '-o', jni_libs, 'build', '--release', '-p', 'ribbit-android'], cwd=ROOT, env=env)
    lib = jni_libs / 'arm64-v8a' / 'libribbit.so'
    if not lib.exists():
        sys.exit(f'missing {lib}')

    # 2. Launcher icons.
    rows = FROG.read_text().splitlines()
    res = OUT / 'res'
    for name, size in DENSITIES.items():
        d = res / f'mipmap-{name}'
        d.mkdir(parents=True, exist_ok=True)
        (d / 'ic_launcher.png').write_bytes(png_rgba(size, rows))

    # 3. Resources + manifest.
    flat = OUT / 'res.zip'
    run([tools / f'aapt2{exe}', 'compile', '--dir', res, '-o', flat])
    unsigned = OUT / 'ribbit-unaligned.apk'
    run([tools / f'aapt2{exe}', 'link', '-o', unsigned, '-I', platform / 'android.jar', '--manifest', PACKAGE_DIR / 'AndroidManifest.xml',
         '--min-sdk-version', '26', '--target-sdk-version', '36', flat])

    # 4. Native library into the APK.
    with zipfile.ZipFile(unsigned, 'a', compression=zipfile.ZIP_DEFLATED) as z:
        z.write(lib, 'lib/arm64-v8a/libribbit.so')

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
    print(f'\nAPK: {final} ({final.stat().st_size // 1024} KiB)')


if __name__ == '__main__':
    main()
