#!/usr/bin/env python3
"""
find_round_keys.py — найти 16 round-keys для SID 0x27 seed/key в **ROM нашего ECU**.

Из agent reseach мы знаем что:
- Subaru SH7058 seed/key algorithm universal: 16-round Feistel + 32-byte nibble S-box.
- Round-keys (16 × u16) firmware-specific — captures не совпадают с K-Line nisprog values.
- Round-keys ARE in the ROM, **рядом с S-box** который мы знаем.

Стратегия:
1. Найти S-box pattern в ROM (`05 06 07 01 09 0C 0D 08 0A 0D 02 0B 0F 04 00 03 0B 04 06 00 0F 02 0D 09 05 0C 01 0A 03 0D 0E 08`).
2. Рядом (within ~256 байт) поискать **16 × u16 BE** массив — это candidate round-keys.
3. Прогнать против наших 7 captured (seed, key) пар через james-portman's
   verified Feistel implementation. Если совпадение — мы нашли key table.
"""

import json
import sys
from pathlib import Path

# Verbatim из james-portman/subaru-ecu-flashing/encryption.py
INDEX_TRANSFORMATION = [
    0x5, 0x6, 0x7, 0x1, 0x9, 0xC, 0xD, 0x8,
    0xA, 0xD, 0x2, 0xB, 0xF, 0x4, 0x0, 0x3,
    0xB, 0x4, 0x6, 0x0, 0xF, 0x2, 0xD, 0x9,
    0x5, 0xC, 0x1, 0xA, 0x3, 0xD, 0xE, 0x8,
]
SBOX_BYTES = bytes(INDEX_TRANSFORMATION)


def transformnibbles(num):
    num = (num + ((num & 0xFF) << 16)) & 0xFFFFFF
    r = 0
    for i in range(4):
        r += INDEX_TRANSFORMATION[(num >> (i * 4)) % 32] << (i * 4)
    return r & 0xFFFF


def generate_0x27_auth_key(seed_bytes, word_list):
    """james-portman's exact encrypt() with our candidate word_list."""
    data = list(seed_bytes)
    rounds = len(word_list)
    high_word = (data[0] << 8) | data[1]
    low_word = (data[2] << 8) | data[3]
    for j in range(rounds):
        idx = low_word ^ word_list[rounds - 1 - j]
        key16 = transformnibbles(idx)
        # rotate right 3
        for _ in range(3):
            rb = key16 & 1
            key16 = (key16 >> 1) + (rb << 15)
        num = key16 ^ high_word
        high_word = low_word
        low_word = num
    return bytes([low_word >> 8, low_word & 0xFF, high_word >> 8, high_word & 0xFF])


def test_table(pairs, word_list):
    for p in pairs:
        seed = bytes.fromhex(p["seed"])
        expected = bytes.fromhex(p["key"])
        if generate_0x27_auth_key(seed, word_list) != expected:
            return False
    return True


def read_u16_be(rom, pos): return (rom[pos] << 8) | rom[pos + 1]
def read_u16_le(rom, pos): return rom[pos] | (rom[pos + 1] << 8)


def scan_rom_full(rom, pairs, endian="be"):
    """Полный sweep ROM: для каждого aligned u16-position попробовать table из 16 слов.
    Пред-фильтр: тестируем сначала по 1 паре (быстро), полные 7 только на survivors."""
    first_seed = bytes.fromhex(pairs[0]["seed"])
    first_key = bytes.fromhex(pairs[0]["key"])
    reader = read_u16_be if endian == "be" else read_u16_le

    survivors = []
    n_positions = (len(rom) - 32) // 2
    print(f"  Scanning {n_positions:,} positions ({endian.upper()})…")
    for tbl_pos in range(0, len(rom) - 32, 2):
        table = [reader(rom, tbl_pos + i * 2) for i in range(16)]
        # Quick filter: skip tables that are all-zero or all-one (clearly not key data)
        if all(w == 0 for w in table) or all(w == 0xFFFF for w in table):
            continue
        if generate_0x27_auth_key(first_seed, table) == first_key:
            survivors.append((tbl_pos, table))
    print(f"  → {len(survivors)} survivor(s) match pair #1")

    final = []
    for tbl_pos, table in survivors:
        if test_table(pairs, table):
            final.append((tbl_pos, table))
    return final


def main():
    rom_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
        "/Users/firefighter91/Documents/Develop/MyProjects/RomRider/romraider-rs/"
        "fixtures/forester-xt-2007-4E42504007.bin"
    )
    pairs_path = Path("/tmp/seed_key_pairs.json")

    rom = rom_path.read_bytes()
    pairs = json.loads(pairs_path.read_text())
    print(f"ROM:   {rom_path.name}  size={len(rom)} bytes")
    print(f"Pairs: {len(pairs)} captured (seed, key)")

    print("\n=== Phase 1: Big-endian u16 sweep over full ROM ===")
    found_be = scan_rom_full(rom, pairs, "be")
    if found_be:
        print(f"\n✅ {len(found_be)} BE match(es):")
        for tbl_pos, table in found_be:
            print(f"  @ 0x{tbl_pos:06X}: {' '.join(f'0x{w:04X}' for w in table)}")
        return

    print("\n=== Phase 2: Little-endian u16 sweep ===")
    found_le = scan_rom_full(rom, pairs, "le")
    if found_le:
        print(f"\n✅ {len(found_le)} LE match(es):")
        for tbl_pos, table in found_le:
            print(f"  @ 0x{tbl_pos:06X}: {' '.join(f'0x{w:04X}' for w in table)}")
        return

    print("\n❌ No 16-word table in ROM matches captured (seed, key) pairs.")
    print("   This means either:")
    print("   (a) Round-keys are not contiguous in ROM (scattered / computed runtime)")
    print("   (b) The Feistel structure itself differs (different rotation/swap/etc.)")
    print("   (c) Some seeds need different key set (alternate auth level)")


if __name__ == "__main__":
    main()
