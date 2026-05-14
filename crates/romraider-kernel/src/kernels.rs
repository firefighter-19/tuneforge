//! Bundled kernel binaries (из `npkern/precompiled/`, GPL-3.0) и метаданные.
//!
//! Включаются в final-binary через `include_bytes!` — никакого file-IO в
//! runtime. Размер каждого kernel'а ~1-4 KB, не влияет на сборку.

/// MCU-семья ECU. Определяет load-address и какой kernel-binary использовать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McuFamily {
    /// Renesas SH7055 — 512 KiB ROM. Subaru ~2003-2006 (GD-chassis WRX/STI).
    /// Kernel грузится в `0xFFFF6000`, entry-trampoline в `0xFFFF6004`.
    Sh7055,
    /// Renesas SH7058 — 1 MiB ROM. Subaru ~2006-2008 (SG-chassis Forester XT,
    /// GD post-MY06).
    /// Kernel грузится в `0xFFFF3000`, entry-trampoline в `0xFFFF3004`.
    Sh7058,
}

impl McuFamily {
    /// RAM-адрес, по которому ECU ждёт kernel-binary.
    pub const fn load_address(self) -> u32 {
        match self {
            Self::Sh7055 => 0xFFFF_6000,
            Self::Sh7058 => 0xFFFF_3000,
        }
    }

    /// Адрес trampoline для SID 0x31 handover (load_address + 4).
    pub const fn entry_address(self) -> u32 {
        self.load_address() + 4
    }

    /// Полный размер ROM (для default `length` в [`crate::KernelDumpConfig`]).
    pub const fn rom_size(self) -> usize {
        match self {
            Self::Sh7055 => 512 * 1024,
            Self::Sh7058 => 1024 * 1024,
        }
    }
}

/// Прикомпилированный kernel-binary с метаданными для конкретной MCU-семьи.
#[derive(Debug, Clone, Copy)]
pub struct KernelBinary {
    pub mcu: McuFamily,
    pub bytes: &'static [u8],
    /// Имя/версия бинаря (для логов: `"ssmk_SH7058_18"`).
    pub identifier: &'static str,
}

// Bundled binaries — из `kernels/`, скопированные из `fenugrec/npkern/precompiled/`.
// Включены под GPL-3.0 (apstream license).

#[cfg(feature = "sh7058")]
pub const KERNEL_SH7058: KernelBinary = KernelBinary {
    mcu: McuFamily::Sh7058,
    bytes: include_bytes!("../kernels/ssmk_SH7058.bin"),
    identifier: "ssmk_SH7058 (npkern, GPL-3.0)",
};

#[cfg(feature = "sh7055")]
pub const KERNEL_SH7055: KernelBinary = KernelBinary {
    mcu: McuFamily::Sh7055,
    bytes: include_bytes!("../kernels/ssmk_SH7055.bin"),
    identifier: "ssmk_SH7055 (npkern, GPL-3.0)",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_address_offsets_are_consistent() {
        assert_eq!(McuFamily::Sh7055.entry_address(), 0xFFFF_6004);
        assert_eq!(McuFamily::Sh7058.entry_address(), 0xFFFF_3004);
    }

    #[test]
    fn rom_sizes_match_physical_chips() {
        assert_eq!(McuFamily::Sh7055.rom_size(), 0x80_000);
        assert_eq!(McuFamily::Sh7058.rom_size(), 0x100_000);
    }

    #[cfg(feature = "sh7058")]
    #[test]
    fn sh7058_kernel_bundled_size_within_npk_region() {
        // npkern linker script `lkr_subaru_7058.ld` отводит 36K (0xFFFF3000..0xFFFF5000+).
        // Текущий precompiled = 3516 байт; sanity-границы по правилам npkern.
        assert!(
            !KERNEL_SH7058.bytes.is_empty() && KERNEL_SH7058.bytes.len() <= 8 * 1024,
            "SH7058 kernel binary size out of range: {} bytes",
            KERNEL_SH7058.bytes.len(),
        );
    }
}
