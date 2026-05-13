#!/usr/bin/env python3
"""
extract_seed_key_pairs.py — извлечь (seed, key) пары из Wireshark pcapng
файлов с захватом EcuFlash CAN-сессий через Tactrix Openport 2.0.

Парсит USB Bulk-передачи, находит `atp 1 5\r\n<07 seed_4B>` запросы Tactrix
DLL к Tactrix firmware (proprietary key calculator) и соответствующие
`arp 1 4\r\n<key_4B>` ответы.

Эти пары — точный input/output Subaru CAN-side seed/key algorithm,
который мы хотим reverse-engineer.

Usage:
    ./extract_seed_key_pairs.py capture1.pcapng [capture2.pcapng ...]
    ./extract_seed_key_pairs.py --json out.json *.pcapng

Зависимости: tshark (брутто из Wireshark, обычно `brew install wireshark` на Mac).
"""

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass, asdict
from pathlib import Path


@dataclass
class SeedKeyPair:
    """Одна пара seed→key, привязка к source-файлу для отладки."""
    seed: str            # 8 hex chars, like "DDEEAB05"
    key:  str            # 8 hex chars
    source_file: str
    seed_time: float     # frame timestamp
    key_time:  float

    def seed_bytes(self) -> bytes: return bytes.fromhex(self.seed)
    def key_bytes(self)  -> bytes: return bytes.fromhex(self.key)


def extract_pairs_from_pcapng(path: Path) -> list[SeedKeyPair]:
    """Запускает tshark на pcapng файл, парсит USBCOM payloads."""
    cmd = [
        "tshark", "-r", str(path),
        "-Y", "usbcom.data.out_payload || usbcom.data.in_payload",
        "-T", "fields",
        "-e", "frame.time_relative",
        "-e", "usbcom.data.out_payload",
        "-e", "usbcom.data.in_payload",
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
    except FileNotFoundError:
        print("ERROR: tshark not found (brew install wireshark)", file=sys.stderr)
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        print(f"ERROR: tshark failed on {path.name}: {e.stderr}", file=sys.stderr)
        return []

    # Парсим строки. Каждая строка = `time\tout_hex\tin_hex` (одно из 2-3 пустое)
    events = []
    for line in result.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        try:
            t = float(parts[0])
        except ValueError:
            continue
        out_hex = parts[1].replace(":", "")
        in_hex  = parts[2].replace(":", "") if len(parts) > 2 else ""
        if out_hex:
            try:
                events.append((t, "TX", bytes.fromhex(out_hex)))
            except ValueError: pass
        if in_hex:
            try:
                events.append((t, "RX", bytes.fromhex(in_hex)))
            except ValueError: pass

    # Ищем `atp 1 5\r\n<07><seed:4>` затем ближайший `arp 1 4\r\n<key:4>`.
    pairs = []
    i = 0
    while i < len(events):
        t, dir_, data = events[i]
        if dir_ == "TX" and data.startswith(b"atp 1 5\r\n") and len(data) >= 14:
            # payload: `07 <seed_4>`
            seed = data[10:14]
            seed_time = t

            # Найти ближайший RX начинающийся с `arp 1 4\r\n` ИЛИ просто 4-byte key
            # Tactrix часто отвечает в 2 USB transfers: первый `arp 1 4\r\n` (8 байт),
            # второй — 4 байта key (или склеено).
            key = None
            key_time = None
            j = i + 1
            while j < len(events) and j < i + 10:
                t2, dir2, data2 = events[j]
                if dir2 != "RX":
                    j += 1
                    continue
                if data2.startswith(b"arp 1 4\r\n"):
                    # Variant A: все 12 байт сразу
                    if len(data2) >= 13:
                        key = data2[9:13]
                        key_time = t2
                        break
                    # Variant B: header в этом packet, key в следующем
                    if j + 1 < len(events):
                        t3, dir3, data3 = events[j + 1]
                        if dir3 == "RX" and len(data3) >= 4:
                            key = data3[:4]
                            key_time = t3
                            break
                j += 1
            if key:
                pairs.append(SeedKeyPair(
                    seed=seed.hex().upper(),
                    key=key.hex().upper(),
                    source_file=path.name,
                    seed_time=seed_time,
                    key_time=key_time,
                ))
        i += 1
    return pairs


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("pcapng_files", nargs="+", type=Path)
    ap.add_argument("--json", type=Path, help="Сохранить пары как JSON")
    args = ap.parse_args()

    all_pairs: list[SeedKeyPair] = []
    for path in args.pcapng_files:
        if not path.exists():
            print(f"WARN: {path} not found", file=sys.stderr)
            continue
        pairs = extract_pairs_from_pcapng(path)
        print(f"{path.name}: {len(pairs)} pair(s)", file=sys.stderr)
        for p in pairs:
            print(f"  seed={p.seed}  →  key={p.key}  @ t={p.seed_time:.3f}s")
        all_pairs.extend(pairs)

    print(f"\n=== TOTAL: {len(all_pairs)} (seed, key) pair(s) ===")
    for p in all_pairs:
        print(f"  {p.seed}  →  {p.key}  ({p.source_file})")

    if args.json:
        with args.json.open("w") as f:
            json.dump([asdict(p) for p in all_pairs], f, indent=2)
        print(f"\nSaved to {args.json}")

    # Quick analysis pass.
    if len(all_pairs) >= 2:
        print("\n=== Quick analysis ===")
        seeds = [p.seed_bytes() for p in all_pairs]
        keys = [p.key_bytes() for p in all_pairs]
        # Detect determinism
        unique_seeds = set(s.hex() for s in seeds)
        if len(unique_seeds) < len(seeds):
            print(f"  ⚠ {len(seeds) - len(unique_seeds)} seed(s) repeated across captures — partial determinism")
        else:
            print(f"  ✓ All {len(seeds)} seeds unique → ECU uses true random nonce")
        # XOR analysis between seed↔key per pair
        for p in all_pairs:
            s, k = p.seed_bytes(), p.key_bytes()
            xor = bytes(a ^ b for a, b in zip(s, k))
            print(f"  seed={p.seed} key={p.key} seed^key={xor.hex().upper()}")


if __name__ == "__main__":
    main()
