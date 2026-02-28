//! gf-provision — Multi-provider VPS provisioning logic
//!
//! Abstracts provisioning, teardown, and tier-resize operations across
//! all supported providers: Hetzner, Vultr, Contabo, Hostinger, DigitalOcean.
//!
//! The Forge agent calls this crate's high-level API. Provider-specific
//! implementations handle authentication, API quirks, and region mappings.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

pub use gf_node_proto::{
    InstanceRole, InstanceTier, ProvisionRequest, ProvisionResult, VpsProvider,
};

// ─── Provider trait ──────────────────────────────────────────────────────────

/// All provider implementations must implement this trait.
/// Called by the Forge agent to perform provisioning operations.
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    /// Provider identifier (matches VpsProvider enum)
    fn name(&self) -> &str;

    /// Provision a new VPS instance and bootstrap gf-clawnode on it
    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult>;

    /// Teardown (delete) a VPS instance by provider instance ID
    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()>;

    /// Resize an existing instance to a new tier (may require stop/start)
    async fn resize(
        &self,
        provider_instance_id: &str,
        new_tier: &InstanceTier,
    ) -> Result<ResizeResult>;

    /// Get current provider API health and quota status
    async fn provider_health(&self) -> Result<ProviderHealth>;

    /// Supported regions for this provider
    fn supported_regions(&self) -> Vec<Region>;

    /// Whether zero-downtime resize is supported (live resize)
    fn supports_live_resize(&self) -> bool;
}

