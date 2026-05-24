//! High-level orchestrator: «открыть → залить kernel → стримить ROM → завершить».
//!
//! Единственная функция верхнего уровня — [`dump_rom_via_kernel`]. Внутри
//! она делает:
//!
//! 1. [`upload::start_communication`] — KWP2000 startup
//! 2. [`upload::unlock`] — seed/key handshake (SID 0x27)
//! 3. [`upload::start_programming_session`] — SID 0x10 85 02
//! 4. [`upload::upload_and_jump`] — заливка kernel-binary + trailer + SID 0x31
//! 5. [`kernel_wire::handshake`] — kernel-side приветствие (SID 0x81 → C1 …)
//! 6. *(опционально)* [`kernel_wire::set_kernel_speed`] для повышения baud
//! 7. [`kernel_wire::dump_blocks`] в цикле по диапазону + прогресс-callback
//! 8. [`kernel_wire::kernel_reset`] — чистый exit (ECU restart)
//!
//! **NB: переключение хост-baud-rate** (на 15625 после programming session,
//! и на ~62500 после kernel handshake) пока **не делается** — нужно расширение
//! `TactrixTransport` API. Без него весь дамп идёт на 4800 baud
//! (~480 B/s после kernel) и занимает ~35 мин для 1 МБ ROM. С baud-switch
//! ускоряется в ~13× — это TODO.

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{KernelError, KernelResult};
use crate::kernel_wire::{self, DumpArea};
use crate::kernels::{KernelBinary, McuFamily};
use crate::upload;

#[derive(Debug, Clone)]
pub struct KernelDumpConfig {
    pub mcu: McuFamily,
    pub start_addr: u32,
    pub length: usize,
    /// Опциональный высокий baud для kernel-режима. `None` = остаётся на 4800.
    /// Поддерживается будущим расширением; сейчас игнорируется.
    pub fast_baud: Option<u32>,
    /// Таймаут на отдельный round-trip (SID-запрос → ответ).
    pub timeout: Duration,
}

impl Default for KernelDumpConfig {
    fn default() -> Self {
        Self {
            mcu: McuFamily::Sh7058,
            start_addr: 0x0000_0000,
            length: McuFamily::Sh7058.rom_size(),
            // 62 500 baud (BRR=9) — рекомендованное nisprog значение, в ~13× быстрее
            // 4800 баз. Дамп 1 МБ занимает ~3 мин.
            fast_baud: Some(62_500),
            timeout: Duration::from_secs(2),
        }
    }
}

/// Сколько 32-байтных блоков за один SID_DUMP. 64 = 2 KiB за раз — баланс между
/// округлым прогрессом и round-trip overhead.
const BLOCKS_PER_BATCH: u16 = 64;

/// Подобрать встроенный kernel-binary для MCU. Требует соответствующий feature-flag.
fn kernel_for(mcu: McuFamily) -> KernelResult<KernelBinary> {
    match mcu {
        McuFamily::Sh7058 => {
            #[cfg(feature = "sh7058")]
            {
                Ok(crate::kernels::KERNEL_SH7058)
            }
            #[cfg(not(feature = "sh7058"))]
            {
                Err(KernelError::UnsupportedMcu(mcu))
            }
        }
        McuFamily::Sh7055 => {
            #[cfg(feature = "sh7055")]
            {
                Ok(crate::kernels::KERNEL_SH7055)
            }
            #[cfg(not(feature = "sh7055"))]
            {
                Err(KernelError::UnsupportedMcu(mcu))
            }
        }
    }
}

