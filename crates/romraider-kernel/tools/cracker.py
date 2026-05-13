#!/usr/bin/env python3
"""
cracker.py — verify Subaru SH7058 CAN seed/key algorithm.

Sources:
  - nisprog/ssm_backend.c::sub_genkey  (K-Line baseline, fenugrec/nisprog)
  - james-portman/subaru-ecu-flashing/encryption.py  (CAN, 06 EDM Impreza
    STI, SH7058 — appears identical to ours: 2007 Forester XT)

Reference test vector from james-portman (CAN):
  generate_0x27_auth_key([0x00,0x00,0x00,0x00]) == [0x2a,0x52,0xf9,0x63]
  decrypt_0x36([0x21, 0x78, 0xb1, 0x0a])         == [0x00, 0x09, 0x00, 0x09]

KEY OBSERVATION about james-portman's `encrypt()`:
  - high_word = (data[0]<<8) | data[1]   ← seed[0..1]
  - low_word  = (data[2]<<8) | data[3]   ← seed[2..3]
  - rounds iterate j in [0..rounds-1] using `word_list[len-1-j]`
    so for j=0 it grabs `word_list[15]` (== 0x88F0 for 0x27, == 0x6E86 for 0x36)
  - F-function: idx = low ^ word_list[…]; t = transformnibbles(idx);
                t = rotr3(t, 16-bit); new_low = t ^ high; new_high = old_low
  - After loop: data[0]=low>>8; data[1]=low&FF; data[2]=high>>8; data[3]=high&FF
    ⇒ implicit final word-swap (the new_high/new_low get swapped on write-out).

vs nisprog/sub_genkey (K-Line):
  - same structure but
      * iterates ki = 15..0  (so first round uses word_list[15] too — match)
      * BUT uses ROTATE-RIGHT 3 (>>3 | <<13)  — matches james-portman
      * has NO final byte-swap (the loop already alternates low/high)
  Both effectively run identical state machines; the visible "difference"
  is purely in IO marshalling.

So the only real differences between K-Line `sub_genkey` and CAN `generate_0x27_auth_key` should be:
  (a) which 16 round-keys are used (apparently the SAME 0x53DA..0x88F0!), and
  (b) whether the input seed bytes are interpreted big-endian/little-endian, plus output marshalling.

This cracker tests EVERY permutation against captured pairs.
"""

import json
import sys
import itertools
from pathlib import Path

# ---------------------------------------------------------------------------
# Reference tables.
# ---------------------------------------------------------------------------

# 32-entry nibble S-box (identical in nisprog K-Line and james-portman CAN).
INDEX_TRANSFORMATION = [
    0x5, 0x6, 0x7, 0x1, 0x9, 0xC, 0xD, 0x8,
    0xA, 0xD, 0x2, 0xB, 0xF, 0x4, 0x0, 0x3,
    0xB, 0x4, 0x6, 0x0, 0xF, 0x2, 0xD, 0x9,
    0x5, 0xC, 0x1, 0xA, 0x3, 0xD, 0xE, 0x8,
]

# 16 round-keys for 0x27 SecurityAccess (K-Line and CAN per james-portman).
KEYTABLE_GENKEY_16 = [
    0x53DA, 0x33BC, 0x72EB, 0x437D,
    0x7CA3, 0x3382, 0x834F, 0x3608,
    0xAFB8, 0x503D, 0xDBA3, 0x9D34,
    0x3563, 0x6B70, 0x6E74, 0x88F0,
]

# 4 round-keys for 0x36 RequestDownload data encryption (kernel upload).
KEYTABLE_ENCRYPT_4 = [0x7856, 0xCE22, 0xF513, 0x6E86]

# Alternative observed in james-portman source (commented "not sure when used"):
KEYTABLE_ALT_16 = [
    0x24B9, 0x9D91, 0xFF0C, 0xB8D5, 0x15BB, 0xF998, 0x8723,
    0x9E05, 0x7092, 0xD683, 0xBA03, 0x59E1, 0x6136, 0x9B9A,
    0x9CFB, 0x9DDB,
]


# ---------------------------------------------------------------------------
# Direct port of james-portman's `encrypt()` from subaru-ecu-flashing.
# Verbatim, only Pythonic minor cleanups.
# ---------------------------------------------------------------------------

def transformnibbles_jp(num):
    """james-portman's exact transformnibbles()."""
    # Mirror low byte into bits 16..23 so nibble lookups [(num>>0..12)%32]
    # span 5-bit indices (the low nibble carries bit 4 from the byte above).
    num = (num + ((num & 0xFF) << 16)) & 0xFFFFFF
    result = 0
    for i in range(4):
        result += INDEX_TRANSFORMATION[(num >> (i * 4)) % 32] << (i * 4)
    return result & 0xFFFF


