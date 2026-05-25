use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;
use tracing::{info, warn};
use tuneforge_core::{Address, Endian};
use tuneforge_defs::{CompiledLogParameter, StorageType};
use tuneforge_io::Transport;
use tuneforge_protocol::ssm;

use crate::datalog::DatalogWriter;
use crate::error::{LoggerError, LoggerResult};
use crate::sample::{Sample, SampleValue};

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub poll_interval: Duration,
    pub timeout: Duration,
    pub datalog_dir: Option<PathBuf>,
    pub channel_capacity: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(15), // ~66 Hz, как у Java-логгера на SSM
            timeout: Duration::from_millis(500),
            datalog_dir: None,
            channel_capacity: 1024,
        }
    }
}

/// Управляет циклом опроса. В этой версии:
/// - `poll_once(transport)` — синхронный one-shot: собирает все подписки
///   в один SSM-`read_addresses`-запрос, парсит ответ, возвращает [`Sample`].
/// - `run(transport)` — async loop, рассылает через broadcast-канал.
pub struct LoggerSession {
    cfg: SessionConfig,
    tx: broadcast::Sender<Sample>,
    datalog: Option<DatalogWriter>,
    subscriptions: Vec<CompiledLogParameter>,
}

impl LoggerSession {
    #[must_use]
    pub fn new(cfg: SessionConfig) -> Self {
        let (tx, _rx) = broadcast::channel(cfg.channel_capacity);
        Self {
            cfg,
            tx,
            datalog: None,
            subscriptions: Vec::new(),
        }
    }

    #[must_use]
    pub fn subscribe_channel(&self) -> broadcast::Receiver<Sample> {
        self.tx.subscribe()
    }

    /// Добавить параметр в опрос. Параметр должен быть уже скомпилирован
    /// через [`tuneforge_defs::LogParameter::compile`].
    pub fn subscribe(&mut self, param: CompiledLogParameter) {
        self.subscriptions.push(param);
    }

    #[must_use]
    pub fn subscriptions(&self) -> &[CompiledLogParameter] {
        &self.subscriptions
    }

    pub fn enable_datalog(&mut self, file_name: &str) -> LoggerResult<()> {
        let Some(dir) = self.cfg.datalog_dir.clone() else {
            warn!("datalog dir not configured");
            return Ok(());
        };
        std::fs::create_dir_all(&dir)?;
        self.datalog = Some(DatalogWriter::create(dir.join(file_name))?);
        info!(?self.datalog, "datalog enabled");
        Ok(())
    }

    pub fn datalog_mut(&mut self) -> Option<&mut DatalogWriter> {
        self.datalog.as_mut()
    }

    /// Один цикл опроса: собрать байтовые адреса всех подписок, послать
    /// SSM-`read_addresses`, нарезать ответ по storage-size каждой подписки,
    /// применить scaling, вернуть `Sample` со всеми значениями.
    ///
    /// Endian для SSM-канала всегда `Big` (Subaru convention).
    pub fn poll_once<T: Transport + ?Sized>(&self, transport: &mut T) -> LoggerResult<Sample> {
        if self.subscriptions.is_empty() {
            return Err(LoggerError::NoSubscriptions);
        }
        // 1. Собрать список 1-байтных адресов: для uint16 параметра по адресу X
        //    добавляем X и X+1; SSM read-address отдаёт по байту на адрес.
        let mut addresses: Vec<Address> = Vec::new();
        for sub in &self.subscriptions {
            let stride = sub.storage_type.byte_size() as u32;
            for i in 0..stride {
                addresses.push(Address::new(sub.address.raw() + i));
            }
        }

        // 2. Один SSM read-addresses запрос. pad_byte=0x00 для SSM2.
        let response = ssm::read_addresses(transport, &addresses, 0, self.cfg.timeout)?;
        if response.len() != addresses.len() {
            return Err(LoggerError::Protocol(
                tuneforge_protocol::ProtocolError::ResponseTooShort {
                    got: response.len(),
                    expected: addresses.len(),
                },
            ));
        }

        // 3. Нарезать по подпискам и расшифровать.
        let mut values: Vec<SampleValue> = Vec::with_capacity(self.subscriptions.len());
        let mut cursor = 0usize;
        for sub in &self.subscriptions {
            let size = sub.storage_type.byte_size();
            let chunk = &response[cursor..cursor + size];
            let raw = decode_raw_value(chunk, sub.storage_type, Endian::Big);
            let real = sub.evaluate(raw);
            values.push(SampleValue {
                parameter_id: sub.source.id.clone(),
                raw: chunk.to_vec(),
                value: real,
            });
            cursor += size;
        }

        Ok(Sample {
            timestamp: SystemTime::now(),
            values,
        })
    }

