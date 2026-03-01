//! clawnode — ClawOps VPS Node Agent
//!
//! Connects to the OpenClaw gateway, registers as a VPS node, and handles
//! fleet management commands: health, metrics, docker, config, secrets.

use clap::{Parser, Subcommand};
mod hetzner_cmd;
use clawnode::{GatewayClient, config::NodeConfig, create_state};
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[derive(Parser)]
#[command(name = "clawnode")]
#[command(about = "ClawOps VPS Node Agent")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the node agent (connect to gateway and serve commands)
    Run {
        /// Path to config file
        #[arg(short, long, default_value = "/etc/clawnode/config.json")]
        config: PathBuf,
    },

    /// Join a gateway using a bootstrap token (saves config and connects once)
    Join {
        /// Gateway WebSocket URL
        #[arg(long)]
        gateway: String,

        /// Bootstrap token or auth token
        #[arg(long)]
        token: String,

        /// Path to save config
        #[arg(long, default_value = "/etc/clawnode/config.json")]
        config: PathBuf,
    },

    /// Show system information for this VPS
    Info,

    /// Generate a sample config file
    InitConfig {
        /// Path to write config
        #[arg(short, long, default_value = "/etc/clawnode/config.json")]
        output: PathBuf,

        /// Gateway URL
        #[arg(long, default_value = "wss://localhost:18789")]
        gateway: String,
    },

    /// Execute an internal command (for use via system.run or testing)
    ///
    /// Examples:
    ///   clawnode exec vps.status
    ///   clawnode exec health.check
    ///   clawnode exec vps.metrics
    ///   clawnode exec node.capabilities
    Exec {
        /// Command name (e.g. vps.status, health.check, vps.metrics)
        command: String,

        /// JSON parameters for the command (default: {})
        #[arg(long, default_value = "{}")]
        params: String,
    },

    /// Hetzner Cloud API commands
    #[command(subcommand)]
    Hetzner(HetznerCommands),
}

#[derive(Subcommand)]
enum HetznerCommands {
    /// List all servers
    List,
    /// Get server details
    Get {
        /// Server ID or name
        server: String,
    },
    /// Server metrics (last hour)
    Metrics {
        /// Server ID or name
        server: String,
    },
    /// Create a new server
    Create {
        /// Server name
        #[arg(long)]
        name: String,
        /// Server type (e.g. cx22, cpx32)
        #[arg(long, default_value = "cpx32")]
        server_type: String,
        /// Location (nbg1, hel1, fsn1, ash)
        #[arg(long, default_value = "hel1")]
        location: String,
        /// OS image
        #[arg(long, default_value = "ubuntu-24.04")]
        image: String,
    },
    /// Delete a server (⚠️ destructive)
    Delete {
        /// Server ID or name
        server: String,
    },
    /// Reboot a server
    Reboot {
        /// Server ID or name
        server: String,
    },
    /// Power on a server
    Poweron {
        /// Server ID or name
        server: String,
    },
    /// Power off a server
    Poweroff {
        /// Server ID or name
        server: String,
    },
    /// Resize a server
    Resize {
        /// Server ID or name
        server: String,
        /// New server type (e.g. cx22, cpx32, cpx42)
        #[arg(long)]
        server_type: String,
    },
    /// List available server types
    Types,
    /// List SSH keys
    SshKeys,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Suppress tracing for exec commands to keep stdout clean JSON
    if !matches!(cli.command, Commands::Exec { .. } | Commands::Hetzner(_)) {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::from_default_env().add_directive("clawnode=info".parse()?))
            .init();
    }

    match cli.command {
        Commands::Run { config } => {
            run_agent(config).await?;
        }
        Commands::Join {
            gateway,
            token,
            config,
        } => {
            join_gateway(gateway, token, config).await?;
        }
        Commands::Info => {
            system_info()?;
        }
        Commands::InitConfig { output, gateway } => {
            init_config(output, gateway)?;
        }
        Commands::Exec { command, params } => {
            exec_command(&command, &params).await?;
        }
        Commands::Hetzner(cmd) => {
            handle_hetzner(cmd).await?;
        }
    }

    Ok(())
}

// ─── Hetzner ──────────────────────────────────────────────────────────────────

