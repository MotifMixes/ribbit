"""Regenerates the GNU-binutils oracle fixtures for ribbit-cpu.

Usage (from the repository root):
    python scripts/gen-cpu-vectors.py --toolchain path/to/arc-gnu/bin

Needs arc-elf32-as and arc-elf32-objdump from the ARC GNU toolchain
(https://github.com/foss-for-mips-arc-processors/toolchain, GPL tools; only
their *output* is committed). Writes:

  crates/ribbit-cpu/tests/vectors/corpus.objdump  objdump -d of corpus.s
  crates/ribbit-cpu/tests/vectors/fuzz.bin        deterministic pseudo-random bytes
  crates/ribbit-cpu/tests/vectors/fuzz.objdump    objdump of fuzz.bin as ARC600 code
  crates/ribbit-cpu/tests/vectors/aux-regs.objdump     `lr r0,[n]` for n < 0x800, giving
                                                  objdump's aux-register spellings
  crates/ribbit-cpu/tests/vectors/exec.bin        exec.s linked at 0x40000000
  crates/ribbit-cpu/tests/vectors/exec.sym        its symbols (address name)
"""

import argparse
import os
import subprocess
import sys

VEC = os.path.join("crates", "ribbit-cpu", "tests", "vectors")
FUZZ_BYTES = 64 * 1024


def tool(bindir, name):
    exe = os.path.join(bindir, name + (".exe" if os.name == "nt" else ""))
    if not os.path.exists(exe):
        sys.exit(f"missing {exe}")
    return exe


def run(args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def xorshift32(seed):
    x = seed
    while True:
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        yield x


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--toolchain", required=True)
    args = ap.parse_args()
    asm = tool(args.toolchain, "arc-elf32-as")
    objdump = tool(args.toolchain, "arc-elf32-objdump")
    ld = tool(args.toolchain, "arc-elf32-ld")
    objcopy = tool(args.toolchain, "arc-elf32-objcopy")
    nm = tool(args.toolchain, "arc-elf32-nm")

    obj = os.path.join(VEC, "corpus.o")
    run([asm, "-mcpu=arc600", "-mmul64", "-mnorm", "-mswap", "-mEA", "-o", obj, os.path.join(VEC, "corpus.s")])
    with open(os.path.join(VEC, "corpus.objdump"), "w", newline="\n") as f:
        f.write(run([objdump, "-d", obj]))
    os.remove(obj)

    rng = xorshift32(0x5EED1234)
    data = bytearray()
    while len(data) < FUZZ_BYTES:
        data += next(rng).to_bytes(4, "little")
    fuzz = os.path.join(VEC, "fuzz.bin")
    with open(fuzz, "wb") as f:
        f.write(data)
    with open(os.path.join(VEC, "fuzz.objdump"), "w", newline="\n") as f:
        f.write(run([objdump, "-D", "-b", "binary", "-m", "ARC600", "-EL", fuzz]))
    aux = bytearray()
    for n in range(0x800):
        # LR r0,[s12]: major 0x04, P=2, sub-opcode 0x2A.
        w = (4 << 27) | (2 << 22) | (0x2A << 16) | ((n & 0x3F) << 6) | ((n >> 6) & 0x3F)
        aux += (w >> 16).to_bytes(2, "little") + (w & 0xFFFF).to_bytes(2, "little")
    auxbin = os.path.join(VEC, "aux.bin")
    with open(auxbin, "wb") as f:
        f.write(aux)
    with open(os.path.join(VEC, "aux-regs.objdump"), "w", newline="\n") as f:
        f.write(run([objdump, "-D", "-b", "binary", "-m", "ARC600", "-EL", auxbin]))
    os.remove(auxbin)
    eo = os.path.join(VEC, "exec.o")
    elf = os.path.join(VEC, "exec.elf")
    run([asm, "-mcpu=arc600", "-mmul64", "-mnorm", "-mswap", "-mEA", "-o", eo, os.path.join(VEC, "exec.s")])
    run([ld, "-Ttext=0x40000000", "-e", "_vectors", "-o", elf, eo])
    run([objcopy, "-O", "binary", "-j", ".text", elf, os.path.join(VEC, "exec.bin")])
    syms = []
    for line in run([nm, elf]).splitlines():
        addr, kind, name = line.split()
        if kind in "Tt":
            syms.append(f"{addr} {name}")
    with open(os.path.join(VEC, "exec.sym"), "w", newline="\n") as f:
        f.write("\n".join(sorted(syms)) + "\n")
    with open(os.path.join(VEC, "exec.objdump"), "w", newline="\n") as f:
        f.write(run([objdump, "-d", elf]))
    os.remove(eo)
    os.remove(elf)
    print("ok")


if __name__ == "__main__":
    main()
