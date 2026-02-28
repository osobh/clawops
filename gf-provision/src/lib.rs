//! gf-provision — Multi-provider VPS provisioning logic
//!
//! Abstracts provisioning, teardown, and tier-resize operations across
//! all supported providers: Hetzner, Vultr, Contabo, Hostinger, DigitalOcean.
//!
//! The Forge agent calls this crate's high-level API. Provider-specific
//! implementations handle authentication, API quirks, and region mappings.

use anyhow::Result;
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
    async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult>;

    /// Get current provider API health and quota status
    async fn provider_health(&self) -> Result<ProviderHealth>;

    /// Supported regions for this provider
    fn supported_regions(&self) -> &[Region];

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
    pub fn specs() -> HashMap<InstanceTier, TierSpec> {
        use InstanceTier::*;
        [
            (Nano,       TierSpec { tier: Nano,       vcpu: 1, ram_gb: 1,  disk_gb: 20,  bandwidth_tb: 1.0,  monthly_cost_usd: 4.00 }),
            (Standard,   TierSpec { tier: Standard,   vcpu: 2, ram_gb: 4,  disk_gb: 80,  bandwidth_tb: 4.0,  monthly_cost_usd: 12.00 }),
            (Pro,        TierSpec { tier: Pro,        vcpu: 4, ram_gb: 8,  disk_gb: 160, bandwidth_tb: 8.0,  monthly_cost_usd: 24.00 }),
            (Enterprise, TierSpec { tier: Enterprise, vcpu: 8, ram_gb: 16, disk_gb: 320, bandwidth_tb: 20.0, monthly_cost_usd: 48.00 }),
        ]
        .into_iter()
        .collect()
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

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        info!(name = provider.name(), "Registering provider");
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Select optimal provider based on region preference, health score, and cost
    pub async fn select_provider(
        &self,
        preferred: &VpsProvider,
        continent: Continent,
        tier: &InstanceTier,
    ) -> Option<(&dyn Provider, Region)> {
        // 1. Try preferred provider first
        // 2. Check health score >= 75
        // 3. Find available region in preferred continent
        // 4. Fall back to next-best provider if preferred is degraded
        let _ = (preferred, continent, tier);
        // TODO: implement provider selection algorithm
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

// ─── Hetzner provider ────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct HetznerProvider {
    api_token: String,
    base_url: String,
}

impl HetznerProvider {
    pub fn new(api_token: String) -> Self {
        Self {
            api_token,
            base_url: "https://api.hetzner.cloud/v1".to_string(),
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
        ]
    }
}

#[async_trait]
impl Provider for HetznerProvider {
    fn name(&self) -> &str {
        "hetzner"
    }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        info!(
            account_id = %req.account_id,
            region = %req.region,
            tier = ?req.tier,
            "Provisioning Hetzner instance"
        );
        // TODO: call Hetzner Cloud API:
        // POST /servers — create VPS with cloud-init script that installs gf-clawnode
        // Wait for server state == "running"
        // Install Tailscale via API or cloud-init
        // Bootstrap gf-clawnode with instance config
        let _ = &self.api_token;
        Ok(ProvisionResult {
            request_id: req.request_id,
            instance_id: Uuid::new_v4().to_string(),
            success: true,
            error: None,
            provision_duration_ms: 0,
            instance_ip: None,
            tailscale_ip: None,
            provider_instance_id: None,
        })
    }

    async fn teardown(&self, provider_instance_id: &str, account_id: &str) -> Result<()> {
        info!(provider_instance_id, account_id, "Tearing down Hetzner instance");
        // TODO: DELETE /servers/{id}
        Ok(())
    }

    async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult> {
        info!(provider_instance_id, tier = ?new_tier, "Resizing Hetzner instance");
        // TODO: POST /servers/{id}/actions/change_type
        // Hetzner supports live resize for upgrades (no downtime)
        Ok(ResizeResult {
            instance_id: provider_instance_id.to_string(),
            old_tier: InstanceTier::Standard,
            new_tier: new_tier.clone(),
            downtime_seconds: 0,
            completed_at: Utc::now(),
        })
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        // TODO: check https://status.hetzner.com/ + test API round-trip latency
        Ok(ProviderHealth {
            provider: VpsProvider::Hetzner,
            api_reachable: true,
            health_score: 95,
            provision_avg_ms: 252_000, // ~4min 12s average
            provision_success_rate_7d: 0.99,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0,
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> &[Region] {
        // TODO: return static or cached list
        &[]
    }

    fn supports_live_resize(&self) -> bool {
        true // Hetzner supports live resize for upgrades
    }
}

// ─── Vultr provider ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VultrProvider {
    api_key: String,
}

impl VultrProvider {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl Provider for VultrProvider {
    fn name(&self) -> &str { "vultr" }

    async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
        info!(account_id = %req.account_id, "Provisioning Vultr instance");
        let _ = &self.api_key;
        // TODO: Vultr API v2 — POST /instances
        Ok(ProvisionResult {
            request_id: req.request_id,
            instance_id: Uuid::new_v4().to_string(),
            success: true,
            error: None,
            provision_duration_ms: 0,
            instance_ip: None,
            tailscale_ip: None,
            provider_instance_id: None,
        })
    }

    async fn teardown(&self, _provider_instance_id: &str, _account_id: &str) -> Result<()> {
        // TODO: DELETE /instances/{instance-id}
        Ok(())
    }

    async fn resize(&self, provider_instance_id: &str, new_tier: &InstanceTier) -> Result<ResizeResult> {
        Ok(ResizeResult {
            instance_id: provider_instance_id.to_string(),
            old_tier: InstanceTier::Standard,
            new_tier: new_tier.clone(),
            downtime_seconds: 60, // Vultr requires stop/start for resize
            completed_at: Utc::now(),
        })
    }

    async fn provider_health(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth {
            provider: VpsProvider::Vultr,
            api_reachable: true,
            health_score: 90,
            provision_avg_ms: 300_000,
            provision_success_rate_7d: 0.98,
            active_incident: false,
            incident_description: None,
            quota_used_pct: 0.0,
            checked_at: Utc::now(),
        })
    }

    fn supported_regions(&self) -> &[Region] { &[] }
    fn supports_live_resize(&self) -> bool { false }
}

// ─── Placeholder providers (Contabo, Hostinger, DigitalOcean) ────────────────

#[derive(Debug)]
pub struct ContaboProvider { pub api_key: String }

#[derive(Debug)]
pub struct HostingerProvider { pub api_key: String }

#[derive(Debug)]
pub struct DigitalOceanProvider { pub api_token: String }

macro_rules! stub_provider {
    ($name:ident, $provider_str:expr, $enum_variant:path, $avg_ms:expr, $health:expr) => {
        #[async_trait]
        impl Provider for $name {
            fn name(&self) -> &str { $provider_str }

            async fn provision(&self, req: &ProvisionRequest) -> Result<ProvisionResult> {
                info!(account_id = %req.account_id, "Provisioning {} instance", $provider_str);
                Ok(ProvisionResult {
                    request_id: req.request_id,
                    instance_id: Uuid::new_v4().to_string(),
                    success: true,
                    error: None,
                    provision_duration_ms: 0,
                    instance_ip: None,
                    tailscale_ip: None,
                    provider_instance_id: None,
                })
            }

            async fn teardown(&self, _provider_instance_id: &str, _account_id: &str) -> Result<()> {
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

            fn supported_regions(&self) -> &[Region] { &[] }
            fn supports_live_resize(&self) -> bool { false }
        }
    };
}

stub_provider!(ContaboProvider,      "contabo",      VpsProvider::Contabo,      360_000, 85);
stub_provider!(HostingerProvider,    "hostinger",    VpsProvider::Hostinger,    582_000, 71);
stub_provider!(DigitalOceanProvider, "digitalocean", VpsProvider::DigitalOcean, 280_000, 92);

// ─── Provision orchestrator ──────────────────────────────────────────────────

/// High-level provisioning orchestrator used by Forge agent.
/// Handles pair provisioning (primary + standby), retry logic, and audit logging.
pub struct ProvisionOrchestrator {
    registry: ProviderRegistry,
}

impl ProvisionOrchestrator {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self { registry }
    }

    /// Provision a primary/standby pair for an account
    pub async fn provision_pair(
        &self,
        account_id: &str,
        tier: InstanceTier,
        primary_provider: VpsProvider,
        primary_region: String,
        standby_provider: VpsProvider,
        standby_region: String,
    ) -> Result<PairProvisionResult> {
        let pair_id = Uuid::new_v4();
        info!(
            %account_id,
            %pair_id,
            ?primary_provider,
            ?standby_provider,
            "Starting pair provision"
        );

        let primary_req = ProvisionRequest {
            request_id: Uuid::new_v4(),
            account_id: account_id.to_string(),
            tier: tier.clone(),
            role: InstanceRole::Primary,
            provider: primary_provider,
            region: primary_region,
            pair_instance_id: None, // Will be set after standby provisions
            openclaw_config: serde_json::json!({}),
            requested_by: gf_node_proto::AgentIdentity::Forge,
            requested_at: Utc::now(),
        };

        // TODO: provision primary, then provision standby with pair_instance_id set
        // TODO: on failure, retry up to 2× with fallback provider
        // TODO: log all steps to gf-audit

        let _ = primary_req;
        Ok(PairProvisionResult {
            pair_id,
            account_id: account_id.to_string(),
            primary: None,
            standby: None,
            success: false,
            error: Some("Not yet implemented".to_string()),
        })
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
