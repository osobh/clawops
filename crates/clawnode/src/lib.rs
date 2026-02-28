//! clawnode — ClawOps VPS Node Agent
//!
//! Forked from clawbernetes/crates/clawnode. GPU/container/mesh logic removed.
//! VPS fleet management commands substituted.

#![forbid(unsafe_code)]

pub mod auth_cmd;
pub mod client;
pub mod commands;
pub mod config;
pub mod config_cmd;
pub mod error;
pub mod health_cmd;
pub mod identity;
pub mod persist;
pub mod rolling_push;
pub mod secrets_cmd;
pub mod vps_cmd;

use std::sync::Arc;
use tokio::sync::RwLock;

pub use client::GatewayClient;
pub use config::NodeConfig;

// ─── Node state ───────────────────────────────────────────────────────────────

/// Mutable per-node runtime state.
#[derive(Debug)]
pub struct NodeState {
    pub config: NodeConfig,
    pub connected: bool,
    pub node_id: Option<String>,
    pub node_token: Option<String>,
    pub approved: bool,
    pub capabilities: Vec<String>,
    pub commands: Vec<String>,
}

impl NodeState {
    pub fn new(config: NodeConfig) -> Self {
        // VPS capabilities
        let capabilities = vec![
            "system".to_string(),
            "vps".to_string(),
            "docker".to_string(),
            "tailscale".to_string(),
            "openclaw".to_string(),
            "health".to_string(),
        ];

        let commands = vec![
            // VPS core
            "vps.info".to_string(),
            "vps.status".to_string(),
            "vps.metrics".to_string(),
            "vps.restart".to_string(),
            "system.info".to_string(),
            "system.run".to_string(),
            // OpenClaw
            "openclaw.health".to_string(),
            // Docker
            "docker.status".to_string(),
            "docker.restart".to_string(),
            // Health
            "health.check".to_string(),
            "health.score".to_string(),
            "node.health".to_string(),
            "node.capabilities".to_string(),
            // Config
            "config.create".to_string(),
            "config.get".to_string(),
            "config.set".to_string(),
            "config.update".to_string(),
            "config.delete".to_string(),
            "config.list".to_string(),
            // Secrets
            "secret.create".to_string(),
            "secret.get".to_string(),
            "secret.delete".to_string(),
            "secret.list".to_string(),
            "secret.rotate".to_string(),
            // Auth & audit
            "auth.create_key".to_string(),
            "auth.revoke_key".to_string(),
            "auth.list_keys".to_string(),
            "audit.query".to_string(),
        ];

        Self {
            config,
            connected: false,
            node_id: None,
            node_token: None,
            approved: false,
            capabilities,
            commands,
        }
    }
}

// ─── Shared state ─────────────────────────────────────────────────────────────

/// Shared state — passed by reference into every command handler.
pub struct SharedState {
    inner: Arc<RwLock<NodeState>>,
    pub capabilities: Vec<String>,
    pub commands: Vec<String>,
    pub node_token: Option<String>,

    // ─── Persistence stores ───────────────────────────────────────────────
    pub vps_store: Arc<RwLock<persist::VpsInstanceStore>>,
    pub event_store: Arc<RwLock<persist::EventStore>>,
    pub secret_store: Arc<RwLock<claw_secrets::SecretStore>>,
    pub config_store: Arc<RwLock<claw_config::ConfigStore>>,
    pub api_key_store: Arc<RwLock<claw_auth::ApiKeyStore>>,
    pub audit_log_store: Arc<RwLock<claw_auth::AuditLogStore>>,
}

