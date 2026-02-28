//! Multi-provider VPS provisioning for ClawOps.
//!
//! Supports Hetzner, Vultr, Contabo, Hostinger, and DigitalOcean.
//! The Forge agent calls this crate's high-level API. Provider-specific
//! implementations handle authentication, API quirks, and region mappings.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use claw_proto::{InstanceRole, InstanceTier, ProvisionRequest, ProvisionResult, VpsProvider};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

// ─── Provider trait ───────────────────────────────────────────────────────────

/// All provider implementations must implement this trait.
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult>;
    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()>;
    async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult>;
    async fn provider_health(&self) -> Result<ProviderHealth>;
    fn supported_regions(&self) -> Vec<Region>;
    fn supports_live_resize(&self) -> bool;
}

// ─── Core types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub id: String,
    pub display_name: String,
    pub city: String,
    pub country: String,
    pub continent: Continent,
    pub provider: VpsProvider,
    pub available: bool,
    pub latency_class: LatencyClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Continent {
    EU,
    US,
    APAC,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LatencyClass {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierSpec {
    pub tier: InstanceTier,
    pub vcpu: u32,
    pub ram_gb: u32,
    pub disk_gb: u32,
    pub bandwidth_tb: f32,
    pub monthly_cost_usd: f32,
}

impl TierSpec {
    pub fn all() -> HashMap<String, TierSpec> {
        [
            ("nano".to_string(), TierSpec { tier: InstanceTier::Nano, vcpu: 1, ram_gb: 1, disk_gb: 20, bandwidth_tb: 1.0, monthly_cost_usd: 4.00 }),
            ("standard".to_string(), TierSpec { tier: InstanceTier::Standard, vcpu: 2, ram_gb: 4, disk_gb: 80, bandwidth_tb: 4.0, monthly_cost_usd: 12.00 }),
            ("pro".to_string(), TierSpec { tier: InstanceTier::Pro, vcpu: 4, ram_gb: 8, disk_gb: 160, bandwidth_tb: 8.0, monthly_cost_usd: 24.00 }),
            ("enterprise".to_string(), TierSpec { tier: InstanceTier::Enterprise, vcpu: 8, ram_gb: 16, disk_gb: 320, bandwidth_tb: 20.0, monthly_cost_usd: 48.00 }),
        ].into_iter().collect()
    }

    pub fn monthly_cost(tier: &InstanceTier) -> f32 {
        match tier {
            InstanceTier::Nano => 4.00,
            InstanceTier::Standard => 12.00,
            InstanceTier::Pro => 24.00,
            InstanceTier::Enterprise => 48.00,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeResult {
    pub instance_id: String,
    pub old_tier: InstanceTier,
    pub new_tier: InstanceTier,
    pub downtime_seconds: u32,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider: VpsProvider,
    pub api_reachable: bool,
    pub health_score: u8,
    pub provision_avg_ms: u64,
    pub provision_success_rate_7d: f32,
    pub active_incident: bool,
    pub incident_description: Option<String>,
    pub quota_used_pct: f32,
    pub checked_at: DateTime<Utc>,
}

// ─── Provider registry ────────────────────────────────────────────────────────

pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self { providers: HashMap::new() }
    }

    pub fn from_env() -> Self {
        let mut registry = Self::new();

        if let Ok(token) = std::env::var("HETZNER_API_TOKEN") {
            registry.register(Box::new(HetznerProvider::new(token)));
        }
        if let Ok(key) = std::env::var("VULTR_API_KEY") {
            registry.register(Box::new(VultrProvider { api_key: key, client: build_client() }));
        }
        if let Ok(key) = std::env::var("CONTABO_API_KEY") {
            registry.register(Box::new(ContaboProvider { api_key: key }));
        }
        if let Ok(key) = std::env::var("HOSTINGER_API_KEY") {
            registry.register(Box::new(HostingerProvider { api_key: key }));
        }
        if let Ok(token) = std::env::var("DO_API_TOKEN") {
            registry.register(Box::new(DigitalOceanProvider { api_token: token }));
        }

        registry
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        info!(name = provider.name(), "registering provider");
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    pub async fn select_provider(
        &self,
        preferred: &VpsProvider,
        continent: Continent,
    ) -> Option<(&dyn Provider, Region)> {
        let preferred_name = provider_name(preferred);

        if let Some(provider) = self.providers.get(preferred_name)
            && let Ok(health) = provider.provider_health().await
            && health.health_score >= 75 && !health.active_incident
            && let Some(region) = provider
                .supported_regions()
                .into_iter()
                .find(|r| r.continent == continent && r.available)
        {
            return Some((provider.as_ref(), region));
        }

        // Fallback: find best available provider
        let mut candidates: Vec<(u8, &str)> = Vec::new();
        for (name, provider) in &self.providers {
            if name == preferred_name {
                continue;
            }
            if let Ok(health) = provider.provider_health().await
                && !health.active_incident && health.health_score >= 65
            {
                candidates.push((health.health_score, name.as_str()));
            }
        }
        candidates.sort_by_key(|b| std::cmp::Reverse(b.0));

        for (_, name) in candidates {
            if let Some(provider) = self.providers.get(name)
                && let Some(region) = provider
                    .supported_regions()
                    .into_iter()
                    .find(|r| r.continent == continent && r.available)
            {
                return Some((provider.as_ref(), region));
            }
        }

        None
    }

    pub async fn all_health(&self) -> Vec<ProviderHealth> {
        let mut results = Vec::new();
        for provider in self.providers.values() {
            match provider.provider_health().await {
                Ok(health) => results.push(health),
                Err(e) => warn!(provider = provider.name(), "provider health check failed: {e}"),
            }
        }
        results
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn provider_name(provider: &VpsProvider) -> &'static str {
    match provider {
        VpsProvider::Hetzner => "hetzner",
        VpsProvider::Vultr => "vultr",
        VpsProvider::Contabo => "contabo",
        VpsProvider::Hostinger => "hostinger",
        VpsProvider::DigitalOcean => "digitalocean",
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client")
}

// ─── Cloud-init bootstrap script ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn cloud_init_script(
    instance_id: &str,
    account_id: &str,
    gateway_url: &str,
    api_key: &str,
    role: &InstanceRole,
    pair_instance_id: Option<&str>,
    tier: &str,
    provider: &str,
    region: &str,
) -> String {
    let role_str = match role {
        InstanceRole::Primary => "primary",
        InstanceRole::Standby => "standby",
    };
    let pair_id = pair_instance_id.unwrap_or("");
    let tailscale_auth_key = std::env::var("TAILSCALE_AUTH_KEY").unwrap_or_default();
    let clawnode_url = std::env::var("CLAWNODE_BINARY_URL").unwrap_or_else(|_| {
        "https://releases.clawops.io/clawnode/latest/clawnode-linux-amd64".to_string()
    });

    format!(
        r#"#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl wget ca-certificates gnupg lsb-release
# Docker
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" > /etc/apt/sources.list.d/docker.list
apt-get update -qq
apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-compose-plugin
systemctl enable docker && systemctl start docker
# Tailscale
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --authkey="{tailscale_auth_key}" --hostname="co-{instance_id}" --accept-routes
# ClawNode
mkdir -p /usr/local/bin /etc/clawnode /var/log/clawnode
wget -q -O /usr/local/bin/clawnode "{clawnode_url}"
chmod +x /usr/local/bin/clawnode
cat > /etc/clawnode/config.json << 'CONFIG_EOF'
{{
  "instance_id": "{instance_id}",
  "account_id": "{account_id}",
  "gateway": "{gateway_url}",
  "token": "{api_key}",
  "provider": "{provider}",
  "region": "{region}",
  "tier": "{tier}",
  "role": "{role_str}",
  "pair_instance_id": "{pair_id}",
  "heartbeat_interval_secs": 30
}}
CONFIG_EOF
cat > /etc/systemd/system/clawnode.service << 'SERVICE_EOF'
[Unit]
Description=ClawOps ClawNode Agent
After=network.target docker.service tailscaled.service
Requires=docker.service
[Service]
Type=simple
ExecStart=/usr/local/bin/clawnode run --config /etc/clawnode/config.json
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal
SyslogIdentifier=clawnode
[Install]
WantedBy=multi-user.target
SERVICE_EOF
systemctl daemon-reload && systemctl enable clawnode && systemctl start clawnode
echo "clawnode bootstrap complete - instance {instance_id}"
"#
    )
}


// ─── Hetzner provider ─────────────────────────────────────────────────────────

fn hetzner_server_type(tier: &InstanceTier) -> &'static str {
    match tier {
        InstanceTier::Nano => "cx11",
        InstanceTier::Standard => "cx21",
        InstanceTier::Pro => "cx31",
        InstanceTier::Enterprise => "cx41",
    }
}

fn hetzner_location(region_id: &str) -> &'static str {
    match region_id {
        "eu-hetzner-nbg1" => "nbg1",
        "eu-hetzner-hel1" => "hel1",
        "eu-hetzner-fsn1" => "fsn1",
        "us-hetzner-ash" => "ash",
        _ => "nbg1",
    }
}

#[derive(Debug)]
pub struct HetznerProvider {
    api_token: String,
    base_url: String,
    client: reqwest::Client,
}

impl HetznerProvider {
    pub fn new(api_token: String) -> Self {
        Self {
            api_token,
            base_url: "https://api.hetzner.cloud/v1".to_string(),
            client: build_client(),
        }
    }

    fn regions() -> Vec<Region> {
        vec![
            Region { id: "eu-hetzner-nbg1".to_string(), display_name: "Hetzner Nuremberg 1".to_string(), city: "Nuremberg".to_string(), country: "DE".to_string(), continent: Continent::EU, provider: VpsProvider::Hetzner, available: true, latency_class: LatencyClass::Low },
            Region { id: "eu-hetzner-hel1".to_string(), display_name: "Hetzner Helsinki 1".to_string(), city: "Helsinki".to_string(), country: "FI".to_string(), continent: Continent::EU, provider: VpsProvider::Hetzner, available: true, latency_class: LatencyClass::Low },
            Region { id: "eu-hetzner-fsn1".to_string(), display_name: "Hetzner Falkenstein 1".to_string(), city: "Falkenstein".to_string(), country: "DE".to_string(), continent: Continent::EU, provider: VpsProvider::Hetzner, available: true, latency_class: LatencyClass::Low },
            Region { id: "us-hetzner-ash".to_string(), display_name: "Hetzner Ashburn".to_string(), city: "Ashburn".to_string(), country: "US".to_string(), continent: Continent::US, provider: VpsProvider::Hetzner, available: true, latency_class: LatencyClass::Medium },
        ]
    }

    async fn wait_for_server_running(&self, server_id: u64) -> Result<()> {
        let url = format!("{}/servers/{}", self.base_url, server_id);
        for attempt in 0..120 {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            let resp: serde_json::Value = self
                .client
                .get(&url)
                .bearer_auth(&self.api_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            let status = resp["server"]["status"].as_str().unwrap_or("unknown");
            if status == "running" {
                return Ok(());
            }
            if status == "off" || status == "error" {
                bail!("server {} entered unexpected state: {status}", server_id);
            }
            if attempt % 12 == 0 {
                info!(server_id, status, "waiting for server to reach running state");
            }
        }
        bail!("timeout waiting for Hetzner server {} to reach running state", server_id)
    }

    /// List all ClawOps-managed servers.
    pub async fn list_servers(&self) -> Result<Vec<HetznerServer>> {
        let mut all_servers = Vec::new();
        let mut page = 1u32;

        loop {
            let url = format!(
                "{}/servers?label_selector=managed_by%3Dclawops&page={}&per_page=25",
                self.base_url, page
            );
            let resp: HetznerListServersResponse = self
                .client
                .get(&url)
                .bearer_auth(&self.api_token)
                .send()
                .await
                .context("Hetzner GET /servers request failed")?
                .error_for_status()
                .context("Hetzner GET /servers returned error status")?
                .json()
                .await
                .context("Failed to parse Hetzner server list")?;

            let has_next = resp.meta.pagination.next_page.is_some();
            all_servers.extend(resp.servers);
            if has_next { page += 1; } else { break; }
        }

        info!(count = all_servers.len(), "listed Hetzner servers");
        Ok(all_servers)
    }

    /// Get a single server by provider ID.
    pub async fn get_server(&self, server_id: u64) -> Result<HetznerServer> {
        let url = format!("{}/servers/{}", self.base_url, server_id);
        let resp: serde_json::Value = self
            .client
            .get(&url)
            .bearer_auth(&self.api_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        serde_json::from_value(resp["server"].clone())
            .context("failed to deserialize Hetzner server")
    }
}

#[async_trait]
impl Provider for HetznerProvider {
    fn name(&self) -> &str { "hetzner" }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        let start = std::time::Instant::now();
        info!(account_id = %req.account_id, region = %req.region, tier = ?req.tier, "provisioning Hetzner instance");

        let server_type = hetzner_server_type(&req.tier);
        let location = hetzner_location(&req.region);
        let server_name = format!("co-{}-{}", req.account_id, &req.request_id.to_string()[..8]);
        let instance_id = Uuid::new_v4().to_string();

        let user_data = cloud_init_script(
            &instance_id,
            &req.account_id,
            &std::env::var("CLAWOPS_GATEWAY_URL").unwrap_or_default(),
            &std::env::var("CLAWOPS_API_KEY").unwrap_or_default(),
            &req.role,
            req.pair_instance_id.as_deref(),
            &req.tier.to_string(),
            "hetzner",
            &req.region,
        );

        let body = serde_json::json!({
            "name": server_name,
            "server_type": server_type,
            "location": location,
            "image": "ubuntu-22.04",
            "user_data": user_data,
            "labels": {
                "account_id": req.account_id,
                "tier": req.tier.to_string(),
                "role": req.role.to_string(),
                "managed_by": "clawops",
            },
            "start_after_create": true,
        });

        let resp: serde_json::Value = self
            .client
            .post(format!("{}/servers", self.base_url))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .context("Hetzner POST /servers request failed")?
            .error_for_status()
            .context("Hetzner POST /servers returned error status")?
            .json()
            .await
            .context("failed to parse Hetzner server creation response")?;

        let server_id = resp["server"]["id"]
            .as_u64()
            .context("missing server.id in Hetzner response")?;
        let instance_ip = resp["server"]["public_net"]["ipv4"]["ip"]
            .as_str()
            .map(String::from);

        self.wait_for_server_running(server_id).await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(%instance_id, server_id, duration_ms, "Hetzner instance provisioned");

        Ok(ProvisionResult {
            request_id: req.request_id,
            instance_id: Some(instance_id),
            success: true,
            error: None,
            provision_duration_ms: duration_ms,
            instance_ip,
            tailscale_ip: None,
            provider_instance_id: Some(server_id.to_string()),
        })
    }

    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()> {
        info!(provider_instance_id, account_id, "tearing down Hetzner instance");
        self.client
            .delete(format!("{}/servers/{}", self.base_url, provider_instance_id))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .context("Hetzner DELETE /servers request failed")?
            .error_for_status()
            .context("Hetzner DELETE /servers returned error status")?;
        info!(provider_instance_id, "Hetzner instance deleted");
        Ok(())
    }

    async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult> {
        info!(provider_instance_id, tier = ?new_tier, "resizing Hetzner instance");

        // Power off
        self.client
            .post(format!("{}/servers/{}/actions/poweroff", self.base_url, provider_instance_id))
            .bearer_auth(&self.api_token)
            .send().await?.error_for_status()?;

        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;

        // Change type
        let body = serde_json::json!({
            "server_type": hetzner_server_type(new_tier),
            "upgrade_disk": false,
        });
        self.client
            .post(format!("{}/servers/{}/actions/change_type", self.base_url, provider_instance_id))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send().await?.error_for_status()?;

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Power back on
        self.client
            .post(format!("{}/servers/{}/actions/poweron", self.base_url, provider_instance_id))
            .bearer_auth(&self.api_token)
            .send().await?.error_for_status()?;

        Ok(ResizeResult {
            instance_id: provider_instance_id.to_string(),
            old_tier: InstanceTier::Standard, // Would be fetched from API in production
            new_tier: *new_tier,
            downtime_seconds: 20,
            completed_at: Utc::now(),
        })
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        let start = std::time::Instant::now();
        let api_resp = self
            .client
            .get(format!("{}/datacenters", self.base_url))
            .bearer_auth(&self.api_token)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        let api_reachable = api_resp.is_ok() && api_resp.as_ref().unwrap().status().is_success();
        let api_latency_ms = start.elapsed().as_millis() as u64;

        let health_score = if !api_reachable { 0u8 }
            else if api_latency_ms < 500 { 95 }
            else if api_latency_ms < 2000 { 75 }
            else { 50 };

        Ok(ProviderHealth {
            provider: VpsProvider::Hetzner,
            api_reachable,
            health_score,
            provision_avg_ms: 252_000,
            provision_success_rate_7d: 0.99,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0,
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> Vec<Region> { Self::regions() }
    fn supports_live_resize(&self) -> bool { false }
}

// ─── Hetzner API types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerServer {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub created: String,
    pub public_net: HetznerPublicNet,
    pub server_type: HetznerServerType,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerPublicNet {
    pub ipv4: Option<HetznerIpv4>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerIpv4 {
    pub ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerServerType {
    pub id: u32,
    pub name: String,
    pub cores: u32,
    pub memory: f32,
    pub disk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerListServersResponse {
    pub servers: Vec<HetznerServer>,
    pub meta: HetznerMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerMeta {
    pub pagination: HetznerPagination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerPagination {
    pub page: u32,
    pub per_page: u32,
    pub next_page: Option<u32>,
    pub total_entries: u32,
}

// ─── Vultr provider (stub) ────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VultrProvider {
    api_key: String,
    client: reqwest::Client,
}

#[async_trait]
impl Provider for VultrProvider {
    fn name(&self) -> &str { "vultr" }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        info!(account_id = %req.account_id, "provisioning Vultr instance (stub)");
        bail!("Vultr provisioning not yet fully implemented")
    }

    async fn teardown(&self, provider_instance_id: &str, _account_id: &str) -> Result<()> {
        self.client
            .delete(format!("https://api.vultr.com/v2/instances/{}", provider_instance_id))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send().await?.error_for_status()?;
        Ok(())
    }

    async fn resize(&self, _provider_instance_id: &str, _new_tier: &InstanceTier) -> Result<ResizeResult> {
        bail!("Vultr resize not yet implemented")
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        let start = std::time::Instant::now();
        let ok = self.client
            .get("https://api.vultr.com/v2/regions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .timeout(std::time::Duration::from_secs(5))
            .send().await.is_ok();
        let ms = start.elapsed().as_millis() as u64;
        Ok(ProviderHealth {
            provider: VpsProvider::Vultr,
            api_reachable: ok,
            health_score: if ok && ms < 500 { 90 } else if ok { 70 } else { 0 },
            provision_avg_ms: 180_000,
            provision_success_rate_7d: 0.97,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0,
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> Vec<Region> {
        vec![
            Region { id: "eu-vultr-ams".to_string(), display_name: "Vultr Amsterdam".to_string(), city: "Amsterdam".to_string(), country: "NL".to_string(), continent: Continent::EU, provider: VpsProvider::Vultr, available: true, latency_class: LatencyClass::Low },
            Region { id: "us-vultr-ewr".to_string(), display_name: "Vultr New Jersey".to_string(), city: "Newark".to_string(), country: "US".to_string(), continent: Continent::US, provider: VpsProvider::Vultr, available: true, latency_class: LatencyClass::Medium },
        ]
    }

    fn supports_live_resize(&self) -> bool { false }
}

// ─── Contabo provider (stub) ──────────────────────────────────────────────────

#[derive(Debug)]
pub struct ContaboProvider {
    pub api_key: String,
}

#[async_trait]
impl Provider for ContaboProvider {
    fn name(&self) -> &str { "contabo" }
    async fn provision(&self, _req: &ProvisionRequest) -> Result<ProvisionResult> { bail!("Contabo provisioning not yet implemented") }
    async fn teardown(&self, _id: &str, _account_id: &str) -> Result<()> { bail!("Contabo teardown not yet implemented") }
    async fn resize(&self, _id: &str, _tier: &InstanceTier) -> Result<ResizeResult> { bail!("Contabo resize not yet implemented") }
    async fn provider_health(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth { provider: VpsProvider::Contabo, api_reachable: false, health_score: 0, provision_avg_ms: 0, provision_success_rate_7d: 0.0, active_incident: false, incident_description: None, quota_used_pct: 0.0, checked_at: Utc::now() })
    }
    fn supported_regions(&self) -> Vec<Region> { vec![] }
    fn supports_live_resize(&self) -> bool { false }
}

// ─── Hostinger provider (stub) ────────────────────────────────────────────────

#[derive(Debug)]
pub struct HostingerProvider {
    pub api_key: String,
}

#[async_trait]
impl Provider for HostingerProvider {
    fn name(&self) -> &str { "hostinger" }
    async fn provision(&self, _req: &ProvisionRequest) -> Result<ProvisionResult> { bail!("Hostinger provisioning not yet implemented") }
    async fn teardown(&self, _id: &str, _account_id: &str) -> Result<()> { bail!("Hostinger teardown not yet implemented") }
    async fn resize(&self, _id: &str, _tier: &InstanceTier) -> Result<ResizeResult> { bail!("Hostinger resize not yet implemented") }
    async fn provider_health(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth { provider: VpsProvider::Hostinger, api_reachable: false, health_score: 0, provision_avg_ms: 0, provision_success_rate_7d: 0.0, active_incident: false, incident_description: None, quota_used_pct: 0.0, checked_at: Utc::now() })
    }
    fn supported_regions(&self) -> Vec<Region> { vec![] }
    fn supports_live_resize(&self) -> bool { false }
}

// ─── DigitalOcean provider (stub) ─────────────────────────────────────────────

#[derive(Debug)]
pub struct DigitalOceanProvider {
    pub api_token: String,
}

#[async_trait]
impl Provider for DigitalOceanProvider {
    fn name(&self) -> &str { "digitalocean" }
    async fn provision(&self, _req: &ProvisionRequest) -> Result<ProvisionResult> { bail!("DigitalOcean provisioning not yet implemented") }
    async fn teardown(&self, _id: &str, _account_id: &str) -> Result<()> { bail!("DigitalOcean teardown not yet implemented") }
    async fn resize(&self, _id: &str, _tier: &InstanceTier) -> Result<ResizeResult> { bail!("DigitalOcean resize not yet implemented") }
    async fn provider_health(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth { provider: VpsProvider::DigitalOcean, api_reachable: false, health_score: 0, provision_avg_ms: 0, provision_success_rate_7d: 0.0, active_incident: false, incident_description: None, quota_used_pct: 0.0, checked_at: Utc::now() })
    }
    fn supported_regions(&self) -> Vec<Region> { vec![] }
    fn supports_live_resize(&self) -> bool { false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_spec_monthly_cost() {
        assert_eq!(TierSpec::monthly_cost(&InstanceTier::Nano), 4.00);
        assert_eq!(TierSpec::monthly_cost(&InstanceTier::Standard), 12.00);
        assert_eq!(TierSpec::monthly_cost(&InstanceTier::Pro), 24.00);
        assert_eq!(TierSpec::monthly_cost(&InstanceTier::Enterprise), 48.00);
    }

    #[test]
    fn test_hetzner_server_type_mapping() {
        assert_eq!(hetzner_server_type(&InstanceTier::Nano), "cx11");
        assert_eq!(hetzner_server_type(&InstanceTier::Standard), "cx21");
        assert_eq!(hetzner_server_type(&InstanceTier::Pro), "cx31");
        assert_eq!(hetzner_server_type(&InstanceTier::Enterprise), "cx41");
    }

    #[test]
    fn test_hetzner_regions() {
        let regions = HetznerProvider::regions();
        assert!(!regions.is_empty());
        assert!(regions.iter().any(|r| r.id == "eu-hetzner-nbg1"));
        assert!(regions.iter().any(|r| r.continent == Continent::EU));
    }

    #[test]
    fn test_hetzner_location_mapping() {
        assert_eq!(hetzner_location("eu-hetzner-nbg1"), "nbg1");
        assert_eq!(hetzner_location("eu-hetzner-hel1"), "hel1");
        assert_eq!(hetzner_location("unknown"), "nbg1");
    }

    #[test]
    fn test_tier_spec_all() {
        let all = TierSpec::all();
        assert!(all.contains_key("nano"));
        assert!(all.contains_key("enterprise"));
    }

    #[test]
    fn test_provider_registry_empty() {
        let registry = ProviderRegistry::new();
        assert!(registry.get("hetzner").is_none());
    }

    #[test]
    fn test_cloud_init_script_contains_key_elements() {
        let script = cloud_init_script(
            "i-test",
            "acc-1",
            "wss://gateway.example.com",
            "api-key-123",
            &InstanceRole::Primary,
            Some("i-standby"),
            "standard",
            "hetzner",
            "eu-hetzner-nbg1",
        );

        assert!(script.contains("i-test"));
        assert!(script.contains("acc-1"));
        assert!(script.contains("primary"));
        assert!(script.contains("clawnode"));
        assert!(script.contains("Docker"));
        assert!(script.contains("Tailscale"));
    }
}
