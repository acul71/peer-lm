use serde::{Deserialize, Serialize};

pub const CAPS_PROTOCOL: &str = "/peer-lm/caps/1.0.0";
pub const INFER_PROTOCOL: &str = "/peer-lm/infer/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerLmRequest {
    Caps,
    Infer {
        body: Vec<u8>,
        stream: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerLmResponse {
    Caps(CapsInfo),
    Infer(InferResponse),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsInfo {
    pub models: Vec<String>,
    pub engine: String,
    pub slots_free: u32,
    pub max_ctx: u32,
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}
