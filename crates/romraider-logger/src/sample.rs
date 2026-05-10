use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub timestamp: SystemTime,
    pub values:    Vec<SampleValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleValue {
    pub parameter_id: String,
    pub raw:          Vec<u8>,
    pub value:        f64,
}
