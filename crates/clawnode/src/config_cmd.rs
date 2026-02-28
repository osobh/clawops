//! Config command handlers — delegates to claw-config

use crate::SharedState;
use crate::commands::{CommandError, CommandRequest};
use serde_json::{json, Value};
use std::collections::HashMap;

pub async fn handle_config_command(
    state: &SharedState,
    request: CommandRequest,
) -> Result<Value, CommandError> {
    let mut store = state.config_store.write().await;

    match request.command.as_str() {
        "config.get" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;

            match store.get(name) {
                Some(entry) => Ok(json!({
                    "ok": true,
                    "name": name,
                    "data": entry.data,
                    "immutable": entry.immutable,
                    "updated_at": entry.updated_at,
                })),
                None => Ok(json!({ "ok": false, "error": "config not found" })),
            }
        }

        "config.create" => {
            let name = request
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?
                .to_string();
            let immutable = request
                .params
                .get("immutable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let data: HashMap<String, String> = request
                .params
                .get("data")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            store
                .create(name.clone(), data, immutable)
                .map_err(|e| format!("config.create error: {e}"))?;

            Ok(json!({ "ok": true, "name": name }))
        }

        "config.set" => {
            let name = request
                .params
                .get("name")
                .or_else(|| request.params.get("key"))
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?
                .to_string();

            let data: HashMap<String, String> = request
                .params
                .get("data")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            if store.get(&name).is_some() {
                store
                    .update(&name, data)
                    .map_err(|e| format!("config.set update error: {e}"))?;
            } else {
                store
                    .create(name.clone(), data, false)
                    .map_err(|e| format!("config.set create error: {e}"))?;
            }

            Ok(json!({ "ok": true, "name": name }))
        }

        "config.update" => {
            let name = request
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;

            let data: HashMap<String, String> = request
                .params
                .get("data")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            store
                .update(name, data)
                .map_err(|e| format!("config.update error: {e}"))?;

            Ok(json!({ "ok": true, "name": name }))
        }

        "config.delete" => {
            let name = request
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;

            store
                .delete(name)
                .map_err(|e| format!("config.delete error: {e}"))?;

            Ok(json!({ "ok": true, "name": name }))
        }

        "config.list" => {
            let prefix = request.params.get("prefix").and_then(|v| v.as_str());
            let entries = store.list(prefix);
            Ok(json!({
                "ok": true,
                "configs": entries.iter().map(|(name, entry)| json!({
                    "name": name,
                    "immutable": entry.immutable,
                    "keys": entry.data.keys().collect::<Vec<_>>(),
                    "updated_at": entry.updated_at,
                })).collect::<Vec<_>>(),
            }))
        }

        other => Err(format!("unknown config command: {other}").into()),
    }
}