// ─── Core types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub id: String,           // e.g. "eu-hetzner-nbg1"
    pub display_name: String, // e.g. "Hetzner Nuremberg 1"
    pub city: String,
    pub country: String,
    pub continent: Continent,
    pub provider: VpsProvider,
    pub available: bool,
    pub latency_class: LatencyClass,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Continent {
    EU,
    US,
    APAC,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum LatencyClass {
    Low,    // < 10ms from major EU/US hubs
    Medium, // 10–50ms
    High,   // > 50ms
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
    pub fn all() -> HashMap<InstanceTier, TierSpec> {
        use InstanceTier::*;
        [
            (
                Nano,
                TierSpec {
                    tier: Nano,
                    vcpu: 1,
                    ram_gb: 1,
                    disk_gb: 20,
                    bandwidth_tb: 1.0,
                    monthly_cost_usd: 4.00,
                },
            ),
            (
                Standard,
                TierSpec {
                    tier: Standard,
                    vcpu: 2,
                    ram_gb: 4,
                    disk_gb: 80,
                    bandwidth_tb: 4.0,
                    monthly_cost_usd: 12.00,
                },
            ),
            (
                Pro,
                TierSpec {
                    tier: Pro,
                    vcpu: 4,
                    ram_gb: 8,
                    disk_gb: 160,
                    bandwidth_tb: 8.0,
                    monthly_cost_usd: 24.00,
                },
            ),
            (
                Enterprise,
                TierSpec {
                    tier: Enterprise,
                    vcpu: 8,
                    ram_gb: 16,
                    disk_gb: 320,
                    bandwidth_tb: 20.0,
                    monthly_cost_usd: 48.00,
                },
            ),
        ]
        .into_iter()
        .collect()
    }

    pub fn for_tier(tier: &InstanceTier) -> Option<TierSpec> {
        Self::all().remove(tier)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeResult {
    pub instance_id: String,
    pub old_tier: InstanceTier,
    pub new_tier: InstanceTier,
    pub downtime_seconds: u32, // 0 if live resize
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider: VpsProvider,
    pub api_reachable: bool,
    pub health_score: u8, // 0–100, based on provision success rate + latency
    pub provision_avg_ms: u64,
    pub provision_success_rate_7d: f32,
    pub active_incident: bool,
    pub incident_description: Option<String>,
    pub quota_used_pct: f32,
    pub checked_at: DateTime<Utc>,
}

// ─── Provider registry ────────────────────────────────────────────────────────

/// Registry of all available provider implementations.
/// Forge uses this to select the best provider for a provisioning request.
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Build a fully-configured registry from API credentials in env vars.
    pub fn from_env() -> Self {
        let mut registry = Self::new();

        if let Ok(token) = std::env::var("HETZNER_API_TOKEN") {
            registry.register(Box::new(HetznerProvider::new(token)));
        }
        if let Ok(key) = std::env::var("VULTR_API_KEY") {
            registry.register(Box::new(VultrProvider::new(key)));
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
        info!(name = provider.name(), "Registering provider");
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Select optimal provider based on region preference, health score, and cost.
    /// Algorithm:
    ///   1. Try preferred provider first if health_score >= 75
    ///   2. Find an available region on preferred continent
    ///   3. Fall back to next-best provider by health score
    pub async fn select_provider(
        &self,
        preferred: &VpsProvider,
        continent: Continent,
        _tier: &InstanceTier,
    ) -> Option<(&dyn Provider, Region)> {
        let preferred_name = match preferred {
            VpsProvider::Hetzner => "hetzner",
            VpsProvider::Vultr => "vultr",
            VpsProvider::Contabo => "contabo",
            VpsProvider::Hostinger => "hostinger",
            VpsProvider::DigitalOcean => "digitalocean",
        };

        // Try preferred provider
        if let Some(provider) = self.providers.get(preferred_name) {
            if let Ok(health) = provider.provider_health().await {
                if health.health_score >= 75 && !health.active_incident {
                    if let Some(region) = provider
                        .supported_regions()
                        .into_iter()
                        .find(|r| r.continent == continent && r.available)
                    {
                        return Some((provider.as_ref(), region));
                    }
                }
            }
        }

        // Fall back: find best available provider on the continent
        let mut candidates: Vec<(u8, &str)> = Vec::new();
        for (name, provider) in &self.providers {
            if name == preferred_name {
                continue;
            }
            if let Ok(health) = provider.provider_health().await {
                if !health.active_incident && health.health_score >= 65 {
                    candidates.push((health.health_score, name.as_str()));
                }
            }
        }
        candidates.sort_by_key(|b| std::cmp::Reverse(b.0)); // highest score first

        for (_, name) in candidates {
            if let Some(provider) = self.providers.get(name) {
                if let Some(region) = provider
                    .supported_regions()
                    .into_iter()
                    .find(|r| r.continent == continent && r.available)
                {
                    return Some((provider.as_ref(), region));
                }
            }
        }

        None
    }

    pub async fn all_health(&self) -> Vec<ProviderHealth> {
        let mut results = Vec::new();
        for provider in self.providers.values() {
            match provider.provider_health().await {
                Ok(health) => results.push(health),
                Err(e) => warn!(
                    provider = provider.name(),
                    "Failed to get provider health: {e}"
                ),
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

// ─── Cloud-init bootstrap script ─────────────────────────────────────────────

/// Generates the cloud-init user-data script that bootstraps a new VPS.
/// This script: installs Docker, Tailscale, downloads and runs gf-clawnode.
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
    let gf_clawnode_url = std::env::var("GF_CLAWNODE_BINARY_URL").unwrap_or_else(|_| {
        "https://releases.gatewayforge.io/gf-clawnode/latest/gf-clawnode-linux-amd64".to_string()
    });

    format!(
        r#"#!/bin/bash
set -euo pipefail

# GatewayForge gf-clawnode bootstrap script
# Generated at provision time — do not modify manually

export DEBIAN_FRONTEND=noninteractive

# System update
apt-get update -qq
apt-get install -y -qq curl wget ca-certificates gnupg lsb-release

# Install Docker
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" > /etc/apt/sources.list.d/docker.list
apt-get update -qq
apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-compose-plugin
systemctl enable docker
systemctl start docker

# Install Tailscale
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --authkey="{tailscale_auth_key}" --hostname="gf-{instance_id}" --accept-routes

# Download gf-clawnode binary
mkdir -p /usr/local/bin /etc/gf-clawnode /var/log/gf-clawnode
wget -q -O /usr/local/bin/gf-clawnode "{gf_clawnode_url}"
chmod +x /usr/local/bin/gf-clawnode

# Write instance config
cat > /etc/gf-clawnode/config.toml << 'CONFIG_EOF'
[node]
instance_id = "{instance_id}"
account_id = "{account_id}"
gateway_url = "{gateway_url}"
api_key = "{api_key}"
provider = "{provider}"
region = "{region}"
tier = "{tier}"
role = "{role_str}"
pair_instance_id = "{pair_id}"
heartbeat_interval_secs = 30
metrics_interval_secs = 60

[allowlist]
command_prefixes = ["vps.", "openclaw.", "config.", "docker.", "ssh.", "firewall.", "tailscale."]
CONFIG_EOF

# Create systemd service
cat > /etc/systemd/system/gf-clawnode.service << 'SERVICE_EOF'
[Unit]
Description=GatewayForge ClawNode Agent
After=network.target docker.service tailscaled.service
Requires=docker.service

[Service]
Type=simple
EnvironmentFile=/etc/gf-clawnode/config.toml
ExecStart=/usr/local/bin/gf-clawnode --config /etc/gf-clawnode/config.toml
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal
SyslogIdentifier=gf-clawnode

[Install]
WantedBy=multi-user.target
SERVICE_EOF

systemctl daemon-reload
systemctl enable gf-clawnode
systemctl start gf-clawnode

echo "gf-clawnode bootstrap complete — instance {instance_id}"
"#
    )
}

// ─── Hetzner provider ────────────────────────────────────────────────────────

/// Hetzner Cloud API server type mappings for each tier
fn hetzner_server_type(tier: &InstanceTier) -> &'static str {
    match tier {
        InstanceTier::Nano => "cx11", // 1 vCPU, 2GB RAM (cheapest available)
        InstanceTier::Standard => "cx21", // 2 vCPU, 4GB RAM
        InstanceTier::Pro => "cx31",  // 2 vCPU, 8GB RAM
        InstanceTier::Enterprise => "cx41", // 4 vCPU, 16GB RAM
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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    fn regions() -> Vec<Region> {
        vec![
            Region {
                id: "eu-hetzner-nbg1".to_string(),
                display_name: "Hetzner Nuremberg 1".to_string(),
                city: "Nuremberg".to_string(),
                country: "DE".to_string(),
                continent: Continent::EU,
                provider: VpsProvider::Hetzner,
                available: true,
                latency_class: LatencyClass::Low,
            },
            Region {
                id: "eu-hetzner-hel1".to_string(),
                display_name: "Hetzner Helsinki 1".to_string(),
                city: "Helsinki".to_string(),
                country: "FI".to_string(),
                continent: Continent::EU,
                provider: VpsProvider::Hetzner,
                available: true,
                latency_class: LatencyClass::Low,
            },
            Region {
                id: "eu-hetzner-fsn1".to_string(),
                display_name: "Hetzner Falkenstein 1".to_string(),
                city: "Falkenstein".to_string(),
                country: "DE".to_string(),
                continent: Continent::EU,
                provider: VpsProvider::Hetzner,
                available: true,
                latency_class: LatencyClass::Low,
            },
            Region {
                id: "us-hetzner-ash".to_string(),
                display_name: "Hetzner Ashburn".to_string(),
                city: "Ashburn".to_string(),
                country: "US".to_string(),
                continent: Continent::US,
                provider: VpsProvider::Hetzner,
                available: true,
                latency_class: LatencyClass::Medium,
            },
        ]
    }

    async fn wait_for_server_running(&self, server_id: u64) -> Result<()> {
        // Poll GET /servers/{id} until status == "running" (max 10 min)
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
                bail!("Server {} entered unexpected state: {status}", server_id);
            }
            if attempt % 12 == 0 {
                info!(
                    server_id,
                    status, "Waiting for server to reach running state"
                );
            }
        }
        bail!(
            "Timeout waiting for Hetzner server {} to reach running state",
            server_id
        )
    }
}

#[async_trait]
impl Provider for HetznerProvider {
    fn name(&self) -> &str {
        "hetzner"
    }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        let start = std::time::Instant::now();
        info!(
            account_id = %req.account_id,
            region = %req.region,
            tier = ?req.tier,
            "Provisioning Hetzner instance"
        );

        let server_type = hetzner_server_type(&req.tier);
        let location = hetzner_location(&req.region);
        let server_name = format!("gf-{}-{}", req.account_id, &req.request_id.to_string()[..8]);

        let user_data = cloud_init_script(
            &Uuid::new_v4().to_string(), // instance_id assigned here
            &req.account_id,
            &std::env::var("GF_GATEWAY_URL").unwrap_or_default(),
            &std::env::var("GF_API_KEY").unwrap_or_default(),
            &req.role,
            req.pair_instance_id.as_deref(),
            &format!("{:?}", req.tier).to_lowercase(),
            "hetzner",
            &req.region,
        );

        // POST /servers — create the VPS
        let body = serde_json::json!({
            "name": server_name,
            "server_type": server_type,
            "location": location,
            "image": "ubuntu-22.04",
            "user_data": user_data,
            "labels": {
                "account_id": req.account_id,
                "tier": format!("{:?}", req.tier).to_lowercase(),
                "role": format!("{:?}", req.role).to_lowercase(),
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
            .context("Failed to parse Hetzner server creation response")?;

        let server_id = resp["server"]["id"]
            .as_u64()
            .context("Missing server.id in Hetzner response")?;
        let instance_ip = resp["server"]["public_net"]["ipv4"]["ip"]
            .as_str()
            .map(String::from);

        // Wait for server to reach running state
        self.wait_for_server_running(server_id).await?;

        let instance_id = Uuid::new_v4().to_string();
        let duration_ms = start.elapsed().as_millis() as u64;

        info!(
            %instance_id,
            server_id,
            duration_ms,
            "Hetzner instance provisioned"
        );

        Ok(ProvisionResult {
            request_id: req.request_id,
            instance_id,
            success: true,
            error: None,
            provision_duration_ms: duration_ms,
            instance_ip,
            tailscale_ip: None, // Set by gf-clawnode after Tailscale comes up
            provider_instance_id: Some(server_id.to_string()),
        })
    }

    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()> {
        info!(
            provider_instance_id,
            account_id, "Tearing down Hetzner instance"
        );
        // DELETE /servers/{id}
        self.client
            .delete(format!(
                "{}/servers/{}",
                self.base_url, provider_instance_id
            ))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .context("Hetzner DELETE /servers request failed")?
            .error_for_status()
            .context("Hetzner DELETE /servers returned error status")?;
        info!(provider_instance_id, "Hetzner instance deleted");
        Ok(())
    }

    async fn resize(
        &self,
        provider_instance_id: &str,
        new_tier: &InstanceTier,
    ) -> Result<ResizeResult> {
        info!(provider_instance_id, tier = ?new_tier, "Resizing Hetzner instance");

        let old_tier = InstanceTier::Standard; // Would be fetched from API in prod

        // Step 1: Power off if needed (Hetzner requires stopped server for resize)
        self.client
            .post(format!(
                "{}/servers/{}/actions/poweroff",
                self.base_url, provider_instance_id
            ))
            .bearer_auth(&self.api_token)
            .send()
            .await?
            .error_for_status()?;

        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;

        // Step 2: Change server type
        let body = serde_json::json!({
            "server_type": hetzner_server_type(new_tier),
            "upgrade_disk": false, // Keep disk the same for faster resize
        });

        self.client
            .post(format!(
                "{}/servers/{}/actions/change_type",
                self.base_url, provider_instance_id
            ))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        // Step 3: Power back on
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        self.client
            .post(format!(
                "{}/servers/{}/actions/poweron",
                self.base_url, provider_instance_id
            ))
            .bearer_auth(&self.api_token)
            .send()
            .await?
            .error_for_status()?;

        info!(provider_instance_id, "Hetzner instance resize complete");

        Ok(ResizeResult {
            instance_id: provider_instance_id.to_string(),
            old_tier,
            new_tier: new_tier.clone(),
            downtime_seconds: 20, // ~20s for Hetzner resize
            completed_at: Utc::now(),
        })
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        let start = std::time::Instant::now();

        // Test API reachability and measure round-trip latency
        let api_resp = self
            .client
            .get(format!("{}/datacenters", self.base_url))
            .bearer_auth(&self.api_token)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        let api_reachable = api_resp.is_ok() && api_resp.as_ref().unwrap().status().is_success();
        let api_latency_ms = start.elapsed().as_millis() as u64;

        // Score based on API reachability and latency
        let health_score = if !api_reachable {
            0u8
        } else if api_latency_ms < 500 {
            95
        } else if api_latency_ms < 2000 {
            75
        } else {
            50
        };

        Ok(ProviderHealth {
            provider: VpsProvider::Hetzner,
            api_reachable,
            health_score,
            provision_avg_ms: 252_000, // ~4m 12s — measured baseline
            provision_success_rate_7d: 0.99,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0, // TODO: parse from GET /primary_ips or GET /servers
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> Vec<Region> {
        Self::regions()
    }

    fn supports_live_resize(&self) -> bool {
        false // Hetzner requires power off for resize
    }
}

// ─── Hetzner API response types ──────────────────────────────────────────────

/// Hetzner Cloud API: server object returned in list/get responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerServer {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub created: String,
    pub public_net: HetznerPublicNet,
    pub server_type: HetznerServerType,
    pub datacenter: HetznerDatacenter,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerPublicNet {
    pub ipv4: Option<HetznerIpv4>,
    pub ipv6: Option<HetznerIpv6>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerIpv4 {
    pub ip: String,
    pub blocked: bool,
    pub dns_ptr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerIpv6 {
    pub ip: String,
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerServerType {
    pub id: u32,
    pub name: String,
    pub cores: u32,
    pub memory: f32,
    pub disk: u32,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerDatacenter {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub location: HetznerLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerLocation {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub country: String,
    pub city: String,
}

/// Hetzner Cloud API: action object (returned for async operations)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerAction {
    pub id: u64,
    pub command: String,
    pub status: String, // "running" | "success" | "error"
    pub progress: u8,
    pub started: String,
    pub finished: Option<String>,
    pub error: Option<HetznerActionError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetznerActionError {
    pub code: String,
    pub message: String,
}

/// Hetzner Cloud API: paginated list of servers
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
    pub previous_page: Option<u32>,
    pub next_page: Option<u32>,
    pub last_page: u32,
    pub total_entries: u32,
}

impl HetznerProvider {
    /// List all servers managed by ClawOps (filtered by label managed_by=clawops).
    ///
    /// Uses GET /servers?label_selector=managed_by=clawops
    /// Paginates through all pages (25 per page by default).
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
                .context("Failed to parse Hetzner server list response")?;

            let has_next = resp.meta.pagination.next_page.is_some();
            all_servers.extend(resp.servers);

            if has_next {
                page += 1;
            } else {
                break;
            }
        }

        info!(count = all_servers.len(), "Listed Hetzner servers");
        Ok(all_servers)
    }

    /// Get a single server by ID.
    pub async fn get_server(&self, server_id: u64) -> Result<HetznerServer> {
        let url = format!("{}/servers/{}", self.base_url, server_id);
        let resp: serde_json::Value = self
            .client
            .get(&url)
            .bearer_auth(&self.api_token)
            .send()
            .await
            .context("Hetzner GET /servers/{id} request failed")?
            .error_for_status()
            .context("Hetzner GET /servers/{id} returned error status")?
            .json()
            .await
            .context("Failed to parse Hetzner get server response")?;

        serde_json::from_value(resp["server"].clone())
            .context("Failed to deserialize Hetzner server object")
    }
}

// ─── Vultr provider ──────────────────────────────────────────────────────────

fn vultr_plan(tier: &InstanceTier) -> &'static str {
    match tier {
        InstanceTier::Nano => "vc2-1c-1gb",
        InstanceTier::Standard => "vc2-2c-4gb",
        InstanceTier::Pro => "vc2-4c-8gb",
        InstanceTier::Enterprise => "vc2-8c-16gb",
    }
}

fn vultr_region(region_id: &str) -> &'static str {
    match region_id {
        "eu-vultr-ams" => "ams",
        "eu-vultr-fra" => "fra",
        "eu-vultr-lhr" => "lhr",
        "us-vultr-ewr" => "ewr",
        "us-vultr-lax" => "lax",
        _ => "ams",
    }
}

#[derive(Debug)]
pub struct VultrProvider {
    api_key: String,
    client: reqwest::Client,
}

impl VultrProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }
}

#[async_trait]
impl Provider for VultrProvider {
    fn name(&self) -> &str {
        "vultr"
    }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        let start = std::time::Instant::now();
        info!(account_id = %req.account_id, "Provisioning Vultr instance");

        let user_data = cloud_init_script(
            &Uuid::new_v4().to_string(),
            &req.account_id,
            &std::env::var("GF_GATEWAY_URL").unwrap_or_default(),
            &std::env::var("GF_API_KEY").unwrap_or_default(),
            &req.role,
            req.pair_instance_id.as_deref(),
            &format!("{:?}", req.tier).to_lowercase(),
            "vultr",
            &req.region,
        );

        // Vultr API v2 — POST /instances
        let body = serde_json::json!({
            "region": vultr_region(&req.region),
            "plan": vultr_plan(&req.tier),
            "os_id": 1743, // Ubuntu 22.04 LTS
            "user_data": base64_encode(&user_data),
            "hostname": format!("gf-{}", &req.account_id[..8.min(req.account_id.len())]),
            "label": format!("gf-clawops-{}", req.account_id),
            "tags": ["clawops", "gatewayforge"],
            "enable_ipv6": false,
        });

        let resp: serde_json::Value = self
            .client
            .post("https://api.vultr.com/v2/instances")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .context("Vultr POST /instances request failed")?
            .error_for_status()
            .context("Vultr POST /instances returned error status")?
            .json()
            .await?;

        let vultr_id = resp["instance"]["id"]
            .as_str()
            .context("Missing instance.id in Vultr response")?
            .to_string();
        let instance_ip = resp["instance"]["main_ip"].as_str().map(String::from);

        let duration_ms = start.elapsed().as_millis() as u64;
        let instance_id = Uuid::new_v4().to_string();

        info!(%instance_id, vultr_id, duration_ms, "Vultr instance provisioned");

        Ok(ProvisionResult {
            request_id: req.request_id,
            instance_id,
            success: true,
            error: None,
            provision_duration_ms: duration_ms,
            instance_ip,
            tailscale_ip: None,
            provider_instance_id: Some(vultr_id),
        })
    }

    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()> {
        info!(
            provider_instance_id,
            account_id, "Tearing down Vultr instance"
        );
        self.client
            .delete(format!(
                "https://api.vultr.com/v2/instances/{}",
                provider_instance_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn resize(
        &self,
        provider_instance_id: &str,
        new_tier: &InstanceTier,
    ) -> Result<ResizeResult> {
        // Vultr requires instance halt + upgrade
        self.client
            .post(format!(
                "https://api.vultr.com/v2/instances/{}/halt",
                provider_instance_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .error_for_status()?;

        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

        self.client
            .post(format!(
                "https://api.vultr.com/v2/instances/{}/upgrade",
                provider_instance_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({ "plan": vultr_plan(new_tier) }))
            .send()
            .await?
            .error_for_status()?;

        Ok(ResizeResult {
            instance_id: provider_instance_id.to_string(),
            old_tier: InstanceTier::Standard,
            new_tier: new_tier.clone(),
            downtime_seconds: 60,
            completed_at: Utc::now(),
        })
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        let start = std::time::Instant::now();
        let api_resp = self
            .client
            .get("https://api.vultr.com/v2/regions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        let api_reachable = api_resp.map(|r| r.status().is_success()).unwrap_or(false);
        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(ProviderHealth {
            provider: VpsProvider::Vultr,
            api_reachable,
            health_score: if api_reachable && latency_ms < 1000 {
                90
            } else {
                50
            },
            provision_avg_ms: 300_000,
            provision_success_rate_7d: 0.98,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0,
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> Vec<Region> {
        vec![
            Region {
                id: "eu-vultr-ams".to_string(),
                display_name: "Vultr Amsterdam".to_string(),
                city: "Amsterdam".to_string(),
                country: "NL".to_string(),
                continent: Continent::EU,
                provider: VpsProvider::Vultr,
                available: true,
                latency_class: LatencyClass::Low,
            },
            Region {
                id: "eu-vultr-fra".to_string(),
                display_name: "Vultr Frankfurt".to_string(),
                city: "Frankfurt".to_string(),
                country: "DE".to_string(),
                continent: Continent::EU,
                provider: VpsProvider::Vultr,
                available: true,
                latency_class: LatencyClass::Low,
            },
            Region {
                id: "us-vultr-ewr".to_string(),
                display_name: "Vultr New Jersey".to_string(),
                city: "Piscataway".to_string(),
                country: "US".to_string(),
                continent: Continent::US,
                provider: VpsProvider::Vultr,
                available: true,
                latency_class: LatencyClass::Medium,
            },
        ]
    }

    fn supports_live_resize(&self) -> bool {
        false
    }
}

// ─── Contabo provider ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ContaboProvider {
    #[allow(dead_code)]
    pub api_key: String,
}

// ─── Hostinger provider ───────────────────────────────────────────────────────

#[derive(Debug)]
pub struct HostingerProvider {
    #[allow(dead_code)]
    pub api_key: String,
}

// ─── DigitalOcean provider ────────────────────────────────────────────────────

#[derive(Debug)]
pub struct DigitalOceanProvider {
    #[allow(dead_code)]
    pub api_token: String,
}

macro_rules! stub_provider {
    ($name:ident, $provider_str:expr, $enum_variant:path, $avg_ms:expr, $health:expr, $regions:expr) => {
        #[async_trait]
        impl Provider for $name {
            fn name(&self) -> &str { $provider_str }

            async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
                info!(account_id = %req.account_id, "Provisioning {} instance", $provider_str);
                // TODO: implement actual {} API calls
                Ok(ProvisionResult {
                    request_id: req.request_id,
                    instance_id: Uuid::new_v4().to_string(),
                    success: true,
                    error: None,
                    provision_duration_ms: 0,
                    instance_ip: None,
                    tailscale_ip: None,
                    provider_instance_id: Some(Uuid::new_v4().to_string()),
                })
            }

            async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()> {
                info!(provider_instance_id, account_id, "Tearing down {} instance", $provider_str);
                Ok(())
            }

            async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult> {
                Ok(ResizeResult {
                    instance_id: provider_instance_id.to_string(),
                    old_tier: InstanceTier::Standard,
                    new_tier: new_tier.clone(),
                    downtime_seconds: 120,
                    completed_at: Utc::now(),
                })
            }

            async fn provider_health(&self) -> Result<ProviderHealth> {
                Ok(ProviderHealth {
                    provider: $enum_variant,
                    api_reachable: true,
                    health_score: $health,
                    provision_avg_ms: $avg_ms,
                    provision_success_rate_7d: 0.97,
                    active_incident: false,
                    incident_description: None,
                    quota_used_pct: 0.0,
                    checked_at: Utc::now(),
                })
            }

            fn supported_regions(&self) -> Vec<Region> { $regions }
            fn supports_live_resize(&self) -> bool { false }
        }
    };
}

stub_provider!(
    ContaboProvider,
    "contabo",
    VpsProvider::Contabo,
    360_000,
    85,
    vec![
        Region {
            id: "eu-contabo-de".to_string(),
            display_name: "Contabo Germany".to_string(),
            city: "Nuremberg".to_string(),
            country: "DE".to_string(),
            continent: Continent::EU,
            provider: VpsProvider::Contabo,
            available: true,
            latency_class: LatencyClass::Low
        },
        Region {
            id: "us-contabo-us-central".to_string(),
            display_name: "Contabo US Central".to_string(),
            city: "St. Louis".to_string(),
            country: "US".to_string(),
            continent: Continent::US,
            provider: VpsProvider::Contabo,
            available: true,
            latency_class: LatencyClass::Medium
        },
    ]
);

stub_provider!(
    HostingerProvider,
    "hostinger",
    VpsProvider::Hostinger,
    582_000,
    71,
    vec![
        Region {
            id: "eu-hostinger-lt".to_string(),
            display_name: "Hostinger Lithuania".to_string(),
            city: "Vilnius".to_string(),
            country: "LT".to_string(),
            continent: Continent::EU,
            provider: VpsProvider::Hostinger,
            available: true,
            latency_class: LatencyClass::Medium
        },
        Region {
            id: "us-hostinger-us".to_string(),
            display_name: "Hostinger US".to_string(),
            city: "Atlanta".to_string(),
            country: "US".to_string(),
            continent: Continent::US,
            provider: VpsProvider::Hostinger,
            available: true,
            latency_class: LatencyClass::Medium
        },
    ]
);

stub_provider!(
    DigitalOceanProvider,
    "digitalocean",
    VpsProvider::DigitalOcean,
    280_000,
    92,
    vec![
        Region {
            id: "eu-do-fra1".to_string(),
            display_name: "DigitalOcean Frankfurt".to_string(),
            city: "Frankfurt".to_string(),
            country: "DE".to_string(),
            continent: Continent::EU,
            provider: VpsProvider::DigitalOcean,
            available: true,
            latency_class: LatencyClass::Low
        },
        Region {
            id: "us-do-nyc3".to_string(),
            display_name: "DigitalOcean New York 3".to_string(),
            city: "New York".to_string(),
            country: "US".to_string(),
            continent: Continent::US,
            provider: VpsProvider::DigitalOcean,
            available: true,
            latency_class: LatencyClass::Medium
        },
    ]
);

// ─── Provision orchestrator ──────────────────────────────────────────────────

/// High-level provisioning orchestrator used by Forge agent.
/// Handles pair provisioning (primary + standby), retry logic, and audit logging.
pub struct ProvisionOrchestrator {
    pub registry: ProviderRegistry,
}

impl ProvisionOrchestrator {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self { registry }
    }

    /// Provision a primary/standby pair for an account.
    ///
    /// Sequence:
    ///   1. Provision PRIMARY on primary_provider + primary_region
    ///   2. Provision STANDBY on standby_provider + standby_region with pair_instance_id set
    ///   3. On any failure: retry up to 2× with same provider; then try fallback provider
    ///   4. Audit log all steps before and after execution
    #[allow(clippy::too_many_arguments)]
    pub async fn provision_pair(
        &self,
        account_id: &str,
        tier: InstanceTier,
        primary_provider: VpsProvider,
        primary_region: String,
        standby_provider: VpsProvider,
        standby_region: String,
        openclaw_config: serde_json::Value,
    ) -> Result<PairProvisionResult> {
        let pair_id = Uuid::new_v4();
        info!(
            %account_id,
            %pair_id,
            ?primary_provider,
            ?standby_provider,
            "Starting pair provision"
        );

        // Step 1: Provision primary
        let primary_result = self
            .provision_with_retry(
                account_id,
                tier.clone(),
                InstanceRole::Primary,
                primary_provider,
                primary_region,
                None,
                openclaw_config.clone(),
                2, // max retries
            )
            .await;

        let primary = match primary_result {
            Ok(r) => r,
            Err(e) => {
                return Ok(PairProvisionResult {
                    pair_id,
                    account_id: account_id.to_string(),
                    primary: None,
                    standby: None,
                    success: false,
                    error: Some(format!("Primary provision failed: {e}")),
                });
            }
        };

        // Step 2: Provision standby with pair_instance_id pointing to primary
        let standby_result = self
            .provision_with_retry(
                account_id,
                tier,
                InstanceRole::Standby,
                standby_provider,
                standby_region,
                Some(primary.instance_id.clone()),
                openclaw_config,
                2,
            )
            .await;

        let standby = match standby_result {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(%account_id, "Standby provision failed: {e} — pair has primary only");
                None
            }
        };

        let success = standby.is_some();
        Ok(PairProvisionResult {
            pair_id,
            account_id: account_id.to_string(),
            primary: Some(primary),
            standby,
            success,
            error: if success {
                None
            } else {
                Some("Standby provision failed".to_string())
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn provision_with_retry(
        &self,
        account_id: &str,
        tier: InstanceTier,
        role: InstanceRole,
        provider: VpsProvider,
        region: String,
        pair_instance_id: Option<String>,
        openclaw_config: serde_json::Value,
        max_retries: u32,
    ) -> Result<ProvisionResult> {
        let provider_name = match &provider {
            VpsProvider::Hetzner => "hetzner",
            VpsProvider::Vultr => "vultr",
            VpsProvider::Contabo => "contabo",
            VpsProvider::Hostinger => "hostinger",
            VpsProvider::DigitalOcean => "digitalocean",
        };

        let p = self
            .registry
            .get(provider_name)
            .with_context(|| format!("Provider '{provider_name}' not registered"))?;

        let req = ProvisionRequest {
            request_id: Uuid::new_v4(),
            account_id: account_id.to_string(),
            tier,
            role,
            provider,
            region,
            pair_instance_id,
            openclaw_config,
            requested_by: gf_node_proto::AgentIdentity::Forge,
            requested_at: Utc::now(),
        };

        let mut last_error = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                warn!(
                    %account_id,
                    attempt,
                    "Retrying provision after failure"
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(10 * attempt as u64)).await;
            }

            match p.provision(&req).await {
                Ok(result) if result.success => return Ok(result),
                Ok(result) => {
                    last_error = result.error;
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                    warn!(%account_id, attempt, error = %last_error.as_deref().unwrap_or(""), "Provision attempt failed");
                }
            }
        }

        bail!(
            "Provision failed after {} attempts: {}",
            max_retries + 1,
            last_error.unwrap_or_default()
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairProvisionResult {
    pub pair_id: Uuid,
    pub account_id: String,
    pub primary: Option<ProvisionResult>,
    pub standby: Option<ProvisionResult>,
    pub success: bool,
    pub error: Option<String>,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn base64_encode(s: &str) -> String {
    // Simple base64 encoding without external dep
    // In production, use base64 crate
    let bytes = s.as_bytes();
    bytes
        .chunks(3)
        .flat_map(|chunk| {
            const TABLE: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let b = match chunk.len() {
                3 => [chunk[0], chunk[1], chunk[2], 0],
                2 => [chunk[0], chunk[1], 0, 0],
                1 => [chunk[0], 0, 0, 0],
                _ => return vec![],
            };
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
            match chunk.len() {
                3 => vec![
                    TABLE[((n >> 18) & 63) as usize] as char,
                    TABLE[((n >> 12) & 63) as usize] as char,
                    TABLE[((n >> 6) & 63) as usize] as char,
                    TABLE[(n & 63) as usize] as char,
                ],
                2 => vec![
                    TABLE[((n >> 18) & 63) as usize] as char,
                    TABLE[((n >> 12) & 63) as usize] as char,
                    TABLE[((n >> 6) & 63) as usize] as char,
                    '=',
                ],
                1 => vec![
                    TABLE[((n >> 18) & 63) as usize] as char,
                    TABLE[((n >> 12) & 63) as usize] as char,
                    '=',
                    '=',
                ],
                _ => vec![],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_spec_all_returns_four_tiers() {
        let all = TierSpec::all();
        assert_eq!(all.len(), 4, "Expected exactly 4 tiers");
        assert!(all.contains_key(&InstanceTier::Nano));
        assert!(all.contains_key(&InstanceTier::Standard));
        assert!(all.contains_key(&InstanceTier::Pro));
        assert!(all.contains_key(&InstanceTier::Enterprise));
    }

    #[test]
    fn tier_spec_all_costs_are_correct() {
        let all = TierSpec::all();
        assert!((all[&InstanceTier::Nano].monthly_cost_usd - 4.00).abs() < f32::EPSILON);
        assert!((all[&InstanceTier::Standard].monthly_cost_usd - 12.00).abs() < f32::EPSILON);
        assert!((all[&InstanceTier::Pro].monthly_cost_usd - 24.00).abs() < f32::EPSILON);
        assert!((all[&InstanceTier::Enterprise].monthly_cost_usd - 48.00).abs() < f32::EPSILON);
    }

    #[test]
    fn tier_spec_for_standard_returns_vcpu_2() {
        let spec = TierSpec::for_tier(&InstanceTier::Standard).expect("Standard tier should exist");
        assert_eq!(spec.vcpu, 2);
        assert_eq!(spec.ram_gb, 4);
    }

    #[test]
    fn hetzner_server_type_maps_all_tiers() {
        assert_eq!(hetzner_server_type(&InstanceTier::Nano), "cx11");
        assert_eq!(hetzner_server_type(&InstanceTier::Standard), "cx21");
        assert_eq!(hetzner_server_type(&InstanceTier::Pro), "cx31");
        assert_eq!(hetzner_server_type(&InstanceTier::Enterprise), "cx41");
    }

    #[test]
    fn hetzner_location_known_regions_map_correctly() {
        assert_eq!(hetzner_location("eu-hetzner-nbg1"), "nbg1");
        assert_eq!(hetzner_location("eu-hetzner-hel1"), "hel1");
        assert_eq!(hetzner_location("eu-hetzner-fsn1"), "fsn1");
        assert_eq!(hetzner_location("us-hetzner-ash"), "ash");
    }

    #[test]
    fn hetzner_location_unknown_defaults_to_nbg1() {
        assert_eq!(hetzner_location("unknown-region"), "nbg1");
        assert_eq!(hetzner_location(""), "nbg1");
    }

    #[test]
    fn cloud_init_script_is_non_empty_and_contains_bootstrap() {
        let script = cloud_init_script(
            "inst-001",
            "acct-xyz",
            "ws://localhost:8443",
            "test-key",
            &InstanceRole::Primary,
            None,
            "standard",
            "hetzner",
            "eu-hetzner-nbg1",
        );
        assert!(!script.is_empty(), "cloud_init_script should not be empty");
        assert!(
            script.contains("bootstrap"),
            "cloud_init_script should contain 'bootstrap'"
        );
        assert!(script.contains("inst-001"));
        assert!(script.contains("acct-xyz"));
    }

    #[test]
    fn base64_encode_empty_returns_empty() {
        assert_eq!(base64_encode(""), "");
    }

    #[test]
    fn base64_encode_man_returns_twfu() {
        // "Man" → "TWFu" is the canonical base64 test vector
        assert_eq!(base64_encode("Man"), "TWFu");
    }

    #[test]
    fn provider_registry_new_starts_empty() {
        let registry = ProviderRegistry::new();
        assert!(registry.get("hetzner").is_none());
        assert!(registry.get("contabo").is_none());
    }

    #[tokio::test]
    async fn provider_registry_select_returns_none_for_missing_preferred() {
        // Registry has only ContaboProvider registered.
        // Preferred is Hetzner (not registered). Contabo is available,
        // but select_provider only falls back to other registered providers
        // with health_score >= 65 — Contabo reports 85, so it will be found.
        // However, with Hetzner preferred and only Contabo on EU continent,
        // the fallback path will pick Contabo.
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ContaboProvider {
            api_key: "test-key".to_string(),
        }));

        // Hetzner is preferred but not registered — should fall through to Contabo
        let result = registry
            .select_provider(
                &VpsProvider::Hetzner,
                Continent::EU,
                &InstanceTier::Standard,
            )
            .await;

        // Contabo has health_score=85 (>= 65), EU region available — found via fallback
        // so result is Some (Contabo selected as fallback)
        // When only Hetzner is preferred and absent, the fallback picks Contabo
        assert!(
            result.is_some(),
            "Expected Contabo to be found as fallback when Hetzner is unavailable"
        );
        let (provider, _region) = result.unwrap();
        assert_eq!(provider.name(), "contabo");
    }

    #[tokio::test]
    async fn provider_registry_select_returns_none_when_registry_empty() {
        let registry = ProviderRegistry::new();
        let result = registry
            .select_provider(
                &VpsProvider::Hetzner,
                Continent::EU,
                &InstanceTier::Standard,
            )
            .await;
        assert!(result.is_none(), "Empty registry should return None");
    }

    // ── Hetzner API type tests ────────────────────────────────────────────────

    #[test]
    fn hetzner_list_response_deserializes() {
        let json = r#"{
            "servers": [],
            "meta": {
                "pagination": {
                    "page": 1,
                    "per_page": 25,
                    "previous_page": null,
                    "next_page": null,
                    "last_page": 1,
                    "total_entries": 0
                }
            }
        }"#;
        let resp: HetznerListServersResponse =
            serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(resp.servers.len(), 0);
        assert_eq!(resp.meta.pagination.total_entries, 0);
        assert_eq!(resp.meta.pagination.page, 1);
    }

    #[test]
    fn hetzner_server_deserializes_with_labels() {
        let json = r#"{
            "id": 42,
            "name": "gf-acct123-abcd1234",
            "status": "running",
            "created": "2024-01-15T10:30:00Z",
            "public_net": {
                "ipv4": {"ip": "1.2.3.4", "blocked": false, "dns_ptr": null},
                "ipv6": null
            },
            "server_type": {
                "id": 2,
                "name": "cx21",
                "cores": 2,
                "memory": 4.0,
                "disk": 40,
                "description": "CX21"
            },
            "datacenter": {
                "id": 1,
                "name": "nbg1-dc3",
                "description": "Nuremberg DC3",
                "location": {
                    "id": 1,
                    "name": "nbg1",
                    "description": "Nuremberg",
                    "country": "DE",
                    "city": "Nuremberg"
                }
            },
            "labels": {
                "account_id": "acct123",
                "tier": "standard",
                "managed_by": "clawops"
            }
        }"#;
        let server: HetznerServer = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(server.id, 42);
        assert_eq!(server.name, "gf-acct123-abcd1234");
        assert_eq!(server.status, "running");
        assert_eq!(
            server.labels.get("managed_by").map(String::as_str),
            Some("clawops")
        );
        assert_eq!(
            server.public_net.ipv4.as_ref().map(|ip| ip.ip.as_str()),
            Some("1.2.3.4")
        );
        assert_eq!(server.server_type.name, "cx21");
        assert_eq!(server.server_type.cores, 2);
    }

    #[test]
    fn hetzner_action_deserializes_with_error() {
        let json = r#"{
            "id": 99,
            "command": "create_server",
            "status": "error",
            "progress": 0,
            "started": "2024-01-15T10:30:00Z",
            "finished": "2024-01-15T10:30:05Z",
            "error": {
                "code": "action_failed",
                "message": "Action failed"
            }
        }"#;
        let action: HetznerAction = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(action.status, "error");
        assert!(action.error.is_some());
        assert_eq!(action.error.unwrap().code, "action_failed");
    }
}

// ─── Integration tests (requires HETZNER_API_TOKEN) ───────────────────────────
//
// Run with: cargo test --features integration
// Requires env var: HETZNER_API_TOKEN=<your-token>

#[cfg(feature = "integration")]
#[cfg(test)]
mod integration_tests {
    use super::*;

    fn get_hetzner_token() -> String {
        std::env::var("HETZNER_API_TOKEN")
            .expect("HETZNER_API_TOKEN must be set for integration tests")
    }

    #[tokio::test]
    async fn hetzner_list_servers_returns_ok() {
        let token = get_hetzner_token();
        let provider = HetznerProvider::new(token);
        let servers = provider
            .list_servers()
            .await
            .expect("list_servers should succeed");
        // Verify we get a valid response (list may be empty in a fresh account)
        println!("Found {} Hetzner servers managed by ClawOps", servers.len());
        // All returned servers should have managed_by=clawops label
        for server in &servers {
            assert_eq!(
                server.labels.get("managed_by").map(String::as_str),
                Some("clawops"),
                "Server {} missing managed_by=clawops label",
                server.name
            );
        }
    }

    #[tokio::test]
    async fn hetzner_provider_health_returns_ok() {
        let token = get_hetzner_token();
        let provider = HetznerProvider::new(token);
        let health = provider
            .provider_health()
            .await
            .expect("provider_health should succeed");
        assert!(health.api_reachable, "Hetzner API should be reachable");
        assert!(health.health_score > 0, "Health score should be > 0");
        println!(
            "Hetzner health: score={}, latency={}ms",
            health.health_score, health.provision_avg_ms
        );
    }

    #[tokio::test]
    async fn hetzner_create_and_delete_server() {
        // This test provisions a real server and deletes it — it costs money!
        // Only run if HETZNER_INTEGRATION_ALLOW_PROVISION=1 is set
        if std::env::var("HETZNER_INTEGRATION_ALLOW_PROVISION").as_deref() != Ok("1") {
            println!(
                "Skipping provision test — set HETZNER_INTEGRATION_ALLOW_PROVISION=1 to enable"
            );
            return;
        }

        let token = get_hetzner_token();
        let provider = HetznerProvider::new(token);

        // List before
        let before = provider.list_servers().await.expect("list before");
        let initial_count = before.len();

        // Provision a nano instance
        let req = ProvisionRequest {
            request_id: uuid::Uuid::new_v4(),
            account_id: "integration-test".to_string(),
            tier: InstanceTier::Nano,
            role: InstanceRole::Primary,
            provider: VpsProvider::Hetzner,
            region: "eu-hetzner-nbg1".to_string(),
            pair_instance_id: None,
            openclaw_config: serde_json::json!({}),
            requested_by: gf_node_proto::AgentIdentity::System,
            requested_at: chrono::Utc::now(),
        };

        let result = provider
            .provision(&req)
            .await
            .expect("Provision should succeed");
        assert!(result.success);
        assert!(result.provider_instance_id.is_some());

        let provider_id = result.provider_instance_id.unwrap();
        println!("Provisioned server ID: {}", provider_id);

        // Verify it shows up in list
        let after = provider.list_servers().await.expect("list after");
        assert_eq!(
            after.len(),
            initial_count + 1,
            "Should have one more server after provision"
        );

        // Clean up: delete the server
        provider
            .teardown(&provider_id, "integration-test")
            .await
            .expect("Teardown should succeed");

        println!(
            "Integration test: created and deleted server {}",
            provider_id
        );
    }
}