impl SharedState {
    pub fn new(config: NodeConfig) -> Self {
        let state_path = config.state_path.clone();
        let state = NodeState::new(config);

        let capabilities = state.capabilities.clone();
        let commands = state.commands.clone();

        Self {
            capabilities,
            commands,
            node_token: None,
            inner: Arc::new(RwLock::new(state)),
            vps_store: Arc::new(RwLock::new(persist::VpsInstanceStore::new(&state_path))),
            event_store: Arc::new(RwLock::new(persist::EventStore::new(&state_path))),
            secret_store: Arc::new(RwLock::new(claw_secrets::SecretStore::new(&state_path))),
            config_store: Arc::new(RwLock::new(claw_config::ConfigStore::new(&state_path))),
            api_key_store: Arc::new(RwLock::new(claw_auth::ApiKeyStore::new(&state_path))),
            audit_log_store: Arc::new(RwLock::new(claw_auth::AuditLogStore::new(&state_path))),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, NodeState> {
        self.inner.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, NodeState> {
        self.inner.write().await
    }
}

/// Create shared state from config.
pub fn create_state(config: NodeConfig) -> SharedState {
    SharedState::new(config)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_test_config(state_path: std::path::PathBuf) -> NodeConfig {
        NodeConfig {
            gateway: "wss://localhost:18789".to_string(),
            token: None,
            hostname: "test-node".to_string(),
            provider: "hetzner".to_string(),
            region: "eu-hetzner-nbg1".to_string(),
            tier: "standard".to_string(),
            role: "primary".to_string(),
            account_id: "acct-test".to_string(),
            state_path,
            heartbeat_interval_secs: 30,
            reconnect_delay_secs: 5,
            labels: HashMap::new(),
        }
    }

    // ── NodeState ─────────────────────────────────────────────────────────────

    #[test]
    fn node_state_starts_disconnected() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = NodeState::new(cfg);
        assert!(!state.connected);
        assert!(state.node_id.is_none());
        assert!(!state.approved);
    }

    #[test]
    fn node_state_capabilities_include_all_vps_domains() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = NodeState::new(cfg);
        for cap in &["vps", "docker", "health", "system", "openclaw", "tailscale"] {
            assert!(
                state.capabilities.contains(&cap.to_string()),
                "missing capability: {cap}"
            );
        }
    }

    #[test]
    fn node_state_commands_cover_all_28_commands() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = NodeState::new(cfg);
        let required = [
            "vps.info",
            "vps.status",
            "vps.metrics",
            "vps.restart",
            "system.info",
            "system.run",
            "openclaw.health",
            "docker.status",
            "docker.restart",
            "health.check",
            "health.score",
            "node.health",
            "node.capabilities",
            "config.create",
            "config.get",
            "config.set",
            "config.update",
            "config.delete",
            "config.list",
            "secret.create",
            "secret.get",
            "secret.delete",
            "secret.list",
            "secret.rotate",
            "auth.create_key",
            "auth.revoke_key",
            "auth.list_keys",
            "audit.query",
        ];
        for cmd in &required {
            assert!(
                state.commands.contains(&cmd.to_string()),
                "missing command: {cmd}"
            );
        }
        assert_eq!(
            state.commands.len(),
            required.len(),
            "command count mismatch"
        );
    }

    // ── NodeConfig ────────────────────────────────────────────────────────────