    /// Async loop. На каждой итерации `poll_once` + broadcast в подписчиков
    /// + опционально dump в datalog. При ошибках продолжаем (логируем).
    pub async fn run<T: Transport + Send + ?Sized>(
        &mut self,
        transport: &mut T,
    ) -> LoggerResult<()> {
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        loop {
            tick.tick().await;
            match self.poll_once(transport) {
                Ok(sample) => {
                    if let Some(dl) = self.datalog.as_mut() {
                        if let Err(e) = dl.write_sample(&sample) {
                            warn!(?e, "datalog write failed");
                        }
                    }
                    // Игнорируем `SendError`: значит подписчиков нет.
                    let _ = self.tx.send(sample);
                }
                Err(e) => {
                    warn!(?e, "poll_once failed; continuing");
                }
            }
        }
    }
}

/// Декодировать N байт в `f64` согласно `storage_type`+`endian`. Локальная
/// копия `tuneforge_rom::decode_cells::decode_one` — логгер не должен
/// зависеть от `tuneforge-rom` (концептуально это другой домен).
fn decode_raw_value(bytes: &[u8], storage_type: StorageType, endian: Endian) -> f64 {
    match storage_type {
        StorageType::UInt8 | StorageType::Hex | StorageType::Char => f64::from(bytes[0]),
        StorageType::Int8 => f64::from(bytes[0] as i8),
        StorageType::UInt16 => {
            let arr: [u8; 2] = bytes.try_into().unwrap_or([0; 2]);
            f64::from(endian.read_u16(&arr))
        }
        StorageType::Int16 => {
            let arr: [u8; 2] = bytes.try_into().unwrap_or([0; 2]);
            f64::from(endian.read_u16(&arr) as i16)
        }
        StorageType::UInt32 => {
            let arr: [u8; 4] = bytes.try_into().unwrap_or([0; 4]);
            f64::from(endian.read_u32(&arr))
        }
        StorageType::Int32 => {
            let arr: [u8; 4] = bytes.try_into().unwrap_or([0; 4]);
            f64::from(endian.read_u32(&arr) as i32)
        }
        StorageType::Float => {
            let arr: [u8; 4] = bytes.try_into().unwrap_or([0; 4]);
            f64::from(f32::from_bits(endian.read_u32(&arr)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuneforge_defs::{parse_log_str, LoggerDocument};

    const DEF: &str = r##"
    <ecus>
      <logprotocols>
        <logprotocol type="SSM" default="ssmbase">
          <ecu id="base" name="SSM base" type="ssmbase">
            <parameter id="Engine Speed" offset="#000E" storagetype="uint16"
                       bit="1" byte="1" expr="[value]/4" metric="RPM" desc=""/>
            <parameter id="Throttle" offset="#0015" storagetype="uint8"
                       bit="4" byte="2" expr="[value]*100/255" metric="%" desc=""/>
          </ecu>
        </logprotocol>
      </logprotocols>
    </ecus>
    "##;

    fn build_session(doc: &LoggerDocument) -> LoggerSession {
        let mut s = LoggerSession::new(SessionConfig::default());
        let ecu = doc.find_ecu("base").unwrap();
        s.subscribe(
            ecu.find_parameter("Engine Speed")
                .unwrap()
                .compile()
                .unwrap(),
        );
        s.subscribe(ecu.find_parameter("Throttle").unwrap().compile().unwrap());
        s
    }

    /// Собрать каноничный SSM-ответ на read-addresses с заданными данными.
    fn pack_ssm_response(data_bytes: &[u8]) -> Vec<u8> {
        use tuneforge_protocol::ssm::{Command, ECU_ADDR, HEADER, TOOL_ADDR};
        let mut frame = Vec::new();
        frame.push(HEADER);
        frame.push(TOOL_ADDR);
        frame.push(ECU_ADDR);
        frame.push(u8::try_from(data_bytes.len() + 1).unwrap());
        frame.push(Command::ReadAddress.response_byte());
        frame.extend_from_slice(data_bytes);
        let cksm = frame.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
        frame.push(cksm);
        frame
    }

    #[test]
    fn poll_once_decodes_known_response() {
        // Engine Speed (uint16 BE, expr=[value]/4): 4000 → 1000 RPM
        // Throttle (uint8, expr=[value]*100/255): 128 → ~50.196 %
        let doc = parse_log_str(DEF).unwrap();
        let session = build_session(&doc);

        // 3 1-byte address-ответа: [hi, lo, throttle].
        let payload = [0x0F, 0xA0, 0x80]; // 0x0FA0 = 4000; 0x80 = 128
        let frame = pack_ssm_response(&payload);

        let mut transport = tuneforge_io::mock::MockTransport::with_responses([frame]);

        let sample = session.poll_once(&mut transport).unwrap();
        assert_eq!(sample.values.len(), 2);
        assert_eq!(sample.values[0].parameter_id, "Engine Speed");
        assert_eq!(sample.values[0].raw, vec![0x0F, 0xA0]);
        assert!((sample.values[0].value - 1000.0).abs() < 1e-9);
        assert_eq!(sample.values[1].parameter_id, "Throttle");
        assert_eq!(sample.values[1].raw, vec![0x80]);
        assert!((sample.values[1].value - (128.0 * 100.0 / 255.0)).abs() < 1e-9);
    }

    #[test]
    fn poll_once_without_subscriptions_errors() {
        let session = LoggerSession::new(SessionConfig::default());
        let mut transport = tuneforge_io::mock::MockTransport::new();
        let err = session.poll_once(&mut transport).unwrap_err();
        assert!(matches!(err, LoggerError::NoSubscriptions));
    }

    #[test]
    fn poll_once_builds_correct_request_frame() {
        let doc = parse_log_str(DEF).unwrap();
        let session = build_session(&doc);

        let payload = [0x00, 0x00, 0x00]; // 3 bytes для 2-х подписок (uint16 + uint8)
        let frame = pack_ssm_response(&payload);
        let mut transport = tuneforge_io::mock::MockTransport::with_responses([frame]);

        let _ = session.poll_once(&mut transport).unwrap();

        // Запрос должен запросить 3 адреса: 0x000E, 0x000F (Engine Speed uint16) и 0x0015 (Throttle).
        let request = transport.last_write().unwrap();
        // request: 80 10 F0 <len> A8 00 <addr1: 3B> <addr2: 3B> <addr3: 3B> <ck>
        // len = 1 + 1 (pad) + 9 (3 addr * 3B) = 11 = 0x0B
        assert_eq!(request[0..4], [0x80, 0x10, 0xF0, 0x0B]);
        assert_eq!(request[4], 0xA8); // ReadAddress
        assert_eq!(request[5], 0x00); // pad
                                      // Адрес 0x000E
        assert_eq!(request[6..9], [0x00, 0x00, 0x0E]);
        // Адрес 0x000F
        assert_eq!(request[9..12], [0x00, 0x00, 0x0F]);
        // Адрес 0x0015
        assert_eq!(request[12..15], [0x00, 0x00, 0x15]);
    }
}
