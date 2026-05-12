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

/// **Generate key from seed** для Subaru SID 0x27 securityAccess.
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
}
