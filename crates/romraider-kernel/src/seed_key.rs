//! Subaru SH7055/SH7058 seed/key challenge-response + SID 0x36 payload encryption.
//!
//! **Прямой порт** из `fenugrec/nisprog/ssm_backend.c` (GPL-3.0):
//! - `sub_genkey()` → [`subaru_genkey`] — 16-round Feistel, для unlock в SID 0x27
//! - `sub_encrypt()` → [`subaru_encrypt_word`] — 4-round Feistel, для encrypt payload в SID 0x36
//!
//! Обе функции:
//! - принимают 4 байта big-endian → одно `u32`
//! - крутят Feistel-like rounds (16 для genkey, 4 для encrypt)
//! - возвращают 4 байта big-endian
//!
//! Один и тот же `INDEX_TRANSFORMATION[32]` (nibble-S-box) используется в обеих —
//! отличаются только `KEYTABLE` и количество раундов.
//!
//! ## Wire-flow seed/key (SID 0x27)
//!
//! 1. Tester → ECU: `27 01` (request seed)
//! 2. ECU → Tester: `67 01 S3 S2 S1 S0` (4-байт seed, big-endian)
//! 3. Tester → ECU: `27 02 K3 K2 K1 K0` (4-байт key, big-endian)
//! 4. ECU → Tester: `67 02` (success) или `7F 27 35` (NRC 0x35 INVALID_KEY)
//!
//! После 2 неудачных ключей подряд ECU блокируется на 10 секунд (NRC 0x37).
//!
//! ## Tablica универсальная (для Subaru)
//!
//! `4E42504007` (наша машина) и любой другой SH7058 Subaru обслуживаются
//! одним и тем же keyset-ом — никакого per-ECU-ID lookup-а как у Nissan.

/// Round-keys для генерации key из seed (16 раундов).
const KEYTABLE_GENKEY: [u16; 16] = [
    0x53DA, 0x33BC, 0x72EB, 0x437D,
    0x7CA3, 0x3382, 0x834F, 0x3608,
    0xAFB8, 0x503D, 0xDBA3, 0x9D34,
    0x3563, 0x6B70, 0x6E74, 0x88F0,
];

/// Round-keys для encrypt payload (4 раунда).
const KEYTABLE_ENCRYPT: [u16; 4] = [
    0x7856, 0xCE22, 0xF513, 0x6E86,
];

/// Round-keys для **CAN-side SID 0x27 SecurityAccess**, ECU-specific.
///
/// Извлечены через `tools/find_round_keys.py` из `fixtures/forester-xt-2007-4E42504007.bin`
/// по offset `0x05972C` — full-ROM-sweep нашёл одну-единственную 16×u16 BE таблицу,
/// которая computes correct key для всех 7 captured (seed, key) пар из live EcuFlash
/// сессий.
///
/// Algorithm structure — james-portman/subaru-ecu-flashing — это **тот же Feistel**
/// что K-Line `subaru_genkey` (тот же S-box, тот же ROR-3), но с другими round-keys.
/// FastECU's `get_key_operations_subaru.cpp` confirms что round-keys
/// firmware-specific (i.e. в каждом ROM свои).
///
/// **Этот set валиден ТОЛЬКО для ROM ID `4E42504007`** (2007 Forester XT USDM).
/// Для другого ECU придётся искать свою таблицу.
const KEYTABLE_GENKEY_CAN: [u16; 16] = [
    0x78B1, 0x4625, 0x201C, 0x9EA5,
    0xAD6B, 0x35F4, 0xFD21, 0x5E71,
    0xB046, 0x7F4A, 0x4B75, 0x93F9,
    0x1895, 0x8961, 0x3ECC, 0x862B,
];

/// 32-entry nibble S-box, общая для genkey и encrypt.
const INDEX_TRANSFORMATION: [u8; 32] = [
    0x5, 0x6, 0x7, 0x1, 0x9, 0xC, 0xD, 0x8,
    0xA, 0xD, 0x2, 0xB, 0xF, 0x4, 0x0, 0x3,
    0xB, 0x4, 0x6, 0x0, 0xF, 0x2, 0xD, 0x9,
    0x5, 0xC, 0x1, 0xA, 0x3, 0xD, 0xE, 0x8,
];

