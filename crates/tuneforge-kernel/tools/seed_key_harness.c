/*
 * seed_key_harness.c — генератор bit-perfect test vectors для
 * src/seed_key.rs (`subaru_genkey`, `subaru_encrypt_word`).
 *
 * Содержимое функций sub_genkey() и sub_encrypt() — verbatim из
 * fenugrec/nisprog/ssm_backend.c (GPL-3.0). Этот файл компилируется
 * сам в standalone-binary и печатает (seed, key) пары как готовые
 * Rust-литералы для вставки в `GENKEY_VECTORS` / `ENCRYPT_VECTORS`.
 *
 * Сборка:
 *   cc -O2 -Wall -Wextra -o /tmp/seed_key_harness seed_key_harness.c
 *
 * Запуск:
 *   /tmp/seed_key_harness    # печатает блоки Rust-кода в stdout
 *
 * License: GPL-3.0-or-later (наследуется от исходных функций).
 */

#include <stdio.h>
#include <stdint.h>
#include <string.h>

/* --- verbatim helpers из nisprog/stypes.h ------------------------------- */

static uint32_t reconst_32(const uint8_t *d) {
    return ((uint32_t)d[0] << 24) | ((uint32_t)d[1] << 16)
         | ((uint32_t)d[2] << 8)  |  (uint32_t)d[3];
}

static void write_32b(uint32_t val, uint8_t *out) {
    out[0] = (val >> 24) & 0xFF;
    out[1] = (val >> 16) & 0xFF;
    out[2] = (val >>  8) & 0xFF;
    out[3] =  val        & 0xFF;
}

/* --- verbatim из nisprog/ssm_backend.c, функция sub_genkey() ------------ */

static void sub_genkey(const uint8_t *seed8, uint8_t *key) {
    uint32_t seed, index;
    uint16_t wordtogenerateindex, wordtobeencrypted, encryptionkey;
    int ki, n;

    const uint16_t keytogenerateindex[]={
        0x53DA, 0x33BC, 0x72EB, 0x437D,
        0x7CA3, 0x3382, 0x834F, 0x3608,
        0xAFB8, 0x503D, 0xDBA3, 0x9D34,
        0x3563, 0x6B70, 0x6E74, 0x88F0
    };

    const uint8_t indextransformation[]={
        0x5, 0x6, 0x7, 0x1, 0x9, 0xC, 0xD, 0x8,
        0xA, 0xD, 0x2, 0xB, 0xF, 0x4, 0x0, 0x3,
        0xB, 0x4, 0x6, 0x0, 0xF, 0x2, 0xD, 0x9,
        0x5, 0xC, 0x1, 0xA, 0x3, 0xD, 0xE, 0x8
    };

    seed = reconst_32(seed8);

    for (ki = 15; ki >= 0; ki--) {
        wordtogenerateindex = seed;
        wordtobeencrypted   = seed >> 16;
        index = wordtogenerateindex ^ keytogenerateindex[ki];
        index += index << 16;
        encryptionkey = 0;
        for (n = 0; n < 4; n++) {
            encryptionkey += indextransformation[(index >> (n * 4)) & 0x1F] << (n * 4);
        }
        encryptionkey = (encryptionkey >> 3) + (encryptionkey << 13);
        seed = (encryptionkey ^ wordtobeencrypted) + (wordtogenerateindex << 16);
    }

    seed = (seed >> 16) + (seed << 16);
    write_32b(seed, key);
}

/* --- verbatim из nisprog/ssm_backend.c, функция sub_encrypt() ----------- */

static void sub_encrypt(const uint8_t *datatoencrypt, uint8_t *encrypteddata) {
    uint32_t datatoencrypt32, index;
    uint16_t wordtogenerateindex, wordtobeencrypted, encryptionkey;
    int ki, n;

    const uint16_t keytogenerateindex[]={
        0x7856, 0xCE22, 0xF513, 0x6E86
    };

    const uint8_t indextransformation[]={
        0x5, 0x6, 0x7, 0x1, 0x9, 0xC, 0xD, 0x8,
        0xA, 0xD, 0x2, 0xB, 0xF, 0x4, 0x0, 0x3,
        0xB, 0x4, 0x6, 0x0, 0xF, 0x2, 0xD, 0x9,
        0x5, 0xC, 0x1, 0xA, 0x3, 0xD, 0xE, 0x8
    };

    datatoencrypt32 = reconst_32(datatoencrypt);

    for (ki = 0; ki < 4; ki++) {
        wordtogenerateindex = datatoencrypt32;
        wordtobeencrypted   = datatoencrypt32 >> 16;
        index = wordtogenerateindex ^ keytogenerateindex[ki];
        index += index << 16;
        encryptionkey = 0;
        for (n = 0; n < 4; n++) {
            encryptionkey += indextransformation[(index >> (n * 4)) & 0x1F] << (n * 4);
        }
        encryptionkey = (encryptionkey >> 3) + (encryptionkey << 13);
        datatoencrypt32 = (encryptionkey ^ wordtobeencrypted) + (wordtogenerateindex << 16);
    }

    datatoencrypt32 = (datatoencrypt32 >> 16) + (datatoencrypt32 << 16);
    write_32b(datatoencrypt32, encrypteddata);
}

/* --- main: генерируем Rust-литералы ------------------------------------- */

int main(void) {
    /* Репрезентативный набор seed-ов: edge cases + псевдо-случайные. */
    static const uint8_t seeds[][4] = {
        {0x00, 0x00, 0x00, 0x00}, /* all zeros */
        {0xFF, 0xFF, 0xFF, 0xFF}, /* all ones */
        {0xDE, 0xAD, 0xBE, 0xEF}, /* tradition */
        {0x12, 0x34, 0x56, 0x78}, /* incrementing nibbles */
        {0xCA, 0xFE, 0xBA, 0xBE},
        {0x4E, 0x42, 0x50, 0x40}, /* первые 4 байта ROM ID нашей машины */
        {0xA2, 0x10, 0x11, 0x00}, /* SSM ID + pad */
        {0x80, 0x00, 0x00, 0x00},
        {0x01, 0x00, 0x00, 0x00},
        {0x00, 0x00, 0x00, 0x01},
    };
    const int N = sizeof(seeds) / sizeof(seeds[0]);
    uint8_t out[4];

    printf("// === GENKEY_VECTORS ===\n");
    printf("// Сгенерировано: tools/seed_key_harness.c против эталона nisprog ssm_backend.c::sub_genkey\n");
    for (int i = 0; i < N; i++) {
        sub_genkey(seeds[i], out);
        printf("([0x%02X, 0x%02X, 0x%02X, 0x%02X], [0x%02X, 0x%02X, 0x%02X, 0x%02X]),\n",
            seeds[i][0], seeds[i][1], seeds[i][2], seeds[i][3],
            out[0], out[1], out[2], out[3]);
    }

    printf("\n// === ENCRYPT_VECTORS ===\n");
    printf("// Сгенерировано: tools/seed_key_harness.c против эталона nisprog ssm_backend.c::sub_encrypt\n");
    for (int i = 0; i < N; i++) {
        sub_encrypt(seeds[i], out);
        printf("([0x%02X, 0x%02X, 0x%02X, 0x%02X], [0x%02X, 0x%02X, 0x%02X, 0x%02X]),\n",
            seeds[i][0], seeds[i][1], seeds[i][2], seeds[i][3],
            out[0], out[1], out[2], out[3]);
    }

    return 0;
}
