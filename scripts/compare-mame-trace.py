"""Differential control-flow check: Ribbit vs. MAME on the same BIOS.

Usage (from the repository root; traces go to tools/analysis/, git-ignored):
    python scripts/compare-mame-trace.py --mame tools/mame0289/mame.exe \
        --rompath . --bios uni15 --firmware roms/bios/155-10072-a.bin --stop 40073d74

MAME's trace folds loops ("(loops for N instructions)") and the two emulators
model timers differently (MAME 0.289 returns a constant from 0x0180D800), so
the comparison is the *first-visit order* of PCs: identical control flow
gives identical order regardless of how long each loop spins. Registers that
neither emulator models are stubbed in Ribbit with MAME 0.289's constants
(--mmio-stub; labelled "debug stub" in Ribbit's trace) so both follow the
same path. This compares CPU behaviour, not peripheral accuracy.
"""

import argparse
import os
import re
import subprocess

# MAME 0.289 leapster.cpp read handlers for registers Ribbit does not model.
MAME_0289_STUBS = ["0x01809004=0x00200000", "0x0180b004=0xffffffff", "0x0180b008=1", "0x0180d514=0x80"]


def firsts(path, pattern):
    seen, order = set(), []
    rx = re.compile(pattern)
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = rx.match(line)
            if m:
                pc = int(m.group(1), 16)
                if pc not in seen:
                    seen.add(pc)
                    order.append(pc)
    return order


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mame", required=True)
    ap.add_argument("--rompath", required=True, help="folder containing MAME's leapster.zip")
    ap.add_argument("--bios", default="uni15")
    ap.add_argument("--firmware", required=True)
    ap.add_argument("--stop", required=True, help="hex PC at which both runs stop")
    ap.add_argument("--ribbit", default=os.path.join("target", "release", "ribbit.exe" if os.name == "nt" else "ribbit"))
    args = ap.parse_args()
    args.mame = os.path.abspath(args.mame)

    out = os.path.abspath(os.path.join("tools", "analysis"))
    os.makedirs(out, exist_ok=True)
    mame_trace = os.path.join(out, "mame.trace").replace("\\", "/")
    ribbit_trace = os.path.join(out, "ribbit.trace")
    script = os.path.join(out, "mame_trace.cmd")
    with open(script, "w") as f:
        f.write(f"trace {mame_trace},maincpu\nbpset {args.stop}\ngo\ntraceflush\ntrace off\nquit\n")
    subprocess.run(
        [args.mame, "leapster", "-bios", args.bios, "-rompath", os.path.abspath(args.rompath), "-debug",
         "-debugger", "windows", "-debugscript", script, "-video", "none", "-sound", "none", "-nothrottle",
         "-seconds_to_run", "3", "-skip_gameinfo"],
        check=True, capture_output=True, cwd=os.path.dirname(os.path.abspath(args.mame)))
    cmd = [args.ribbit, "headless", "--firmware", args.firmware, "--max-steps", "50000000", "--tail", "0",
           "--breakpoint", "0x" + args.stop, "--trace", ribbit_trace]
    for s in MAME_0289_STUBS:
        cmd += ["--mmio-stub", s]
    subprocess.run(cmd, capture_output=True)

    mame = firsts(mame_trace, r"([0-9A-F]{8}): ")
    ribbit = firsts(ribbit_trace, r"\s*\d+ ([0-9a-f]{8}): ")
    if ribbit and mame and ribbit[0] != mame[0]:
        ribbit = ribbit[1:]  # MAME's trace starts after the reset-vector jump
    n = min(len(mame), len(ribbit))
    div = next((i for i in range(n) if mame[i] != ribbit[i]), None)
    print(f"distinct PCs: mame {len(mame)}, ribbit {len(ribbit)}")
    if div is None:
        print(f"identical first-visit order for {n} PCs")
    else:
        print(f"first divergence at index {div}: mame {mame[div]:08x}, ribbit {ribbit[div]:08x}")


if __name__ == "__main__":
    main()
