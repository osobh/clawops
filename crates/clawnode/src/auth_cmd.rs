//! Auth and audit command handlers — delegates to claw-auth

use crate::SharedState;
use crate::commands::{CommandError, CommandRequest};
use claw_auth::ApiKeyRecord;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub async fn handle_auth_command(
    state: &SharedState,
    request: CommandRequest,
) -> Result<serde_json::Value, CommandError> {
    match request.command.as_str() {
        "auth.create_key" => {
            let mut key_store = state.api_key_store.write().await;
            let label = request
                .params
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed")
                .to_string();
            let scopes: Vec<String> = request
                .params
                .get("scopes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Generate a random secret and hash it
            let raw_secret = Uuid::new_v4().to_string();
            let secret_hash = format!("{:x}", Sha256::digest(raw_secret.as_bytes()));
            let key_id = format!("key-{}", Uuid::new_v4().simple());

            let record = ApiKeyRecord {
                key_id: key_id.clone(),
                name: label,
                secret_hash,
                scopes,
                role: "operator".to_string(),
                active: true,
                created_at: chrono::Utc::now(),
                last_used: None,
            };

            key_store
                .create(record)
                .map_err(|e| format!("auth.create_key error: {e}"))?;

            Ok(json!({
                "ok": true,
                "key_id": key_id,
                "secret": raw_secret,
                "note": "Store this secret — it will not be shown again"
            }))
        }

        "auth.revoke_key" => {
            let mut key_store = state.api_key_store.write().await;
            let key_id = request
                .params
                .get("key_id")
                .and_then(|v| v.as_str())
                .ok_or("missing 'key_id'")?;

            key_store
                .revoke(key_id)
                .map_err(|e| format!("auth.revoke error: {e}"))?;

            Ok(json!({ "ok": true, "key_id": key_id }))
        }

        "auth.list_keys" => {
            let key_store = state.api_key_store.read().await;
            let keys = key_store.list();
            Ok(json!({
                "ok": true,
                "keys": keys.iter().map(|k| json!({
                    "key_id": k.key_id,
                    "name": k.name,
                    "scopes": k.scopes,
                    "role": k.role,
                    "active": k.active,
                    "created_at": k.created_at,
                    "last_used": k.last_used,
                })).collect::<Vec<_>>(),
            }))
        }

        "audit.query" => {
            let audit_store = state.audit_log_store.read().await;
            let limit = request
                .params
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50) as usize;
            let actor = request
                .params
                .get("actor")
                .and_then(|v| v.as_str())
                .map(String::from);
            let action = request
                .params
                .get("action")
                .and_then(|v| v.as_str())
                .map(String::from);

            let entries = audit_store.query(actor.as_deref(), action.as_deref(), limit);
            Ok(json!({
                "ok": true,
                "entries": entries.iter().map(|e| json!({
                    "id": e.id,
                    "actor": e.actor,
                    "action": e.action,
                    "resource": e.resource,
                    "resource_id": e.resource_id,
                    "result": e.result,
                    "timestamp": e.timestamp,
                    "details": e.details,
                })).collect::<Vec<_>>(),
            }))
        }

        other => Err(format!("unknown auth command: {other}").into()),
    }
}