/// Один раунд Feistel-like transform.
///
/// Параметры:
/// - `state` — текущее 32-битное состояние; биты `0..16` — `wordtogenerateindex`, биты `16..32` — `wordtobeencrypted`.
/// - `round_key` — 16-битная константа раунда из соответствующей KEYTABLE.
///
/// Возвращает новое состояние.
#[inline(always)]
fn feistel_round(state: u32, round_key: u16) -> u32 {
    let word_to_gen_idx  = state as u16;        // low 16
    let word_to_enc      = (state >> 16) as u16; // high 16

    // index = (word_to_gen_idx ^ round_key) дублируется в обе половины u32:
    let index_low: u32   = u32::from(word_to_gen_idx ^ round_key);
    let index            = index_low + (index_low << 16);

    // 4 nibble-извлечения с 5-битной маской → nibble-S-box → склейка обратно в u16.
    // Маска 0x1F даёт 32 значения (отсюда таблица из 32, а не 16!).
    let mut encryption_key: u16 = 0;
    for n in 0..4 {
        let nibble_idx = ((index >> (n * 4)) & 0x1F) as usize;
        let sbox_val   = u16::from(INDEX_TRANSFORMATION[nibble_idx]);
        encryption_key = encryption_key.wrapping_add(sbox_val << (n * 4));
    }

    // 16-bit ROR 3.
    encryption_key = (encryption_key >> 3).wrapping_add(encryption_key << 13);

    // Новое состояние: high = old_low, low = encrypted (= round_output ^ old_high).
    let new_low  = encryption_key ^ word_to_enc;
    let new_high = word_to_gen_idx;
    u32::from(new_low) | (u32::from(new_high) << 16)
}

/// Финальный swap половин (одинаковый и в genkey, и в encrypt).
#[inline(always)]
fn final_word_swap(state: u32) -> u32 {
    (state >> 16) | (state << 16)
}

/// **Generate key from seed** для Subaru SID 0x27 securityAccess — **K-Line** version.
///
/// Использует канонический nisprog `keytogenerateindex[16]`. Работает для GD-chassis
/// Subaru (~2003-2005 WRX/STI) на K-Line. **НЕ подходит** для CAN-ECU — нужна
/// per-ECU keytable (см. [`subaru_genkey_can`]).
///
/// Seed и key — big-endian 4-байтные значения (как они приходят/уходят по wire).
pub fn subaru_genkey(seed: [u8; 4]) -> [u8; 4] {
    let mut state = u32::from_be_bytes(seed);
    // Раунды идут в обратном порядке: ki = 15, 14, …, 0.
    for ki in (0..16).rev() {
        state = feistel_round(state, KEYTABLE_GENKEY[ki]);
    }
    state = final_word_swap(state);
    state.to_be_bytes()
}

/// **Generate key from seed** для Subaru CAN-ECU `4E42504007` (2007 Forester XT).
///
/// Использует [`KEYTABLE_GENKEY_CAN`] — round-keys извлечены из ROM этого ECU.
/// James-portman's exact Feistel structure (с byte-mirroring в `transformnibbles`,
/// в отличие от nisprog K-Line который этого не делает) — реализация ниже.
pub fn subaru_genkey_can(seed: [u8; 4]) -> [u8; 4] {
    // james-portman algorithm:
    //   high = (seed[0]<<8) | seed[1]
    //   low  = (seed[2]<<8) | seed[3]
    //   for j in 0..16:
    //     idx = low ^ word_list[15 - j]
    //     // transformnibbles с byte-mirror: idx |= (idx & 0xFF) << 16
    //     key16 = sbox-substitute 4 nibbles (5-bit indices)
    //     key16 = ROR(key16, 3)
    //     (high, low) = (low, key16 ^ high)
    //   return [low>>8, low&FF, high>>8, high&FF]  // implicit final swap
    let mut high: u16 = u16::from(seed[0]) << 8 | u16::from(seed[1]);
    let mut low:  u16 = u16::from(seed[2]) << 8 | u16::from(seed[3]);

    for j in 0..16usize {
        let rk = KEYTABLE_GENKEY_CAN[15 - j];
        let idx_raw: u32 = u32::from(low ^ rk);
        // byte-mirror: bits 16..23 = bits 0..7 (так в transformnibbles james-portman)
        let idx = idx_raw + ((idx_raw & 0xFF) << 16);

        let mut key16: u16 = 0;
        for n in 0..4 {
            let nibble_idx = ((idx >> (n * 4)) & 0x1F) as usize;
            key16 = key16.wrapping_add(u16::from(INDEX_TRANSFORMATION[nibble_idx]) << (n * 4));
        }
        // ROR 3 (16-bit)
        key16 = (key16 >> 3) | (key16 << 13);

        let new_low = key16 ^ high;
        high = low;
        low  = new_low;
    }
    // Implicit final swap: write low first, high second.
    [(low >> 8) as u8, low as u8, (high >> 8) as u8, high as u8]
}

