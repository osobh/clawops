//! Secret command handlers — delegates to claw-secrets

use crate::SharedState;
use crate::commands::{CommandError, CommandRequest};
use claw_secrets::SecretEntry;
use serde_json::json;

/// Simple base64 encode (no external dep, replace with AES-GCM when key manager is available).
fn encode_value(value: &str) -> String {
    use std::fmt::Write as FmtWrite;
    let bytes = value.as_bytes();
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        let _ = write!(
            out,
            "{}{}{}{}",
            table[b0 >> 2] as char,
            table[((b0 & 3) << 4) | (b1 >> 4)] as char,
            table[((b1 & 0xf) << 2) | (b2 >> 6)] as char,
            table[b2 & 0x3f] as char
        );
        i += 3;
    }
    if i < bytes.len() {
        let b0 = bytes[i] as usize;
        if i + 1 < bytes.len() {
            let b1 = bytes[i + 1] as usize;
            let _ = write!(
                out,
                "{}{}{}=",
                table[b0 >> 2] as char,
                table[((b0 & 3) << 4) | (b1 >> 4)] as char,
                table[(b1 & 0xf) << 2] as char
            );
        } else {
            let _ = write!(
                out,
                "{}{}==",
                table[b0 >> 2] as char,
                table[(b0 & 3) << 4] as char
            );
        }
    }
    out
}

fn decode_value(encoded: &str) -> String {
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let nibbles: Vec<u8> = encoded
        .bytes()
        .filter(|&b| b != b'=')
        .filter_map(|b| table.iter().position(|&t| t == b).map(|p| p as u8))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < nibbles.len() {
        out.push((nibbles[i] << 2) | (nibbles[i + 1] >> 4));
        out.push((nibbles[i + 1] << 4) | (nibbles[i + 2] >> 2));
        out.push((nibbles[i + 2] << 6) | nibbles[i + 3]);
        i += 4;
    }
    if i + 2 < nibbles.len() {
        out.push((nibbles[i] << 2) | (nibbles[i + 1] >> 4));
        out.push((nibbles[i + 1] << 4) | (nibbles[i + 2] >> 2));
    } else if i + 1 < nibbles.len() {
        out.push((nibbles[i] << 2) | (nibbles[i + 1] >> 4));
    }
    String::from_utf8(out).unwrap_or_default()
}

pub async fn handle_secret_command(
    state: &SharedState,
    request: CommandRequest,
) -> Result<serde_json::Value, CommandError> {
    let mut store = state.secret_store.write().await;
    let now = chrono::Utc::now();

    match request.command.as_str() {
        "secret.create" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;
            let value = request
                .params
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or("missing 'value'")?;

            let entry = SecretEntry {
                name: name.to_string(),
                encrypted_data: encode_value(value),
                nonce: String::new(),
                key_version: 1,
                created_at: now,
                rotated_at: now,
                rotation_due: None,
            };

            store
                .create(entry)
                .map_err(|e| format!("secret.create error: {e}"))?;

            Ok(json!({ "ok": true, "name": name }))
        }

        "secret.get" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;

            match store.get(name) {
                Some(entry) => {
                    let value = decode_value(&entry.encrypted_data);
                    Ok(json!({ "ok": true, "name": name, "value": value }))
                }
                None => Ok(json!({ "ok": false, "error": "secret not found" })),
            }
        }

        "secret.delete" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;

            store.delete(name);
            Ok(json!({ "ok": true, "name": name }))
        }

        "secret.list" => {
            let entries = store.list();
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            Ok(json!({ "ok": true, "names": names }))
        }

        "secret.rotate" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;
            let new_value = request
                .params
                .get("new_value")
                .or_else(|| request.params.get("value"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'new_value'")?;

            let (key_version, created_at) = store
                .get(name)
                .map(|e| (e.key_version + 1, e.created_at))
                .ok_or("secret not found")?;

            let updated = SecretEntry {
                name: name.to_string(),
                encrypted_data: encode_value(new_value),
                nonce: String::new(),
                key_version,
                created_at,
                rotated_at: now,
                rotation_due: None,
            };

            store
                .update(name, updated)
                .map_err(|e| format!("secret.rotate error: {e}"))?;

            Ok(json!({ "ok": true, "name": name, "rotated": true, "key_version": key_version }))
        }

        other => Err(format!("unknown secret command: {other}").into()),
    }
}
