//! VPS node persistence stores
//!
//! Thin in-memory stores backed by claw-persist's JsonStore for VPS-specific state.

use claw_persist::JsonStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ─── VPS Instance State ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpsInstanceRecord {
    pub instance_id: String,
    pub provider: String,
    pub region: String,
    pub tier: String,
    pub role: String,
    pub state: String,
    pub ip_public: Option<String>,
    pub ip_tailscale: Option<String>,
    pub provider_instance_id: Option<String>,
    pub account_id: String,
    pub provisioned_at: String,
}

pub struct VpsInstanceStore {
    records: HashMap<String, VpsInstanceRecord>,
    store: JsonStore,
}

impl VpsInstanceStore {
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "vps_instances");
        let records = store.load();
        Self { records, store }
    }

    pub fn upsert(&mut self, record: VpsInstanceRecord) {
        self.records.insert(record.instance_id.clone(), record);
        self.snapshot();
    }

    pub fn get(&self, id: &str) -> Option<&VpsInstanceRecord> {
        self.records.get(id)
    }

    pub fn list(&self) -> Vec<&VpsInstanceRecord> {
        self.records.values().collect()
    }

    pub fn remove(&mut self, id: &str) {
        self.records.remove(id);
        self.snapshot();
    }

    fn snapshot(&self) {
        let _ = self.store.save(&self.records);
    }
}

// ─── Event Log ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub instance_id: String,
    pub event_type: String,
    pub severity: String,
    pub description: String,
    pub timestamp: String,
    pub resolved: bool,
}

pub struct EventStore {
    records: HashMap<String, EventRecord>,
    store: JsonStore,
}

impl EventStore {
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "events");
        let records = store.load();
        Self { records, store }
    }

    pub fn append(&mut self, record: EventRecord) {
        self.records.insert(record.id.clone(), record);
        self.snapshot();
    }

    pub fn list_for_instance(&self, instance_id: &str) -> Vec<&EventRecord> {
        self.records
            .values()
            .filter(|r| r.instance_id == instance_id)
            .collect()
    }

    fn snapshot(&self) {
        let _ = self.store.save(&self.records);
    }
}