def encrypt_jp(data, word_list, rounds):
    """james-portman's exact encrypt()."""
    data = list(data)  # copy
    i = 0
    while i < len(data):
        high_word = (data[i + 0] << 8) | data[i + 1]
        low_word = (data[i + 2] << 8) | data[i + 3]

        for j in range(rounds):
            idx2 = low_word ^ word_list[len(word_list) - 1 - j]
            key16 = transformnibbles_jp(idx2)
            # rotate right 3
            for _ in range(3):
                rotated_bit = key16 & 0b1
                key16 = (key16 >> 1) + (rotated_bit << 15)
            num = key16 ^ high_word
            high_word = low_word
            low_word = num

        # Note the *swap on write*: bytes 0..1 get low_word, bytes 2..3 get high.
        data[i + 0] = low_word >> 8
        data[i + 1] = low_word & 0xFF
        data[i + 2] = high_word >> 8
        data[i + 3] = high_word & 0xFF

        i += 4

    return bytes(data)


def generate_0x27_auth_key_jp(seed_bytes):
    return encrypt_jp(seed_bytes, KEYTABLE_GENKEY_16, rounds=16)


def encrypt_0x36_jp(data):
    return encrypt_jp(data, [0x6E86, 0xF513, 0xCE22, 0x7856], rounds=4)


def decrypt_0x36_jp(data):
    return encrypt_jp(data, KEYTABLE_ENCRYPT_4, rounds=4)


# Self-test against james-portman's published assertions.
def selftest_jp():
    assert generate_0x27_auth_key_jp(bytes([0x00, 0x00, 0x00, 0x00])) == \
        bytes([0x2a, 0x52, 0xf9, 0x63]), "JP self-test 0x27 failed"
    assert decrypt_0x36_jp(bytes([0x21, 0x78, 0xb1, 0x0a])) == \
        bytes([0x00, 0x09, 0x00, 0x09]), "JP self-test 0x36 decrypt failed"
    assert encrypt_0x36_jp(bytes([0x00, 0x09, 0x00, 0x09])) == \
        bytes([0x21, 0x78, 0xb1, 0x0a]), "JP self-test 0x36 encrypt failed"


# ---------------------------------------------------------------------------
# Generalised exploration framework — try every reasonable variant.
# ---------------------------------------------------------------------------

def feistel_round_generic(state, round_key, rot_dir="right", rot_amt=3):
    state &= 0xFFFFFFFF
    word_low = state & 0xFFFF
    word_high = (state >> 16) & 0xFFFF
    index = word_low ^ round_key
    # mirror byte (like james-portman) so nibble[4]/[5]/[6] sample bits 16..23
    index_mirrored = (index + ((index & 0xFF) << 16)) & 0xFFFFFF
    enc_key = 0
    for n in range(4):
        nibble_idx = (index_mirrored >> (n * 4)) & 0x1F
        enc_key += INDEX_TRANSFORMATION[nibble_idx] << (n * 4)
    enc_key &= 0xFFFF
    if rot_dir == "right":
        enc_key = ((enc_key >> rot_amt) | (enc_key << (16 - rot_amt))) & 0xFFFF
    else:
        enc_key = ((enc_key << rot_amt) | (enc_key >> (16 - rot_amt))) & 0xFFFF
    new_low = enc_key ^ word_high
    new_high = word_low
    return ((new_high << 16) | new_low) & 0xFFFFFFFF


def run_variant(seed_bytes, round_keys, *,
                seed_endian="be",   # how to load seed bytes into state
                key_order="rev",    # rev = word_list[N-1-j] (matches JP), fwd = word_list[j]
                rot_dir="right",
                rot_amt=3,
                final_swap=False,
                out_endian="be",    # how to render state to key bytes
                out_swap_words=False):
    """Run a Feistel cipher described by these parameters and return 4 key bytes."""
    # 1. load
    if seed_endian == "be":
        state = int.from_bytes(seed_bytes, "big")
    else:
        state = int.from_bytes(seed_bytes, "little")
    # 2. iterate
    indices = range(len(round_keys) - 1, -1, -1) if key_order == "rev" else range(len(round_keys))
    for ki in indices:
        state = feistel_round_generic(state, round_keys[ki], rot_dir, rot_amt)
    # 3. optional 32-bit word swap (high/low half-swap)
    if final_swap:
        state = ((state << 16) | (state >> 16)) & 0xFFFFFFFF
    if out_swap_words:
        b = state.to_bytes(4, "big")
        return bytes([b[2], b[3], b[0], b[1]])
    # 4. render
    if out_endian == "be":
        return state.to_bytes(4, "big")
    else:
        return state.to_bytes(4, "little")


