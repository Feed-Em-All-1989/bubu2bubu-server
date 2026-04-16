use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StegoConfig {
    pub aes_iterations: u32,
    pub xor_iterations: u32,
    pub chaos_iterations: u32,
    pub chaos_type: String,
    pub position_method: String,
    pub channel_pattern: String,
    pub bit_plane_ratio: f64,
    pub use_xor: bool,
    pub use_shuffle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StegoMetadata {
    pub salt: String,
    pub nonce: String,
    pub tag: String,
    pub total_bits: usize,
    pub image_dimensions: (usize, usize),
    pub config: StegoConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    #[serde(rename = "join")]
    Join { name: String },
    #[serde(rename = "chat")]
    Chat {
        id: String,
        reply_to: Option<String>,
        image: String,
        metadata: StegoMetadata,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    #[serde(rename = "welcome")]
    Welcome { room_key: String },
    #[serde(rename = "joined")]
    Joined { name: String, online: usize },
    #[serde(rename = "left")]
    Left { name: String, online: usize },
    #[serde(rename = "chat")]
    Chat {
        sender: String,
        id: String,
        reply_to: Option<String>,
        image: String,
        metadata: StegoMetadata,
    },
}
