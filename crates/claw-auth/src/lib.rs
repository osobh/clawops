//! API key management and audit logging for ClawOps RBAC.
//!
//! Provides [`ApiKeyStore`] with SHA-256 hashed secrets and [`AuditLogStore`]
//! for tracking all access and operations.

#![forbid(unsafe_code)]

use claw_persist::JsonStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────
// API Key Store
// ─────────────────────────────────────────────────────────────

/// An API key record for RBAC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub name: String,
    pub secret_hash: String,
    pub scopes: Vec<String>,
    pub role: String,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
}

/// In-memory API key store backed by JSON snapshots.
pub struct ApiKeyStore {
    keys: HashMap<String, ApiKeyRecord>,
    store: JsonStore,
}

impl ApiKeyStore {
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "apikeys");
        let keys = store.load();
        debug!(count = keys.len(), "loaded API keys from disk");
        Self { keys, store }
    }

    pub fn create(&mut self, record: ApiKeyRecord) -> Result<(), String> {
        if self.keys.contains_key(&record.key_id) {
            return Err(format!("key '{}' already exists", record.key_id));
        }
        let id = record.key_id.clone();
        self.keys.insert(id, record);
        self.snapshot();
        Ok(())
    }

    pub fn get(&self, key_id: &str) -> Option<&ApiKeyRecord> {
        self.keys.get(key_id)
    }

    pub fn get_mut(&mut self, key_id: &str) -> Option<&mut ApiKeyRecord> {
        self.keys.get_mut(key_id)
    }

    pub fn find_by_hash(&self, secret_hash: &str) -> Option<&ApiKeyRecord> {
        self.keys
            .values()
            .find(|k| k.secret_hash == secret_hash && k.active)
    }

    pub fn revoke(&mut self, key_id: &str) -> Result<(), String> {
        let key = self
            .keys
            .get_mut(key_id)
            .ok_or_else(|| format!("key '{key_id}' not found"))?;
        key.active = false;
        self.snapshot();
        Ok(())
    }

    pub fn delete(&mut self, key_id: &str) -> Option<ApiKeyRecord> {
        let r = self.keys.remove(key_id);
        if r.is_some() {
            self.snapshot();
        }
        r
    }

    pub fn list(&self) -> Vec<&ApiKeyRecord> {
        self.keys.values().collect()
    }

    pub fn update(&mut self, key_id: &str) {
        if self.keys.contains_key(key_id) {
            self.snapshot();
        }
    }

    fn snapshot(&self) {
        if let Err(e) = self.store.save(&self.keys) {
            warn!(error = %e, "failed to snapshot API key store");
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Audit Log Store
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub resource_id: Option<String>,
    pub result: String,
    pub details: Option<String>,
}

pub struct AuditLogStore {
    entries: HashMap<String, AuditLogEntry>,
    store: JsonStore,
}

impl AuditLogStore {
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "audit_log");
        let entries = store.load();
        debug!(count = entries.len(), "loaded audit log from disk");
        Self { entries, store }
    }

    pub fn append(&mut self, entry: AuditLogEntry) {
        self.entries.insert(entry.id.clone(), entry);
        self.snapshot();
    }

    pub fn query(
        &self,
        actor: Option<&str>,
        action: Option<&str>,
        limit: usize,
    ) -> Vec<&AuditLogEntry> {
        let mut results: Vec<_> = self
            .entries
            .values()
            .filter(|e| actor.is_none_or(|a| e.actor == a))
            .filter(|e| action.is_none_or(|a| e.action == a))
            .collect();
        results.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        results.truncate(limit);
        results
    }

    fn snapshot(&self) {
        if let Err(e) = self.store.save(&self.entries) {
            warn!(error = %e, "failed to snapshot audit log");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_store_crud() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ApiKeyStore::new(dir.path());

        let key = ApiKeyRecord {
            key_id: "k-1".to_string(),
            name: "test-key".to_string(),
            secret_hash: "abc123hash".to_string(),
            scopes: vec!["vps.*".to_string()],
            role: "operator".to_string(),
            active: true,
            created_at: chrono::Utc::now(),
            last_used: None,
        };
        store.create(key).expect("create");
        assert!(store.get("k-1").is_some());
        assert!(store.find_by_hash("abc123hash").is_some());

        store.revoke("k-1").expect("revoke");
        assert!(!store.get("k-1").expect("get").active);
        assert!(store.find_by_hash("abc123hash").is_none());
    }

    #[test]
    fn test_audit_log_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = AuditLogStore::new(dir.path());

        store.append(AuditLogEntry {
            id: "a-1".to_string(),
            timestamp: chrono::Utc::now(),
            actor: "commander".to_string(),
            action: "provision".to_string(),
            resource: "instance".to_string(),
            resource_id: Some("i-abc".to_string()),
            result: "success".to_string(),
            details: None,
        });

        let all = store.query(None, None, 10);
        assert_eq!(all.len(), 1);

        let filtered = store.query(Some("commander"), None, 10);
        assert_eq!(filtered.len(), 1);
    }
}
