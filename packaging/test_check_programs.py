import struct
import unittest

import check_mcp


def pe(machine, dlls):
    """A minimal PE32+ image with one section holding an import table."""
    section_rva, raw = 0x1000, 0x200
    offset = 0x40
    header = bytearray(raw)
    header[0:2] = b"MZ"
    struct.pack_into("<I", header, 0x3C, offset)
    header[offset:offset + 4] = b"PE\0\0"
    optional_size = 112 + 16 * 8
    struct.pack_into("<HH12xH", header, offset + 4, machine, 1, optional_size)
    optional = offset + 24
    struct.pack_into("<H", header, optional, 0x20B)
    struct.pack_into("<II", header, optional + 112 + 8, section_rva, 20 * (len(dlls) + 1))
    section = optional + optional_size
    body = bytearray(20 * (len(dlls) + 1))
    for i, dll in enumerate(dlls):
        struct.pack_into("<I", body, 20 * i + 12, section_rva + len(body))
        body += dll.encode() + b"\0"
    struct.pack_into("<IIII", header, section + 8, len(body), section_rva, len(body), raw)
    return bytes(header + body)


class CheckMcpTests(unittest.TestCase):
    def test_architectures_of_each_format(self):
        elf = bytearray(64)
        elf[0:4], elf[5] = b"\x7fELF", 1
        for machine, name in ((62, "x86_64"), (183, "aarch64")):
            struct.pack_into("<H", elf, 18, machine)
            self.assertEqual(check_mcp.architectures(bytes(elf)), [name])
        macho = b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x0100000C) + bytes(24)
        self.assertEqual(check_mcp.architectures(macho), ["aarch64"])
        fat = b"\xca\xfe\xba\xbe" + struct.pack(">I", 2)
        fat += struct.pack(">I16x", 0x01000007) + struct.pack(">I16x", 0x0100000C)
        self.assertEqual(check_mcp.architectures(fat), ["x86_64", "aarch64"])
        self.assertEqual(check_mcp.architectures(pe(0xAA64, [])), ["aarch64"])
        with self.assertRaises(SystemExit):
            check_mcp.architectures(b"#!/bin/sh\n")

    def test_a_pe_images_imports_are_read(self):
        image = pe(0x8664, ["KERNEL32.dll", "VCRUNTIME140.dll"])
        self.assertEqual(check_mcp.pe_imports(image), ["KERNEL32.dll", "VCRUNTIME140.dll"])

    def test_only_the_linux_system_libraries_pass(self):
        for name in ("linux-vdso.so.1", "libc.so.6", "libm.so.6", "libgcc_s.so.1",
                     "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"):
            self.assertTrue(check_mcp.LINUX_SYSTEM.match(name), name)
        for name in ("libssl.so.3", "libonnxruntime.so", "libz.so.1"):
            self.assertFalse(check_mcp.LINUX_SYSTEM.match(name), name)


if __name__ == "__main__":
    unittest.main()
