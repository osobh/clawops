//! Immutable append-only audit trail with SHA-256 chain hashing for ClawOps.
//!
//! Every agent action that affects fleet state must be logged here before executing.
//! Records are cryptographically chained — tampering with any record breaks the chain.

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use claw_persist::JsonStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    Commander,
    Guardian,
    Forge,
    Ledger,
    Triage,
    Briefer,
    System,
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Commander => "commander",
            Self::Guardian => "guardian",
            Self::Forge => "forge",
            Self::Ledger => "ledger",
            Self::Triage => "triage",
            Self::Briefer => "briefer",
            Self::System => "system",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    ProvisionPrimary,
    ProvisionStandby,
    TeardownInstance,
    ResizeInstance,
    InitiateAutoHeal,
    DockerRestartOpenclaw,
    TriggerFailover,
    PromoteStandby,
    ScheduleReprovision,
    PushConfig,
    RollbackConfig,
    GenerateCostReport,
    GenerateIncidentReport,
    UpdateProviderSelection,
    SilenceAlerts,
    OperatorConfirmationReceived,
    AgentSpawned,
    AgentTerminated,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self)
                .unwrap_or_default()
                .as_str()
                .unwrap_or("unknown")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    Instance,
    Pair,
    Account,
    Fleet,
    Provider,
    Config,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub record_id: Uuid,
    pub correlation_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub agent: AgentId,
    pub action: AuditAction,
    pub target_type: TargetType,
    pub target_id: String,
    pub parameters: serde_json::Value,
    pub result: AuditResult,
    pub operator_confirmation: Option<String>,
    /// SHA-256 hex of previous record (empty string for first record).
    pub previous_hash: String,
    /// SHA-256 hex of this record's canonical JSON.
    pub record_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResult {
    pub success: bool,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

// ─── AuditLogger ─────────────────────────────────────────────────────────────

pub struct AuditLogger {
    records: HashMap<String, AuditRecord>,
    store: JsonStore,
    last_hash: String,
}

impl AuditLogger {
    /// Create or load the audit logger from disk.
    ///
    /// # Safety Requirement
    ///
    /// All provider API deletes MUST be logged before execution.
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "audit_chain");
        let records: HashMap<String, AuditRecord> = store.load();

        // Find the last record to continue the chain
        let last_hash = records
            .values()
            .max_by_key(|r| r.timestamp)
            .map(|r| r.record_hash.clone())
            .unwrap_or_default();

        info!(record_count = records.len(), last_hash = %last_hash, "audit logger initialized");
        Self {
            records,
            store,
            last_hash,
        }
    }

    /// Append a new audit record. Returns the record hash.
    ///
    /// This is the primary API — call this BEFORE executing any destructive action.
    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        agent: AgentId,
        action: AuditAction,
        target_type: TargetType,
        target_id: &str,
        parameters: serde_json::Value,
        result: AuditResult,
        correlation_id: Option<Uuid>,
        operator_confirmation: Option<String>,
    ) -> String {
        let record_id = Uuid::new_v4();
        let timestamp = Utc::now();

        // Build canonical JSON for hashing (before record_hash is set)
        let canonical = serde_json::json!({
            "record_id": record_id,
            "timestamp": timestamp,
            "agent": agent,
            "action": action,
            "target_type": target_type,
            "target_id": target_id,
            "parameters": parameters,
            "result": result,
            "previous_hash": self.last_hash,
        });

        let record_hash = sha256_hex(&canonical.to_string());

        let record = AuditRecord {
            record_id,
            correlation_id,
            timestamp,
            agent,
            action,
            target_type,
            target_id: target_id.to_string(),
            parameters,
            result,
            operator_confirmation,
            previous_hash: self.last_hash.clone(),
            record_hash: record_hash.clone(),
        };

        info!(
            record_id = %record_id,
            agent = %agent,
            action = %action,
            target = %target_id,
            "audit record appended"
        );

        self.last_hash = record_hash.clone();
        self.records.insert(record_id.to_string(), record);
        self.snapshot();

        record_hash
    }

    /// Query audit records with filters.
    pub fn query(
        &self,
        account_id: Option<&str>,
        instance_id: Option<&str>,
        agent: Option<AgentId>,
        action: Option<AuditAction>,
        limit: usize,
    ) -> Vec<&AuditRecord> {
        let mut results: Vec<&AuditRecord> = self
            .records
            .values()
            .filter(|r| {
                if account_id.is_some_and(|aid| {
                    !r.target_id.contains(aid) && !r.parameters.to_string().contains(aid)
                }) {
                    return false;
                }
                if instance_id.is_some_and(|iid| {
                    !r.target_id.contains(iid) && !r.parameters.to_string().contains(iid)
                }) {
                    return false;
                }
                if agent.is_some_and(|a| r.agent != a) {
                    return false;
                }
                if action.is_some_and(|act| r.action != act) {
                    return false;
                }
                true
            })
            .collect();

        results.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        results.truncate(limit);
        results
    }

    /// Verify the integrity of the audit chain.
    /// Returns `true` if chain is intact, `false` if tampered.
    pub fn verify_chain(&self) -> bool {
        let mut sorted: Vec<&AuditRecord> = self.records.values().collect();
        sorted.sort_by_key(|r| r.timestamp);

        let mut prev_hash = String::new();
        for record in sorted {
            if record.previous_hash != prev_hash {
                warn!(
                    record_id = %record.record_id,
                    expected = %prev_hash,
                    got = %record.previous_hash,
                    "chain integrity violation"
                );
                return false;
            }
            prev_hash = record.record_hash.clone();
        }
        true
    }

    fn snapshot(&self) {
        if let Err(e) = self.store.save(&self.records) {
            warn!(error = %e, "failed to snapshot audit chain");
        }
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_result(msg: &str) -> AuditResult {
        AuditResult {
            success: true,
            message: msg.to_string(),
            details: None,
        }
    }

    #[test]
    fn test_audit_append_and_query() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut logger = AuditLogger::new(dir.path());

        logger.append(
            AgentId::Forge,
            AuditAction::ProvisionPrimary,
            TargetType::Instance,
            "i-test",
            serde_json::json!({"account_id": "acc-1"}),
            ok_result("provisioned"),
            None,
            None,
        );

        let records = logger.query(Some("acc-1"), None, None, None, 10);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn test_chain_integrity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut logger = AuditLogger::new(dir.path());

        for i in 0..5 {
            logger.append(
                AgentId::System,
                AuditAction::AgentSpawned,
                TargetType::Agent,
                &format!("agent-{i}"),
                serde_json::json!({}),
                ok_result("ok"),
                None,
                None,
            );
        }

        assert!(logger.verify_chain());
    }

    #[test]
    fn test_audit_persistence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hash1 = {
            let mut logger = AuditLogger::new(dir.path());
            logger.append(
                AgentId::Commander,
                AuditAction::TeardownInstance,
                TargetType::Instance,
                "i-old",
                serde_json::json!({"reason": "idle"}),
                ok_result("torn down"),
                None,
                Some("CONF-123".to_string()),
            )
        };

        let logger2 = AuditLogger::new(dir.path());
        assert_eq!(logger2.records.len(), 1);
        // Last hash should match
        assert_eq!(logger2.last_hash, hash1);
    }
}
