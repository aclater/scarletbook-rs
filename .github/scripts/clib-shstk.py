# SPDX-License-Identifier: MIT OR Apache-2.0
"""Marks every object of an x86-64 ELF static library as compatible with the CET shadow
stack (issue #125).

The linker ANDs GNU_PROPERTY_X86_FEATURE_1_AND over every object it pulls from an
archive, so each member needs the SHSTK bit; stable rustc cannot emit it. A member
without a .note.gnu.property section gets one holding only that property. A member that
has one (the C objects of compiler_builtins on some targets carry "x86 ISA used") keeps
its properties, with the SHSTK bit added. Code and symbols are not touched.

Usage: clib-shstk.py LIBRARY.a   (rewrites the archive in place; needs ar and objcopy)
"""

import os
import shutil
import struct
import subprocess
import sys

NT_GNU_PROPERTY_TYPE_0 = 5
GNU_PROPERTY_X86_FEATURE_1_AND = 0xC0000002
GNU_PROPERTY_X86_FEATURE_1_SHSTK = 2
SECTION = ".note.gnu.property"


def align8(n):
    return (n + 7) & ~7


def properties(note):
    """The (type, data) pairs of an ELF64 NT_GNU_PROPERTY_TYPE_0 note "GNU"."""
    namesz, descsz, ntype = struct.unpack_from("<III", note)
    if namesz != 4 or ntype != NT_GNU_PROPERTY_TYPE_0 or note[12:16] != b"GNU\0":
        raise ValueError(f"unexpected {SECTION} note")
    if align8(16 + descsz) != len(note):
        raise ValueError(f"{SECTION} holds more than one note")
    desc, pos, props = note[16 : 16 + descsz], 0, []
    while pos < len(desc):
        ptype, datasz = struct.unpack_from("<II", desc, pos)
        props.append((ptype, desc[pos + 8 : pos + 8 + datasz]))
        pos += 8 + align8(datasz)
    return props


def with_shstk(props):
    """The properties with the SHSTK bit of X86_FEATURE_1_AND set, sorted by type."""
    feature = [d for t, d in props if t == GNU_PROPERTY_X86_FEATURE_1_AND]
    bits = struct.unpack("<I", feature[0])[0] if feature else 0
    props = [(t, d) for t, d in props if t != GNU_PROPERTY_X86_FEATURE_1_AND]
    props.append((GNU_PROPERTY_X86_FEATURE_1_AND, struct.pack("<I", bits | GNU_PROPERTY_X86_FEATURE_1_SHSTK)))
    return sorted(props)


def note_bytes(props):
    desc = b"".join(
        struct.pack("<II", t, len(d)) + d + b"\0" * (align8(len(d)) - len(d)) for t, d in props
    )
    return struct.pack("<III", 4, len(desc), NT_GNU_PROPERTY_TYPE_0) + b"GNU\0" + desc


def main():
    archive = os.path.abspath(sys.argv[1])
    members = subprocess.run(["ar", "t", archive], check=True, capture_output=True, text=True).stdout.split()
    if len(set(members)) != len(members):
        raise SystemExit(f"{archive}: duplicate member names")
    work = archive + ".shstk"
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    subprocess.run(["ar", "x", archive], cwd=work, check=True)
    for member in members:
        # objcopy fails, or only warns (binutils 2.35), when the section does not exist.
        old = os.path.join(work, "note.old")
        subprocess.run(["objcopy", "--dump-section", f"{SECTION}=note.old", member], cwd=work, capture_output=True)
        if os.path.exists(old):
            with open(old, "rb") as f:
                props = properties(f.read())
            os.remove(old)
            action = ["--update-section", f"{SECTION}=note.bin"]
        else:
            props = []
            action = [
                "--add-section",
                f"{SECTION}=note.bin",
                "--set-section-flags",
                f"{SECTION}=alloc,readonly,data",
                "--set-section-alignment",
                f"{SECTION}=8",
            ]
        with open(os.path.join(work, "note.bin"), "wb") as f:
            f.write(note_bytes(with_shstk(props)))
        subprocess.run(["objcopy", *action, member], cwd=work, check=True)
    os.remove(archive)
    # The members in their original order, with a fresh symbol index.
    subprocess.run(["ar", "rcsD", archive, *members], cwd=work, check=True)
    shutil.rmtree(work)
    print(f"{archive}: {len(members)} members marked SHSTK")


if __name__ == "__main__":
    main()