def try_variant(name, pairs, **kwargs):
    matches = 0
    first_miss = None
    for p in pairs:
        seed = bytes.fromhex(p["seed"])
        expected = bytes.fromhex(p["key"])
        got = run_variant(seed, **kwargs)
        if got == expected:
            matches += 1
        elif first_miss is None:
            first_miss = (p["seed"], p["key"], got.hex().upper())
    return matches, len(pairs), name, first_miss


# ---------------------------------------------------------------------------
# Direct application of james-portman's CAN algorithm (preferred path).
# ---------------------------------------------------------------------------

def verify_jp_directly(pairs):
    print("\n[1] Direct test of james-portman's generate_0x27_auth_key() ...")
    all_ok = True
    for p in pairs:
        seed = bytes.fromhex(p["seed"])
        expected = bytes.fromhex(p["key"])
        got = generate_0x27_auth_key_jp(seed)
        mark = "OK " if got == expected else "MISS"
        print(f"    {mark}  seed={p['seed']}  expected={p['key']}  got={got.hex().upper()}")
        if got != expected:
            all_ok = False
    return all_ok


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    selftest_jp()
    print("[self-test] james-portman algorithm self-asserts OK.")

    json_path = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/seed_key_pairs.json")
    pairs = json.loads(json_path.read_text())
    print(f"[load] {len(pairs)} (seed, key) pair(s) from {json_path}")

    # 1. Direct test first.
    ok = verify_jp_directly(pairs)
    if ok:
        print("\nSOLVED: Subaru CAN 0x27 SecurityAccess key derivation =\n"
              "        james-portman generate_0x27_auth_key() (16-round Feistel,\n"
              "        word table 0x53DA..0x88F0, nibble S-box, rot-right-3).\n"
              "        Identical (modulo IO marshalling) to nisprog K-Line.\n")
        return

    # 1b. Try james-portman's "ALT_16" table too.
    print("\n[1b] Trying james-portman's commented-out alt 16-word table ...")
    all_ok = True
    for p in pairs:
        seed = bytes.fromhex(p["seed"])
        expected = bytes.fromhex(p["key"])
        got = encrypt_jp(seed, KEYTABLE_ALT_16, rounds=16)
        mark = "OK " if got == expected else "MISS"
        print(f"    {mark}  seed={p['seed']}  expected={p['key']}  got={got.hex().upper()}")
        if got != expected:
            all_ok = False
    if all_ok:
        print("\nSOLVED with ALT_16 table.")
        return

    # 2. Otherwise, sweep variants.
    print("\n[2] No direct match. Sweeping structural variants ...\n")

    round_key_tables = {
        "GENKEY_16":   KEYTABLE_GENKEY_16,
        "GENKEY_16r":  list(reversed(KEYTABLE_GENKEY_16)),
        "ALT_16":      KEYTABLE_ALT_16,
        "ALT_16r":     list(reversed(KEYTABLE_ALT_16)),
    }
    seed_endians = ["be", "le"]
    key_orders = ["rev", "fwd"]
    rot_dirs = ["right", "left"]
    rot_amts = [3, 13]
    final_swaps = [False, True]
    out_endians = ["be", "le"]
    out_swap_words = [False, True]

    best = (0, len(pairs), None, None)
    for tname, table in round_key_tables.items():
        for se, ko, rd, ra, fs, oe, osw in itertools.product(
            seed_endians, key_orders, rot_dirs, rot_amts,
            final_swaps, out_endians, out_swap_words,
        ):
            label = (f"{tname}/seed={se}/order={ko}/rot={rd}{ra}/"
                     f"swap={int(fs)}/out={oe}/swap_w={int(osw)}")
            m, t, _, miss = try_variant(
                label, pairs,
                round_keys=table,
                seed_endian=se, key_order=ko,
                rot_dir=rd, rot_amt=ra,
                final_swap=fs, out_endian=oe,
                out_swap_words=osw,
            )
            if m == t:
                print(f"  HIT  {m}/{t}  {label}")
            elif m > best[0]:
                best = (m, t, label, miss)

    print()
    if best[0] == best[1] and best[0] > 0:
        print(f"SOLVED variant: {best[2]}")
    elif best[0] > 0:
        print(f"best partial: {best[0]}/{best[1]}  {best[2]}")
        print(f"  example miss: {best[3]}")
    else:
        print("No structural variant of K-Line algorithm matched. Deeper RE needed.")


if __name__ == "__main__":
    main()
