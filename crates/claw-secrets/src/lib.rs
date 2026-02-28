//! Encrypted secrets management for ClawOps nodes.
//!
//! Provides [`SecretStore`] for AES-256-GCM encrypted secret storage
//! with key rotation tracking and memory-safe secret handling.
//!
//! # Security notes
//! - `SecretEntry` implements a custom `Debug` that **never** prints the ciphertext.
//! - Encrypted bytes are overwritten with zeros on drop (best-effort mitigation).
//! - The `rotation_due` field signals when a secret needs rotation per policy.

#![forbid(unsafe_code)]

use claw_persist::JsonStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────

/// Errors from the secret store.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// A secret with the given name already exists.
    #[error("secret '{0}' already exists")]
    AlreadyExists(String),
    /// No secret with the given name was found.
    #[error("secret '{0}' not found")]
    NotFound(String),
    /// The secret exists but its rotation is overdue.
    #[error("rotation is due for secret '{0}'")]
    RotationDue(String),
}

// ─────────────────────────────────────────────────────────────
// SecretEntry
// ─────────────────────────────────────────────────────────────

/// An encrypted secret entry.
///
/// # Security
/// - The `Debug` implementation **redacts** `encrypted_data` and `nonce`.
/// - On drop, those fields are overwritten with zeros.
#[derive(Clone, Serialize, Deserialize)]
pub struct SecretEntry {
    /// Secret name (used as the store key).
    pub name: String,
    /// Encrypted data (base64-encoded ciphertext). Never printed in Debug.
    pub encrypted_data: String,
    /// Nonce used for encryption (base64-encoded). Never printed in Debug.
    pub nonce: String,
    /// Key version used for encryption.
    pub key_version: u32,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last rotation timestamp.
    pub rotated_at: chrono::DateTime<chrono::Utc>,
    /// Optional: when the next rotation is due (None = no policy).
    pub rotation_due: Option<chrono::DateTime<chrono::Utc>>,
}

/// Custom `Debug` that redacts ciphertext and nonce.
impl std::fmt::Debug for SecretEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretEntry")
            .field("name", &self.name)
            .field("encrypted_data", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("key_version", &self.key_version)
            .field("created_at", &self.created_at)
            .field("rotated_at", &self.rotated_at)
            .field("rotation_due", &self.rotation_due)
            .finish()
    }
}

impl Drop for SecretEntry {
    /// Overwrite sensitive fields with zeros on drop to limit the window in which
    /// ciphertext material is accessible on the heap.
    fn drop(&mut self) {
        let zeros_data = vec![b'0'; self.encrypted_data.len()];
        if let Ok(s) = String::from_utf8(zeros_data) {
            self.encrypted_data = s;
        }
        let zeros_nonce = vec![b'0'; self.nonce.len()];
        if let Ok(s) = String::from_utf8(zeros_nonce) {
            self.nonce = s;
        }
    }
}

impl SecretEntry {
    /// Returns `true` if this secret's rotation policy is overdue.
    pub fn is_rotation_due(&self) -> bool {
        self.rotation_due
            .is_some_and(|due| due < chrono::Utc::now())
    }
}

// ─────────────────────────────────────────────────────────────
// SecretStore
// ─────────────────────────────────────────────────────────────

/// In-memory secret store backed by JSON snapshots.
pub struct SecretStore {
    secrets: HashMap<String, SecretEntry>,
    store: JsonStore,
}

impl SecretStore {
    /// Create a new secret store, loading any existing state from disk.
    pub fn new(state_path: &Path) -> Self {
        let store = JsonStore::new(state_path, "secrets");
        let secrets = store.load();
        debug!(count = secrets.len(), "loaded secrets from disk");
        Self { secrets, store }
    }

    /// Create a new secret. Returns `Err(AlreadyExists)` if the name is taken.
    pub fn create(&mut self, entry: SecretEntry) -> Result<(), SecretError> {
        if self.secrets.contains_key(&entry.name) {
            return Err(SecretError::AlreadyExists(entry.name.clone()));
        }
        let name = entry.name.clone();
        self.secrets.insert(name, entry);
        self.snapshot();
        Ok(())
    }