/// **Encrypt one 32-bit word** для Subaru SID 0x36 transferData payload.
///
/// Vorbei: payload-байты буфера группируются по 4 байта (big-endian word),
/// каждый word проходит через `subaru_encrypt_word`, результат заменяет
/// исходные 4 байта. Длина payload должна быть кратна 4 (padding-ом).
pub fn subaru_encrypt_word(plain: [u8; 4]) -> [u8; 4] {
    let mut state = u32::from_be_bytes(plain);
    // В отличие от genkey, тут раунды идут вперёд: ki = 0, 1, 2, 3.
    for ki in 0..4 {
        state = feistel_round(state, KEYTABLE_ENCRYPT[ki]);
    }
    state = final_word_swap(state);
    state.to_be_bytes()
}

/// **Encrypt full buffer** для SID 0x36 payload: разбить на u32-words и
/// зашифровать каждый отдельно. Длина `plain` должна быть кратна 4.
/// Возвращает новый Vec того же размера; padding pad-ит вызывающая сторона.
pub fn subaru_encrypt_buffer(plain: &[u8]) -> Vec<u8> {
    assert!(
        plain.len() % 4 == 0,
        "subaru_encrypt_buffer: length must be 4-byte aligned (got {})",
        plain.len(),
    );
    let mut out = Vec::with_capacity(plain.len());
    for chunk in plain.chunks_exact(4) {
        let word: [u8; 4] = chunk.try_into().unwrap();
        out.extend_from_slice(&subaru_encrypt_word(word));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Тест-векторы получены прогоном эталона `fenugrec/nisprog/ssm_backend.c::sub_genkey`
    // через standalone C-харнес (см. `tools/seed_key_harness.c` в репо).
    // Если меняешь KEYTABLE или INDEX_TRANSFORMATION — нужно ре-генерировать!
    // ------------------------------------------------------------------

    /// Каждая запись: `(seed_be, expected_key_be)`. Прогоняем 1:1 — bit-perfect.
    /// Сгенерировано `tools/seed_key_harness.c` против эталона
    /// `nisprog/ssm_backend.c::sub_genkey`.
    const GENKEY_VECTORS: &[([u8; 4], [u8; 4])] = &[
        ([0x00, 0x00, 0x00, 0x00], [0x2A, 0x52, 0xF9, 0x63]),
        ([0xFF, 0xFF, 0xFF, 0xFF], [0x5A, 0xD3, 0x26, 0x75]),
        ([0xDE, 0xAD, 0xBE, 0xEF], [0xEC, 0x4D, 0xBB, 0xB1]),
        ([0x12, 0x34, 0x56, 0x78], [0xE7, 0x8F, 0x1F, 0x21]),
        ([0xCA, 0xFE, 0xBA, 0xBE], [0x82, 0xF6, 0xFC, 0xA2]),
        ([0x4E, 0x42, 0x50, 0x40], [0x4A, 0x6C, 0x8D, 0x29]), // первые 4 байта нашего ROM ID
        ([0xA2, 0x10, 0x11, 0x00], [0xAA, 0x10, 0x69, 0x7D]),
        ([0x80, 0x00, 0x00, 0x00], [0x68, 0xD5, 0x3C, 0x5A]),
        ([0x01, 0x00, 0x00, 0x00], [0x84, 0xB1, 0x3B, 0x2A]),
        ([0x00, 0x00, 0x00, 0x01], [0x11, 0x87, 0xE8, 0xF6]),
    ];

    /// Same format, but for `sub_encrypt` (SID 0x36 word-encryption).
    const ENCRYPT_VECTORS: &[([u8; 4], [u8; 4])] = &[
        ([0x00, 0x00, 0x00, 0x00], [0xE8, 0x91, 0xF5, 0x06]),
        ([0xFF, 0xFF, 0xFF, 0xFF], [0xA7, 0x01, 0xCF, 0x83]),
        ([0xDE, 0xAD, 0xBE, 0xEF], [0xFC, 0xA8, 0x3E, 0x72]),
        ([0x12, 0x34, 0x56, 0x78], [0xE5, 0x2F, 0x0B, 0x59]),
        ([0xCA, 0xFE, 0xBA, 0xBE], [0x0F, 0xCD, 0x95, 0x75]),
        ([0x4E, 0x42, 0x50, 0x40], [0x88, 0xC3, 0xEC, 0xAE]),
        ([0xA2, 0x10, 0x11, 0x00], [0x2B, 0xFA, 0x50, 0xB9]),
        ([0x80, 0x00, 0x00, 0x00], [0xFC, 0x8D, 0x75, 0x86]),
        ([0x01, 0x00, 0x00, 0x00], [0x0B, 0x23, 0xF4, 0x13]),
        ([0x00, 0x00, 0x00, 0x01], [0x77, 0x79, 0x91, 0x22]),
    ];

    #[test]
    fn genkey_known_vectors() {
        for &(seed, expected) in GENKEY_VECTORS {
            let got = subaru_genkey(seed);
            assert_eq!(
                got, expected,
                "subaru_genkey({:02X?}) → {:02X?}, expected {:02X?}",
                seed, got, expected,
            );
        }
    }

    #[test]
    fn encrypt_known_vectors() {
        for &(plain, expected) in ENCRYPT_VECTORS {
            let got = subaru_encrypt_word(plain);
            assert_eq!(
                got, expected,
                "subaru_encrypt_word({:02X?}) → {:02X?}, expected {:02X?}",
                plain, got, expected,
            );
        }
    }

    #[test]
    fn genkey_is_deterministic() {
        // Тривиальное свойство: одинаковый seed → одинаковый key.
        let seed = [0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(subaru_genkey(seed), subaru_genkey(seed));
    }

    #[test]
    fn genkey_zero_seed_does_not_panic() {
        let _ = subaru_genkey([0; 4]);
        let _ = subaru_genkey([0xFF; 4]);
    }

    #[test]
    fn encrypt_buffer_round_trips_length() {
        let plain = vec![0x12, 0x34, 0x56, 0x78,  0xAA, 0xBB, 0xCC, 0xDD];
        let enc   = subaru_encrypt_buffer(&plain);
        assert_eq!(enc.len(), plain.len());
        // Каждый 4-байтный word шифруется независимо.
        let word0 = subaru_encrypt_word([0x12, 0x34, 0x56, 0x78]);
        let word1 = subaru_encrypt_word([0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(&enc[0..4], &word0);
        assert_eq!(&enc[4..8], &word1);
    }

    #[test]
    fn final_word_swap_swaps_halves() {
        assert_eq!(final_word_swap(0x1234_5678), 0x5678_1234);
        assert_eq!(final_word_swap(0xFFFF_0000), 0x0000_FFFF);
    }

    /// CAN-side genkey против live captures from `4E42504007` ECU.
    /// Round-keys найдены в ROM @ 0x05972C через `tools/find_round_keys.py`.
    #[test]
    fn can_genkey_matches_live_captures() {
        let pairs: &[([u8; 4], [u8; 4])] = &[
            ([0xDD, 0xEE, 0xAB, 0x05], [0x74, 0x15, 0x7C, 0x7D]),
            ([0x0D, 0x13, 0x32, 0x08], [0xFD, 0xB2, 0x52, 0x38]),
            ([0x0F, 0xCE, 0x53, 0xBB], [0x3F, 0x97, 0xAD, 0x93]),
            ([0xA0, 0xE6, 0xDF, 0xD5], [0x5E, 0xB8, 0xE0, 0x6A]),
            ([0x25, 0xA4, 0xF1, 0x81], [0xE7, 0x6B, 0x34, 0x34]),
            ([0xAB, 0x2C, 0xEB, 0xA1], [0x8D, 0x36, 0x6E, 0x24]),
            ([0x7D, 0xAF, 0x7C, 0xBB], [0xE4, 0x3B, 0x52, 0xE2]),
            ([0xBF, 0x78, 0xFD, 0x02], [0x9A, 0x25, 0x62, 0x16]),
        ];
        for &(seed, expected) in pairs {
            let got = subaru_genkey_can(seed);
            assert_eq!(
                got, expected,
                "subaru_genkey_can({:02X?}) = {:02X?}, expected {:02X?}",
                seed, got, expected,
            );
        }
    }
}