async fn handle_hetzner(cmd: HetznerCommands) -> anyhow::Result<()> {
    let client = hetzner_cmd::HetznerClient::from_config()?;

    let result = match cmd {
        HetznerCommands::List => client.list_servers().await?,
        HetznerCommands::Get { server } => {
            let s = client.get_server(&server).await?;
            serde_json::json!({ "ok": true, "server": s })
        }
        HetznerCommands::Metrics { server } => client.server_metrics(&server).await?,
        HetznerCommands::Create { name, server_type, location, image } => {
            client.create_server(&name, &server_type, &location, &image, &[], &std::collections::HashMap::new()).await?
        }
        HetznerCommands::Delete { server } => client.delete_server(&server).await?,
        HetznerCommands::Reboot { server } => client.reboot_server(&server).await?,
        HetznerCommands::Poweron { server } => client.power_action(&server, "poweron").await?,
        HetznerCommands::Poweroff { server } => client.power_action(&server, "poweroff").await?,
        HetznerCommands::Resize { server, server_type } => client.resize_server(&server, &server_type).await?,
        HetznerCommands::Types => client.list_server_types().await?,
        HetznerCommands::SshKeys => client.list_ssh_keys().await?,
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

// ─── Run ─────────────────────────────────────────────────────────────────────

async fn run_agent(config_path: PathBuf) -> anyhow::Result<()> {
    info!(config = %config_path.display(), "starting clawnode");

    let config = NodeConfig::load(&config_path)?;
    info!(
        gateway = %config.gateway,
        hostname = %config.hostname,
        provider = %config.provider,
        region = %config.region,
        tier = %config.tier,
        role = %config.role,
        "loaded config"
    );

    let state = create_state(config.clone());

    info!(
        caps = ?state.capabilities,
        commands = ?state.commands,
        "node capabilities"
    );

    let identity_path = config.state_path.join("device.json");
    let mut client = GatewayClient::new(state, identity_path);
    let token = config.token.as_deref();

    loop {
        if let Err(e) = client.connect(&config.gateway, token).await {
            error!(error = %e, "connection error");
        }

        info!(
            delay = config.reconnect_delay_secs,
            "reconnecting in {} seconds", config.reconnect_delay_secs
        );
        tokio::time::sleep(std::time::Duration::from_secs(config.reconnect_delay_secs)).await;
    }
}

// ─── Join ─────────────────────────────────────────────────────────────────────

async fn join_gateway(gateway: String, token: String, config_path: PathBuf) -> anyhow::Result<()> {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    info!(gateway = %gateway, hostname = %hostname, "joining gateway");

    let state_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".clawnode");

    std::fs::create_dir_all(&state_path)?;

    let config = NodeConfig {
        gateway: gateway.clone(),
        token: Some(token.clone()),
        hostname: hostname.clone(),
        state_path: state_path.clone(),
        ..NodeConfig::default()
    };

    // Save config
    config.save(&config_path)?;
    info!(path = %config_path.display(), "config saved");

    let state = create_state(config);
    let identity_path = state_path.join("device.json");

    info!(
        caps = ?state.capabilities,
        commands = ?state.commands,
        "node capabilities"
    );

    let mut client = GatewayClient::new(state, identity_path);

    match client.connect(&gateway, Some(&token)).await {
        Ok(_) => {
            info!("joined gateway successfully");
            Ok(())
        }
        Err(e) => {
            error!(error = %e, "failed to join gateway");
            anyhow::bail!("{}", e)
        }
    }
}

// ─── Info ─────────────────────────────────────────────────────────────────────

fn system_info() -> anyhow::Result<()> {
    use sysinfo::System;

    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("System Information:");
    println!();
    println!("  Hostname:  {}", hostname);
    println!(
        "  OS:        {} {}",
        System::name().unwrap_or_default(),
        System::os_version().unwrap_or_default()
    );
    println!(
        "  Kernel:    {}",
        System::kernel_version().unwrap_or_default()
    );
    println!();
    println!("  CPUs:      {}", sys.cpus().len());
    println!(
        "  Memory:    {} / {} MB",
        sys.used_memory() / 1024 / 1024,
        sys.total_memory() / 1024 / 1024
    );
    println!("  Uptime:    {} seconds", System::uptime());
    println!();
    println!("  Agent:     clawnode v{}", env!("CARGO_PKG_VERSION"));

    Ok(())
}

// ─── InitConfig ───────────────────────────────────────────────────────────────

fn init_config(output: PathBuf, gateway: String) -> anyhow::Result<()> {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "clawnode".to_string());

    let config = NodeConfig {
        gateway,
        hostname,
        state_path: PathBuf::from("/var/lib/clawnode"),
        ..NodeConfig::default()
    };

    config.save(&output)?;

    println!("Config written to {}", output.display());
    println!();
    println!("Edit the file to add your token, then run:");
    println!("  clawnode run --config {}", output.display());

    Ok(())
}

// ─── Exec ─────────────────────────────────────────────────────────────────────

async fn exec_command(command: &str, params_str: &str) -> anyhow::Result<()> {
    use clawnode::commands::{CommandRequest, handle_command};

    let params: serde_json::Value = serde_json::from_str(params_str)
        .map_err(|e| anyhow::anyhow!("invalid JSON params: {e}"))?;

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let config = NodeConfig {
        hostname: hostname.clone(),
        state_path: dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".clawnode"),
        ..NodeConfig::default()
    };

    let state = create_state(config);

    let request = CommandRequest {
        command: command.to_string(),
        params,
    };

    match handle_command(&state, request).await {
        Ok(result) => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Err(e) => {
            let err = serde_json::json!({
                "ok": false,
                "error": e.to_string(),
                "command": command,
            });
            println!("{}", serde_json::to_string_pretty(&err)?);
            std::process::exit(1);
        }
    }

    Ok(())
}