    #[test]
    fn node_config_default_is_hetzner_standard_primary() {
        let config = NodeConfig::default();
        assert_eq!(config.provider, "hetzner");
        assert_eq!(config.region, "eu-hetzner-nbg1");
        assert_eq!(config.tier, "standard");
        assert_eq!(config.role, "primary");
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.reconnect_delay_secs, 5);
        assert!(config.token.is_none());
        assert!(config.labels.is_empty());
    }

    #[test]
    fn node_config_serialize_deserialize_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let original = make_test_config(dir.path().to_path_buf());
        let json = serde_json::to_string(&original).unwrap();
        let loaded: NodeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.hostname, original.hostname);
        assert_eq!(loaded.provider, original.provider);
        assert_eq!(loaded.region, original.region);
        assert_eq!(loaded.account_id, original.account_id);
        assert_eq!(loaded.tier, original.tier);
        assert_eq!(loaded.role, original.role);
    }

    #[test]
    fn node_config_file_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config = NodeConfig {
            hostname: "saved-node".to_string(),
            account_id: "acct-saved-001".to_string(),
            state_path: dir.path().to_path_buf(),
            ..NodeConfig::default()
        };
        config.save(&config_path).unwrap();
        let loaded = NodeConfig::load(&config_path).unwrap();
        assert_eq!(loaded.hostname, "saved-node");
        assert_eq!(loaded.account_id, "acct-saved-001");
        assert_eq!(loaded.provider, "hetzner");
    }

    #[test]
    fn node_config_load_missing_file_returns_error() {
        let result = NodeConfig::load(std::path::Path::new("/nonexistent/path/config.json"));
        assert!(result.is_err());
    }

    // ── SharedState ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn shared_state_initializes_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);
        assert!(!state.capabilities.is_empty());
        assert!(!state.commands.is_empty());
        assert!(state.node_token.is_none());
        let inner = state.read().await;
        assert!(!inner.connected);
        assert!(!inner.approved);
    }

    // ── Command dispatch: unknown ─────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_command_returns_descriptive_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);
        let req = crate::commands::CommandRequest {
            command: "totally.unknown".to_string(),
            params: serde_json::Value::Null,
        };
        let result = crate::commands::handle_command(&state, req).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown command"), "unexpected: {msg}");
    }

    // ── Command dispatch: config ──────────────────────────────────────────────

    #[tokio::test]
    async fn config_create_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let create = crate::commands::CommandRequest {
            command: "config.create".to_string(),
            params: serde_json::json!({
                "name": "app-config",
                "data": { "api_key": "key-abc", "env": "production" },
                "immutable": false
            }),
        };
        let created = crate::commands::handle_command(&state, create)
            .await
            .unwrap();
        assert_eq!(created["ok"], true);
        assert_eq!(created["name"], "app-config");

        let get = crate::commands::CommandRequest {
            command: "config.get".to_string(),
            params: serde_json::json!({ "name": "app-config" }),
        };
        let result = crate::commands::handle_command(&state, get).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["api_key"], "key-abc");
        assert_eq!(result["data"]["env"], "production");
    }

    #[tokio::test]
    async fn config_list_returns_all_created_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        for name in &["alpha", "beta", "gamma"] {
            let req = crate::commands::CommandRequest {
                command: "config.create".to_string(),
                params: serde_json::json!({
                    "name": name, "data": { "v": "1" }, "immutable": false
                }),
            };
            crate::commands::handle_command(&state, req).await.unwrap();
        }

        let list = crate::commands::CommandRequest {
            command: "config.list".to_string(),
            params: serde_json::Value::Null,
        };
        let result = crate::commands::handle_command(&state, list).await.unwrap();
        assert_eq!(result["ok"], true);
        let entries = result["configs"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn config_set_upserts_existing_or_new() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let set_new = crate::commands::CommandRequest {
            command: "config.set".to_string(),
            params: serde_json::json!({ "name": "mykey", "data": { "v": "initial" } }),
        };
        let r = crate::commands::handle_command(&state, set_new)
            .await
            .unwrap();
        assert_eq!(r["ok"], true);

        let get = crate::commands::CommandRequest {
            command: "config.get".to_string(),
            params: serde_json::json!({ "name": "mykey" }),
        };
        let r2 = crate::commands::handle_command(&state, get).await.unwrap();
        assert_eq!(r2["data"]["v"], "initial");
    }

    // ── Command dispatch: secrets ─────────────────────────────────────────────

    #[tokio::test]
    async fn secret_create_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let create = crate::commands::CommandRequest {
            command: "secret.create".to_string(),
            params: serde_json::json!({ "name": "db_password", "value": "hunter2" }),
        };
        let created = crate::commands::handle_command(&state, create)
            .await
            .unwrap();
        assert_eq!(created["ok"], true);
        assert_eq!(created["name"], "db_password");

        let get = crate::commands::CommandRequest {
            command: "secret.get".to_string(),
            params: serde_json::json!({ "name": "db_password" }),
        };
        let result = crate::commands::handle_command(&state, get).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["value"], "hunter2");
    }

    #[tokio::test]
    async fn secret_list_returns_all_secret_names() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        for name in &["k1", "k2", "k3"] {
            let req = crate::commands::CommandRequest {
                command: "secret.create".to_string(),
                params: serde_json::json!({ "name": name, "value": "placeholder" }),
            };
            crate::commands::handle_command(&state, req).await.unwrap();
        }

        let list = crate::commands::CommandRequest {
            command: "secret.list".to_string(),
            params: serde_json::Value::Null,
        };
        let result = crate::commands::handle_command(&state, list).await.unwrap();
        assert_eq!(result["ok"], true);
        let names = result["names"].as_array().unwrap();
        assert_eq!(names.len(), 3);
    }

    #[tokio::test]
    async fn secret_rotate_updates_value() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let create = crate::commands::CommandRequest {
            command: "secret.create".to_string(),
            params: serde_json::json!({ "name": "rotate_me", "value": "old-value" }),
        };
        crate::commands::handle_command(&state, create)
            .await
            .unwrap();

        let rotate = crate::commands::CommandRequest {
            command: "secret.rotate".to_string(),
            params: serde_json::json!({ "name": "rotate_me", "new_value": "new-value" }),
        };
        let r = crate::commands::handle_command(&state, rotate)
            .await
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["rotated"], true);
        assert_eq!(r["key_version"], 2);

        let get = crate::commands::CommandRequest {
            command: "secret.get".to_string(),
            params: serde_json::json!({ "name": "rotate_me" }),
        };
        let r2 = crate::commands::handle_command(&state, get).await.unwrap();
        assert_eq!(r2["value"], "new-value");
    }

    // ── Command dispatch: auth ────────────────────────────────────────────────

    #[tokio::test]
    async fn auth_create_and_list_keys() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let create = crate::commands::CommandRequest {
            command: "auth.create_key".to_string(),
            params: serde_json::json!({ "label": "ci-runner", "scopes": ["read"] }),
        };
        let created = crate::commands::handle_command(&state, create)
            .await
            .unwrap();
        assert_eq!(created["ok"], true);
        assert!(created["key_id"].as_str().is_some());
        assert!(created["secret"].as_str().is_some());
        let key_id = created["key_id"].as_str().unwrap().to_string();

        let list = crate::commands::CommandRequest {
            command: "auth.list_keys".to_string(),
            params: serde_json::Value::Null,
        };
        let result = crate::commands::handle_command(&state, list).await.unwrap();
        let keys = result["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["name"], "ci-runner");
        assert_eq!(keys[0]["key_id"], key_id);
    }

    #[tokio::test]
    async fn auth_revoke_key() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let create = crate::commands::CommandRequest {
            command: "auth.create_key".to_string(),
            params: serde_json::json!({ "label": "temp-key", "scopes": [] }),
        };
        let created = crate::commands::handle_command(&state, create)
            .await
            .unwrap();
        let key_id = created["key_id"].as_str().unwrap().to_string();

        let revoke = crate::commands::CommandRequest {
            command: "auth.revoke_key".to_string(),
            params: serde_json::json!({ "key_id": key_id }),
        };
        let result = crate::commands::handle_command(&state, revoke)
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["key_id"], key_id);
    }

    // ── Command dispatch: audit ───────────────────────────────────────────────

    #[tokio::test]
    async fn audit_query_returns_empty_initially() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let req = crate::commands::CommandRequest {
            command: "audit.query".to_string(),
            params: serde_json::json!({ "limit": 50 }),
        };
        let result = crate::commands::handle_command(&state, req).await.unwrap();
        assert_eq!(result["ok"], true);
        let entries = result["entries"].as_array().unwrap();
        assert!(entries.is_empty());
    }

    // ── Command dispatch: node.capabilities ──────────────────────────────────

    #[tokio::test]
    async fn node_capabilities_returns_full_list() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = make_test_config(dir.path().to_path_buf());
        let state = create_state(cfg);

        let req = crate::commands::CommandRequest {
            command: "node.capabilities".to_string(),
            params: serde_json::Value::Null,
        };
        let result = crate::commands::handle_command(&state, req).await.unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["capabilities"].is_array());
        assert!(result["commands"].is_array());
        let caps = result["capabilities"].as_array().unwrap();
        assert!(!caps.is_empty());
        let cmds = result["commands"].as_array().unwrap();
        assert!(
            cmds.len() >= 25,
            "expected >= 25 commands in capabilities response"
        );
    }
}
