//! Внешние датчики (AEM, Innovate, PLX, Phidget…).
//!
//! В Java-RomRaider это плагины из директории `plugins/*`. Здесь — единый
//! трейт-контракт для произвольного источника каналов.

use crate::error::LoggerResult;
use crate::sample::SampleValue;

pub trait ExternalSensor: Send + Sync {
    fn id(&self) -> &str;
    fn channels(&self) -> &[String];
    fn read(&mut self) -> LoggerResult<Vec<SampleValue>>;
}
