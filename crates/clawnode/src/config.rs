//! Node configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{NodeError, NodeResult};

/// Configuration for the clawnode VPS agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// OpenClaw gateway WebSocket URL (e.g. wss://gateway.example.com)
    pub gateway: String,

    /// Auth token for the gateway
    pub token: Option<String>,

    /// Hostname / display name for this node
    pub hostname: String,

    /// Provider (hetzner, vultr, contabo, hostinger, digitalocean)
    pub provider: String,

    /// Region (e.g. eu-hetzner-nbg1)
    pub region: String,

    /// Instance tier (nano, standard, pro, enterprise)
    pub tier: String,

    /// Instance role (primary, standby)
    pub role: String,

    /// Account ID
    pub account_id: String,

    /// Path to persistent state directory
    pub state_path: PathBuf,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_secs: u64,

    /// Reconnect delay in seconds
    #[serde(default = "default_reconnect")]
    pub reconnect_delay_secs: u64,

    /// Arbitrary key-value labels
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_heartbeat() -> u64 {
    30
}

fn default_reconnect() -> u64 {
    5
}

impl NodeConfig {
    pub fn load(path: &Path) -> NodeResult<Self> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| NodeError::Config(format!("read {}: {e}", path.display())))?;
        serde_json::from_str(&data)
            .map_err(|e| NodeError::Config(format!("parse {}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> NodeResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            gateway: "wss://localhost:18789".to_string(),
            token: None,
            hostname: "clawnode".to_string(),
            provider: "hetzner".to_string(),
            region: "eu-hetzner-nbg1".to_string(),
            tier: "standard".to_string(),
            role: "primary".to_string(),
            account_id: String::new(),
            state_path: PathBuf::from("/var/lib/clawnode"),
            heartbeat_interval_secs: 30,
            reconnect_delay_secs: 5,
            labels: HashMap::new(),
        }
    }
}