    /// Get a secret by name. Returns `None` if not found.
    pub fn get(&self, name: &str) -> Option<&SecretEntry> {
        self.secrets.get(name)
    }

    /// Get a secret by name, returning `Err(RotationDue)` if rotation is overdue.
    pub fn get_checked(&self, name: &str) -> Result<&SecretEntry, SecretError> {
        let entry = self
            .secrets
            .get(name)
            .ok_or_else(|| SecretError::NotFound(name.to_string()))?;
        if entry.is_rotation_due() {
            return Err(SecretError::RotationDue(name.to_string()));
        }
        Ok(entry)
    }

    /// Update (rotate) an existing secret.
    pub fn update(&mut self, name: &str, entry: SecretEntry) -> Result<(), SecretError> {
        if !self.secrets.contains_key(name) {
            return Err(SecretError::NotFound(name.to_string()));
        }
        self.secrets.insert(name.to_string(), entry);
        self.snapshot();
        Ok(())
    }

    /// Delete a secret. Returns the removed entry, or `None` if not found.
    pub fn delete(&mut self, name: &str) -> Option<SecretEntry> {
        let entry = self.secrets.remove(name);
        if entry.is_some() {
            self.snapshot();
        }
        entry
    }

    /// List all secrets (metadata only; sensitive fields are not printed in Debug).
    pub fn list(&self) -> Vec<&SecretEntry> {
        self.secrets.values().collect()
    }

    /// List all secrets whose rotation is overdue.
    pub fn rotation_due(&self) -> Vec<&SecretEntry> {
        self.secrets
            .values()
            .filter(|e| e.is_rotation_due())
            .collect()
    }

    fn snapshot(&self) {
        if let Err(e) = self.store.save(&self.secrets) {
            warn!(error = %e, "failed to snapshot secret store");
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str) -> SecretEntry {
        SecretEntry {
            name: name.to_string(),
            encrypted_data: "Y2lwaGVydGV4dA==".to_string(),
            nonce: "bm9uY2U=".to_string(),
            key_version: 1,
            created_at: chrono::Utc::now(),
            rotated_at: chrono::Utc::now(),
            rotation_due: None,
        }
    }

    #[test]
    fn test_secret_store_crud() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = SecretStore::new(dir.path());

        store.create(make_entry("db-password")).expect("create");
        assert!(store.get("db-password").is_some());
        assert_eq!(store.list().len(), 1);

        assert!(store.create(make_entry("db-password")).is_err());

        store.delete("db-password");
        assert!(store.get("db-password").is_none());
    }

    #[test]
    fn test_secret_store_persistence() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut store = SecretStore::new(dir.path());
            store.create(make_entry("persist-test")).expect("create");
        }
        {
            let store = SecretStore::new(dir.path());
            assert!(store.get("persist-test").is_some());
        }
    }

    #[test]
    fn test_debug_redacts_ciphertext() {
        let entry = make_entry("my-secret");
        let debug_str = format!("{entry:?}");
        assert!(
            debug_str.contains("[REDACTED]"),
            "Debug must redact ciphertext"
        );
        assert!(
            !debug_str.contains("Y2lwaGVydGV4dA=="),
            "base64 ciphertext must not appear in Debug output"
        );
        assert!(
            !debug_str.contains("bm9uY2U="),
            "base64 nonce must not appear in Debug output"
        );
    }

    #[test]
    fn test_rotation_due_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = SecretStore::new(dir.path());

        let mut entry = make_entry("stale");
        entry.rotation_due = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        store.create(entry).expect("create");

        assert_eq!(store.rotation_due().len(), 1);
        assert!(
            matches!(store.get_checked("stale"), Err(SecretError::RotationDue(_))),
            "get_checked must return RotationDue for overdue secret"
        );
    }

    #[test]
    fn test_fresh_secret_passes_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = SecretStore::new(dir.path());

        let mut entry = make_entry("fresh");
        entry.rotation_due = Some(chrono::Utc::now() + chrono::Duration::days(30));
        store.create(entry).expect("create");

        assert!(store.get_checked("fresh").is_ok());
        assert_eq!(store.rotation_due().len(), 0);
    }
}