/// Дампит ROM через kernel-upload-путь. Возвращает full byte-buffer длины
/// `cfg.length`. `progress(done, total)` зовётся после каждого SID_DUMP batch.
pub fn dump_rom_via_kernel(
    transport: &mut dyn Transport,
    cfg: &KernelDumpConfig,
    mut progress: impl FnMut(usize, usize),
) -> KernelResult<Vec<u8>> {
    if cfg.start_addr % 32 != 0 {
        return Err(KernelError::UploadAborted(format!(
            "start_addr 0x{:06X} not aligned to 32 bytes (kernel SID_DUMP requires alignment)",
            cfg.start_addr,
        )));
    }

    let kernel = kernel_for(cfg.mcu)?;

    // ----- Step 0: разбудить ECU через SSM ecu_init -----
    // Subaru ECU из idle state не отвечает напрямую на KWP2000 SID 0x81 —
    // нужен хотя бы один валидный SSM-frame для активации диагностического
    // server-а в ECU. SSM ecu_init безопасен (read-only) и заодно
    // подтверждает что ROM ID соответствует ожидаемому.
    tracing::info!("step 0: SSM ecu_init (wake-up + sanity-check)");
    let init_response = romraider_protocol::ssm::ecu_init(transport, cfg.timeout)
        .map_err(|e| KernelError::UploadAborted(format!("SSM ecu_init failed: {e}")))?;
    tracing::info!(
        rom_id = format!("{:02X?}", &init_response.rom_id),
        "ECU online via SSM",
    );

    // ----- Step 1-3: KWP2000 unlock + programming session -----
    tracing::info!("step 1: SID 0x81 start communication");
    upload::start_communication(transport, cfg.timeout)?;

    tracing::info!("step 2: SID 0x27 security access (seed/key)");
    upload::unlock(transport, cfg.timeout)?;

    tracing::info!("step 3: SID 0x10 0x85 0x02 start programming session");
    upload::start_programming_session(transport, cfg.timeout)?;

    // ----- Step 4: Upload kernel + jump -----
    tracing::info!(
        kernel = %kernel.identifier, bytes = kernel.bytes.len(),
        "step 4: SID 0x34 + 0x36 upload + SID 0x31 jump",
    );
    upload::upload_and_jump(transport, &kernel, cfg.timeout)?;

    // ----- Step 5: kernel-side handshake -----
    tracing::info!("step 5: kernel handshake");
    kernel_wire::handshake(transport, cfg.timeout)?;

    // ----- Step 6: host + kernel baud switch -----
    if let Some(target_baud) = cfg.fast_baud {
        let Some(brr_div) = kernel_wire::brr_from_kspeed(target_baud) else {
            return Err(KernelError::UploadAborted(format!(
                "fast_baud {target_baud} вне диапазона SH7058 SCI",
            )));
        };
        tracing::info!(target_baud, brr_div, "step 6: switching to faster baud");
        let real_baud = kernel_wire::set_kernel_speed(transport, brr_div, cfg.timeout)?;
        tracing::info!(real_baud, "step 6 OK");
        // Дать line settle после изменения baud-а перед первым SID_DUMP.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ----- Step 7: SID_DUMP loop -----
    tracing::info!(
        start = format!("0x{:06X}", cfg.start_addr),
        length = cfg.length,
        "step 7: streaming ROM via SID_DUMP (0xBD)",
    );
    let result = stream_rom(transport, cfg, &mut progress);

    // ----- Step 8: чистый exit (даже если read fail-нул выше) -----
    if let Err(e) = kernel_wire::kernel_reset(transport, cfg.timeout) {
        tracing::warn!(?e, "kernel_reset returned error; ECU will time out anyway");
    }

    result
}

/// Внутренний цикл: SID_DUMP-batch-ами, прогресс на каждый batch.
fn stream_rom(
    transport: &mut dyn Transport,
    cfg: &KernelDumpConfig,
    progress: &mut impl FnMut(usize, usize),
) -> KernelResult<Vec<u8>> {
    let mut out = Vec::with_capacity(cfg.length);
    let mut addr = cfg.start_addr;
    progress(0, cfg.length);

    while out.len() < cfg.length {
        let remaining = cfg.length - out.len();
        let remaining_blocks = remaining.div_ceil(32);
        let blocks_this = (remaining_blocks.min(usize::from(BLOCKS_PER_BATCH))) as u16;

        let chunk =
            kernel_wire::dump_blocks(transport, DumpArea::Rom, addr, blocks_this, cfg.timeout)?;
        // Последний batch может вернуть больше байт чем нам нужно — обрезаем.
        let take = chunk.len().min(remaining);
        out.extend_from_slice(&chunk[..take]);
        addr += chunk.len() as u32;
        progress(out.len(), cfg.length);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_targets_sh7058_full_rom() {
        let cfg = KernelDumpConfig::default();
        assert_eq!(cfg.mcu, McuFamily::Sh7058);
        assert_eq!(cfg.start_addr, 0);
        assert_eq!(cfg.length, 1024 * 1024);
    }

    #[test]
    fn unaligned_start_rejected_early() {
        // Не нужен реальный transport — fail при первом check-е.
        struct DummyTransport;
        impl romraider_io::transport::Transport for DummyTransport {
            fn write_all(&mut self, _: &[u8], _: Duration) -> romraider_io::error::IoResult<()> {
                Ok(())
            }
            fn read_frame(
                &mut self,
                _: &mut [u8],
                _: Duration,
            ) -> romraider_io::error::IoResult<usize> {
                Ok(0)
            }
            fn purge(&mut self) -> romraider_io::error::IoResult<()> {
                Ok(())
            }
            fn description(&self) -> &'static str {
                "dummy"
            }
        }
        let cfg = KernelDumpConfig {
            start_addr: 0x100, // aligned
            length: 32,
            ..Default::default()
        };
        let _ = cfg; // structural OK
        let bad = KernelDumpConfig {
            start_addr: 0x101,
            length: 32,
            ..Default::default()
        };
        let mut tr = DummyTransport;
        let err = dump_rom_via_kernel(&mut tr, &bad, |_, _| {}).unwrap_err();
        assert!(matches!(err, KernelError::UploadAborted(_)));
    }
}
