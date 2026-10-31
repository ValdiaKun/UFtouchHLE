#!/usr/bin/env python3
"""Unit tests for the offline APK/ELF package QA validator."""
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

THIS_DIR = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("qa_apk", THIS_DIR / "verify-android-apk.py")
mod = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mod)


def fake_elf(alignment=0x4000, machine=183):
    elf = bytearray(128)
    elf[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", elf, 18, machine)
    struct.pack_into("<Q", elf, 32, 64)
    struct.pack_into("<H", elf, 54, 56)
    struct.pack_into("<H", elf, 56, 1)
    struct.pack_into("<I", elf, 64, 1)
    struct.pack_into("<Q", elf, 64 + 8, 0)
    struct.pack_into("<Q", elf, 64 + 16, 0)
    struct.pack_into("<Q", elf, 64 + 48, alignment)
    return bytes(elf)


class VerifyApkTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.apk = Path(self.tmp.name) / "test.apk"

    def build_apk(self, **extra):
        files = {"AndroidManifest.xml": b"manifest", "classes.dex": b"dex"}
        files.update({name: fake_elf() for name in mod.EXPECTED_LIBS})
        files.update(extra)
        with zipfile.ZipFile(self.apk, "w") as zf:
            for name, data in files.items():
                zf.writestr(name, data)
        return self.apk

    def test_valid_minimal_apk_and_aligned_elves(self):
        report = mod.verify(self.build_apk())
        self.assertEqual(len(report["native_libraries"]), 3)
        self.assertEqual(len(report["sha256"]), 64)

    def test_rejects_non_arm64(self):
        with self.assertRaisesRegex(mod.ValidationError, "machine"):
            mod.inspect_elf(fake_elf(machine=62), "bad.so")

    def test_rejects_4kb_load_align(self):
        with self.assertRaisesRegex(mod.ValidationError, "16 KiB aligned"):
            mod.inspect_elf(fake_elf(alignment=0x1000), "bad.so")

    def test_rejects_missing_library(self):
        self.build_apk()
        with zipfile.ZipFile(self.apk) as zf:
            payloads = {n: zf.read(n) for n in zf.namelist() if not n.endswith("libSDL2.so")}
        with zipfile.ZipFile(self.apk, "w") as zf:
            for name, data in payloads.items():
                zf.writestr(name, data)
        with self.assertRaisesRegex(mod.ValidationError, "Missing native libraries"):
            mod.verify(self.apk)

    def test_rejects_forbidden_game_image(self):
        with self.assertRaisesRegex(mod.ValidationError, "game image"):
            mod.verify(self.build_apk(**{"assets/test.ipa": b"bad"}))

    def test_rejects_other_abi(self):
        with self.assertRaisesRegex(mod.ValidationError, "non-ARM64"):
            mod.verify(self.build_apk(**{"lib/x86_64/libunused.so": fake_elf()}))

    def test_rejects_private_key_material(self):
        with self.assertRaisesRegex(mod.ValidationError, "signing material"):
            mod.verify(self.build_apk(**{"assets/release.jks": b"private"}))

    def test_rejects_duplicate_zip_name(self):
        self.build_apk()
        import warnings
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(self.apk, "a") as zf:
                zf.writestr("classes.dex", b"duplicate")
        with self.assertRaisesRegex(mod.ValidationError, "duplicate"):
            mod.verify(self.apk)


if __name__ == "__main__":
    unittest.main()
