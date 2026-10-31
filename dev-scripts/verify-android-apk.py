#!/usr/bin/env python3
"""Offline UFtouchHLE Android APK integrity checks. Standard library only.

Structural validation only: not an emulator run or gameplay benchmark.
Use aapt2/apksigner separately to verify manifest metadata and signatures.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys
import zipfile

EXPECTED_LIBS = {
    "lib/arm64-v8a/libtouchHLE.so",
    "lib/arm64-v8a/libSDL2.so",
    "lib/arm64-v8a/libc++_shared.so",
}
FORBIDDEN_SUFFIXES = (".ipa", ".iso", ".xiso", ".keystore", ".jks", ".p12", ".pem", ".key")
REQUIRED_ALIGNMENT = 0x4000


class ValidationError(Exception):
    pass


def inspect_elf(data, path):
    """Check AArch64 ELF64 PT_LOAD alignment without external ELF packages."""
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise ValidationError(f"{path}: not an ELF64 little-endian library")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 183:  # EM_AARCH64
        raise ValidationError(f"{path}: ELF machine {machine}, expected AArch64 (183)")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize = struct.unpack_from("<H", data, 54)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    if phentsize < 56 or not 0 < phnum <= 4096 or phoff + phentsize * phnum > len(data):
        raise ValidationError(f"{path}: invalid ELF program header table")
    loads = []
    for i in range(phnum):
        off = phoff + phentsize * i
        if struct.unpack_from("<I", data, off)[0] != 1:  # PT_LOAD
            continue
        p_offset = struct.unpack_from("<Q", data, off + 8)[0]
        p_vaddr = struct.unpack_from("<Q", data, off + 16)[0]
        align = struct.unpack_from("<Q", data, off + 48)[0]
        if align < REQUIRED_ALIGNMENT or (p_vaddr - p_offset) % REQUIRED_ALIGNMENT != 0:
            raise ValidationError(f"{path}: PT_LOAD {i} is not 16 KiB aligned (p_align=0x{align:x})")
        loads.append(align)
    if not loads:
        raise ValidationError(f"{path}: ELF has no PT_LOAD segments")
    return {"machine": "AArch64", "load_segments": len(loads), "min_load_alignment": min(loads)}


def verify(apk):
    digest = hashlib.sha256(apk.read_bytes()).hexdigest()
    with zipfile.ZipFile(apk) as zf:
        if zf.testzip() is not None:
            raise ValidationError("APK has an invalid ZIP entry or CRC")
        names = zf.namelist()
        name_set = set(names)
        if len(names) != len(name_set):
            raise ValidationError("APK has duplicate ZIP names")
        if "AndroidManifest.xml" not in name_set or "classes.dex" not in name_set:
            raise ValidationError("APK missing AndroidManifest.xml or classes.dex")
        for name in names:
            parts = Path(name).parts
            if name.startswith("/") or ".." in parts or "\\" in name:
                raise ValidationError(f"Unsafe APK path: {name}")
            if name.lower().endswith(FORBIDDEN_SUFFIXES) or "touchHLE_apps/" in name:
                raise ValidationError(f"Unexpected game image or signing material: {name}")
        native = {n for n in names if n.startswith("lib/") and n.endswith(".so")}
        missing = EXPECTED_LIBS - native
        if missing:
            raise ValidationError(f"Missing native libraries: {sorted(missing)}")
        other_abis = [n for n in native if not n.startswith("lib/arm64-v8a/")]
        if other_abis:
            raise ValidationError(f"Unexpected non-ARM64 native libraries: {other_abis}")
        inspected = {name: inspect_elf(zf.read(name), name) for name in sorted(native)}
    return {
        "apk": apk.name, "sha256": digest, "size_bytes": apk.stat().st_size,
        "native_libraries": inspected,
        "tests": "structural APK and 16 KiB ELF PT_LOAD checks only",
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--report", type=Path, help="Optional path for JSON report")
    args = parser.parse_args(argv)
    try:
        report = verify(args.apk)
    except (OSError, zipfile.BadZipFile, ValidationError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    result = json.dumps(report, indent=2) + "\n"
    print(result, end="")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(result, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
