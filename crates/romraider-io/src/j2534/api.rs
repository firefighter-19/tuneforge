//! Прямой перенос C-API из `J2534.h` v04.04.
//!
//! Имена и значения совпадают с эталонной спецификацией SAE J2534-1.
//! Использовать напрямую не рекомендуется — берите [`super::Device`].

#![allow(non_camel_case_types, non_snake_case)]

use std::os::raw::{c_char, c_long, c_ulong, c_void};

pub type DeviceId = c_ulong;
pub type ChannelId = c_ulong;
pub type FilterId = c_ulong;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct PassThruMsg {
    pub protocol_id: c_ulong,
    pub rx_status: c_ulong,
    pub tx_flags: c_ulong,
    pub timestamp: c_ulong,
    pub data_size: c_ulong,
    pub extra_data_index: c_ulong,
    pub data: [u8; 4128],
}

impl Default for PassThruMsg {
    fn default() -> Self {
        Self {
            protocol_id: 0,
            rx_status: 0,
            tx_flags: 0,
            timestamp: 0,
            data_size: 0,
            extra_data_index: 0,
            data: [0u8; 4128],
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum ProtocolId {
    J1850Vpw = 1,
    J1850Pwm = 2,
    Iso9141 = 3,
    Iso14230 = 4,
    Can = 5,
    Iso15765 = 6,
    Sci_A_Engine = 7,
    Sci_A_Trans = 8,
    Sci_B_Engine = 9,
    Sci_B_Trans = 10,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum IoctlId {
    GetConfig = 0x01,
    SetConfig = 0x02,
    ReadVbatt = 0x03,
    FiveBaudInit = 0x04,
    FastInit = 0x05,
    ClearTxBuffer = 0x07,
    ClearRxBuffer = 0x08,
    ClearPeriodicMsgs = 0x09,
    ClearMsgFilters = 0x0A,
    ClearFunctMsgLookupTable = 0x0B,
    AddToFunctMsgLookupTable = 0x0C,
    DeleteFromFunctMsgLookupTable = 0x0D,
    ReadProgVoltage = 0x0E,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum ConnectFlags {
    None = 0,
    CanIdBoth = 0x800,
    Iso9141Nokey = 0x1000,
    Iso9141Kreceive = 0x2000,
    Iso9141Kresponse = 0x4000,
}

/// Сигнатуры функций PassThru — заполняются из библиотеки `libloading`.
pub struct PassThruVtable {
    pub PassThruOpen: unsafe extern "system" fn(*const c_void, *mut DeviceId) -> c_long,
    pub PassThruClose: unsafe extern "system" fn(DeviceId) -> c_long,
    pub PassThruConnect:
        unsafe extern "system" fn(DeviceId, c_ulong, c_ulong, c_ulong, *mut ChannelId) -> c_long,
    pub PassThruDisconnect: unsafe extern "system" fn(ChannelId) -> c_long,
    pub PassThruReadMsgs:
        unsafe extern "system" fn(ChannelId, *mut PassThruMsg, *mut c_ulong, c_ulong) -> c_long,
    pub PassThruWriteMsgs:
        unsafe extern "system" fn(ChannelId, *const PassThruMsg, *mut c_ulong, c_ulong) -> c_long,
    pub PassThruIoctl:
        unsafe extern "system" fn(c_ulong, c_ulong, *mut c_void, *mut c_void) -> c_long,
    pub PassThruGetLastError: unsafe extern "system" fn(*mut c_char) -> c_long,
}
