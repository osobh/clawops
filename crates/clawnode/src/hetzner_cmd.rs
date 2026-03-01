//! Hetzner Cloud API commands for clawnode CLI.
//!
//! Provides: list, status, create, delete, resize, reboot, metrics

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tracing::info;

const HETZNER_API: &str = "https://api.hetzner.cloud/v1";

pub struct HetznerClient {
    token: String,
    client: reqwest::Client,
}

impl HetznerClient {
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
        }
    }

    /// Load token from config file or env var.
    pub fn from_config() -> Result<Self> {
        // Try env var first
        if let Ok(token) = std::env::var("HETZNER_API_TOKEN") {
            return Ok(Self::new(token));
        }

        // Try ~/.clawops/providers.json
        let home = dirs::home_dir().context("no home dir")?;
        let config_path = home.join(".clawops/providers.json");
        if config_path.exists() {
            let data = std::fs::read_to_string(&config_path)?;
            let cfg: Value = serde_json::from_str(&data)?;
            if let Some(token) = cfg["hetzner"]["api_token"].as_str() {
                return Ok(Self::new(token.to_string()));
            }
        }

        // Try /etc/clawnode/providers.json
        let etc_path = std::path::Path::new("/etc/clawnode/providers.json");
        if etc_path.exists() {
            let data = std::fs::read_to_string(etc_path)?;
            let cfg: Value = serde_json::from_str(&data)?;
            if let Some(token) = cfg["hetzner"]["api_token"].as_str() {
                return Ok(Self::new(token.to_string()));
            }
        }

        bail!("No Hetzner API token found. Set HETZNER_API_TOKEN env var or add to ~/.clawops/providers.json")
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self.client
            .get(format!("{}{}", HETZNER_API, path))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("Hetzner API request failed")?;

        let status = resp.status();
        let body: Value = resp.json().await.context("failed to parse response")?;

        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Hetzner API error ({}): {}", status, msg);
        }
        Ok(body)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self.client
            .post(format!("{}{}", HETZNER_API, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .context("Hetzner API request failed")?;

        let status = resp.status();
        let rbody: Value = resp.json().await.context("failed to parse response")?;

        if !status.is_success() {
            let msg = rbody["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Hetzner API error ({}): {}", status, msg);
        }
        Ok(rbody)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let resp = self.client
            .delete(format!("{}{}", HETZNER_API, path))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("Hetzner API request failed")?;

        if !resp.status().is_success() {
            let body: Value = resp.json().await.unwrap_or(json!({}));
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Hetzner API error: {}", msg);
        }
        Ok(())
    }

    // ── List all servers ──────────────────────────────────────────────────

    pub async fn list_servers(&self) -> Result<Value> {
        let mut all = Vec::new();
        let mut page = 1u32;

        loop {
            let resp = self.get(&format!("/servers?page={}&per_page=50", page)).await?;
            let servers = resp["servers"].as_array().context("missing servers")?;
            all.extend(servers.clone());
            if resp["meta"]["pagination"]["next_page"].is_null() {
                break;
            }
            page += 1;
        }

        let summary: Vec<Value> = all.iter().map(|s| json!({
            "id": s["id"],
            "name": s["name"],
            "status": s["status"],
            "server_type": s["server_type"]["name"],
            "cores": s["server_type"]["cores"],
            "memory_gb": s["server_type"]["memory"],
            "disk_gb": s["server_type"]["disk"],
            "datacenter": s["datacenter"]["name"],
            "location": s["datacenter"]["location"]["name"],
            "ip": s["public_net"]["ipv4"]["ip"],
            "ipv6": s["public_net"]["ipv6"]["ip"],
            "labels": s["labels"],
            "created": s["created"],
        })).collect();

        Ok(json!({
            "ok": true,
            "count": summary.len(),
            "servers": summary,
        }))
    }

    // ── Get single server ─────────────────────────────────────────────────

    pub async fn get_server(&self, id_or_name: &str) -> Result<Value> {
        // Try as ID first
        if let Ok(id) = id_or_name.parse::<u64>() {
            let resp = self.get(&format!("/servers/{}", id)).await?;
            return Ok(resp["server"].clone());
        }

        // Search by name
        let resp = self.get(&format!("/servers?name={}", id_or_name)).await?;
        let servers = resp["servers"].as_array().context("missing servers")?;
        match servers.len() {
            0 => bail!("no server found with name '{}'", id_or_name),
            1 => Ok(servers[0].clone()),
            _ => bail!("multiple servers match name '{}', use ID instead", id_or_name),
        }
    }

    // ── Server metrics ────────────────────────────────────────────────────

    pub async fn server_metrics(&self, id_or_name: &str) -> Result<Value> {
        let server = self.get_server(id_or_name).await?;
        let server_id = server["id"].as_u64().context("missing id")?;

        let end = chrono::Utc::now();
        let start = end - chrono::Duration::hours(1);
        let path = format!(
            "/servers/{}/metrics?type=cpu,disk,network&start={}&end={}&step=60",
            server_id,
            start.to_rfc3339(),
            end.to_rfc3339(),
        );
        let resp = self.get(&path).await?;

        Ok(json!({
            "ok": true,
            "server_id": server_id,
            "server_name": server["name"],
            "metrics": resp["metrics"],
        }))
    }

    // ── Create server ─────────────────────────────────────────────────────

    pub async fn create_server(
        &self,
        name: &str,
        server_type: &str,
        location: &str,
        image: &str,
        ssh_keys: &[String],
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<Value> {
        info!(name, server_type, location, "creating Hetzner server");

        let mut body = json!({
            "name": name,
            "server_type": server_type,
            "location": location,
            "image": image,
            "start_after_create": true,
            "labels": labels,
        });

        if !ssh_keys.is_empty() {
            body["ssh_keys"] = json!(ssh_keys);
        }

        let resp = self.post("/servers", &body).await?;

        let server = &resp["server"];
        Ok(json!({
            "ok": true,
            "id": server["id"],
            "name": server["name"],
            "status": server["status"],
            "ip": server["public_net"]["ipv4"]["ip"],
            "root_password": resp["root_password"],
            "server_type": server["server_type"]["name"],
            "location": location,
        }))
    }

    // ── Delete server ─────────────────────────────────────────────────────

    pub async fn delete_server(&self, id_or_name: &str) -> Result<Value> {
        let server = self.get_server(id_or_name).await?;
        let server_id = server["id"].as_u64().context("missing id")?;
        let name = server["name"].as_str().unwrap_or("unknown");

        info!(server_id, name, "deleting Hetzner server");
        self.delete(&format!("/servers/{}", server_id)).await?;

        Ok(json!({
            "ok": true,
            "deleted_id": server_id,
            "deleted_name": name,
        }))
    }

    // ── Reboot server ─────────────────────────────────────────────────────

    pub async fn reboot_server(&self, id_or_name: &str) -> Result<Value> {
        let server = self.get_server(id_or_name).await?;
        let server_id = server["id"].as_u64().context("missing id")?;

        self.post(&format!("/servers/{}/actions/reboot", server_id), &json!({})).await?;

        Ok(json!({
            "ok": true,
            "server_id": server_id,
            "action": "reboot",
        }))
    }

    // ── Power on/off ──────────────────────────────────────────────────────

    pub async fn power_action(&self, id_or_name: &str, action: &str) -> Result<Value> {
        let server = self.get_server(id_or_name).await?;
        let server_id = server["id"].as_u64().context("missing id")?;

        self.post(&format!("/servers/{}/actions/{}", server_id, action), &json!({})).await?;

        Ok(json!({
            "ok": true,
            "server_id": server_id,
            "action": action,
        }))
    }

    // ── Resize server ─────────────────────────────────────────────────────

    pub async fn resize_server(&self, id_or_name: &str, new_type: &str) -> Result<Value> {
        let server = self.get_server(id_or_name).await?;
        let server_id = server["id"].as_u64().context("missing id")?;
        let old_type = server["server_type"]["name"].as_str().unwrap_or("unknown");

        info!(server_id, old_type, new_type, "resizing server");

        // Power off first
        let _ = self.post(&format!("/servers/{}/actions/poweroff", server_id), &json!({})).await;
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        // Change type
        self.post(
            &format!("/servers/{}/actions/change_type", server_id),
            &json!({ "server_type": new_type, "upgrade_disk": false }),
        ).await?;

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Power on
        self.post(&format!("/servers/{}/actions/poweron", server_id), &json!({})).await?;

        Ok(json!({
            "ok": true,
            "server_id": server_id,
            "old_type": old_type,
            "new_type": new_type,
        }))
    }

    // ── List available server types ───────────────────────────────────────

    pub async fn list_server_types(&self) -> Result<Value> {
        let resp = self.get("/server_types").await?;
        let types: Vec<Value> = resp["server_types"].as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter(|t| t["architecture"].as_str() == Some("x86"))
            .map(|t| json!({
                "name": t["name"],
                "cores": t["cores"],
                "memory_gb": t["memory"],
                "disk_gb": t["disk"],
                "price_monthly": t["prices"].as_array()
                    .and_then(|p| p.first())
                    .and_then(|p| p["price_monthly"]["gross"].as_str()),
            }))
            .collect();

        Ok(json!({ "ok": true, "types": types }))
    }

    // ── List SSH keys ─────────────────────────────────────────────────────

    pub async fn list_ssh_keys(&self) -> Result<Value> {
        let resp = self.get("/ssh_keys").await?;
        let keys: Vec<Value> = resp["ssh_keys"].as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|k| json!({
                "id": k["id"],
                "name": k["name"],
                "fingerprint": k["fingerprint"],
            }))
            .collect();

        Ok(json!({ "ok": true, "keys": keys }))
    }
}
