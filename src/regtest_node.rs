//! Regtest Node Manager
//!
//! This module manages Bitcoin Core regtest nodes for differential testing.
//! It handles starting, stopping, and managing multiple concurrent nodes.

use anyhow::{Context, Result};
use std::path::{PathBuf, Path};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use crate::core_builder::CoreBinaries;

/// Port manager for allocating unique ports
#[derive(Debug, Clone)]
pub struct PortManager {
    base_port: u16,
    used_ports: Arc<tokio::sync::Mutex<std::collections::HashSet<u16>>>,
}

impl PortManager {
    /// Create a new port manager with base port
    pub fn new(base_port: u16) -> Self {
        Self {
            base_port,
            used_ports: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// Allocate a unique port
    pub async fn allocate_port(&self) -> u16 {
        let mut used = self.used_ports.lock().await;
        let mut port = self.base_port;
        while used.contains(&port) || !self.is_port_available(port) {
            port += 1;
            if port > self.base_port + 100 {
                // Fallback: use random port
                port = self.base_port + 100 + (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() % 100) as u16;
            }
        }
        used.insert(port);
        port
    }

    /// Release a port
    pub async fn release_port(&self, port: u16) {
        let mut used = self.used_ports.lock().await;
        used.remove(&port);
    }

    /// Check if port is available
    fn is_port_available(&self, port: u16) -> bool {
        use std::net::TcpListener;
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
}

/// Regtest node configuration
#[derive(Debug, Clone)]
pub struct RegtestNodeConfig {
    /// RPC port
    pub rpc_port: u16,
    /// Data directory
    pub data_dir: PathBuf,
    /// RPC username
    pub rpc_user: String,
    /// RPC password
    pub rpc_pass: String,
    /// RPC host
    pub rpc_host: String,
}

impl Default for RegtestNodeConfig {
    fn default() -> Self {
        Self {
            rpc_port: 18443,
            data_dir: std::env::temp_dir().join(format!("bitcoin-regtest-{}", std::process::id())),
            rpc_user: "test".to_string(),
            rpc_pass: "test".to_string(),
            rpc_host: "127.0.0.1".to_string(),
        }
    }
}

/// A running regtest node
pub struct RegtestNode {
    /// Node configuration
    config: RegtestNodeConfig,
    /// Core binaries
    binaries: CoreBinaries,
    /// Child process
    child: Option<Child>,
    /// Port manager (for cleanup)
    port_manager: Option<Arc<PortManager>>,
}

impl RegtestNode {
    /// Start a new regtest node
    pub async fn start(
        binaries: CoreBinaries,
        config: RegtestNodeConfig,
        port_manager: Option<Arc<PortManager>>,
    ) -> Result<Self> {
        // Verify binaries
        binaries.verify().context("Core binaries verification failed")?;

        // Create data directory
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("Failed to create data directory: {}", config.data_dir.display()))?;

        // Check if node is already running on this port
        if Self::is_rpc_ready(&config).await {
            anyhow::bail!(
                "Bitcoin Core is already running on port {}",
                config.rpc_port
            );
        }

        // Kill any existing bitcoind on this port
        Self::kill_existing_node(&config).await;

        // Start bitcoind
        let mut cmd = Command::new(&binaries.bitcoind);
        cmd.args(&[
            "-regtest",
            "-daemon",
            "-server",
            &format!("-datadir={}", config.data_dir.display()),
            &format!("-rpcuser={}", config.rpc_user),
            &format!("-rpcpassword={}", config.rpc_pass),
            &format!("-rpcport={}", config.rpc_port.to_string()),
            &format!("-rpcallowip={}", config.rpc_host),
            &format!("-rpcbind={}", config.rpc_host),
            "-fallbackfee=0.00001",
            "-txindex=0",
        ]);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        cmd.spawn()
            .context("Failed to start bitcoind")?;

        // Wait for RPC to be ready
        Self::wait_for_rpc_ready(&config, Duration::from_secs(60))
            .await
            .context("Bitcoin Core failed to start or RPC not ready")?;

        Ok(Self {
            config,
            binaries,
            child: None, // Daemon mode, so no child process
            port_manager,
        })
    }

    /// Start with default config and port from manager
    pub async fn start_with_port_manager(
        binaries: CoreBinaries,
        port_manager: Arc<PortManager>,
    ) -> Result<Self> {
        let port = port_manager.allocate_port().await;
        let mut config = RegtestNodeConfig::default();
        config.rpc_port = port;

        // Use tmpfs if available (faster I/O)
        if Path::new("/dev/shm").exists() {
            config.data_dir = PathBuf::from(format!(
                "/dev/shm/bitcoin-regtest-{}",
                std::process::id()
            ));
        } else {
            config.data_dir = std::env::temp_dir().join(format!(
                "bitcoin-regtest-{}",
                std::process::id()
            ));
        }

        Self::start(binaries, config, Some(port_manager)).await
    }

    /// Check if RPC is ready
    async fn is_rpc_ready(config: &RegtestNodeConfig) -> bool {
        let client = reqwest::Client::new();
        let url = format!("http://{}:{}", config.rpc_host, config.rpc_port);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "getblockcount",
            "params": [],
            "id": 1
        });

        let response = client
            .post(&url)
            .basic_auth(&config.rpc_user, Some(&config.rpc_pass))
            .json(&body)
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        match response {
            Ok(resp) => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    json.get("result").and_then(|r| r.as_i64()).is_some()
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    /// Wait for RPC to be ready
    async fn wait_for_rpc_ready(config: &RegtestNodeConfig, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if Self::is_rpc_ready(config).await {
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("RPC not ready after {:?}", timeout)
    }

    /// Kill any existing node on this port
    async fn kill_existing_node(config: &RegtestNodeConfig) {
        // Try to find and kill bitcoind processes
        #[cfg(unix)]
        {
            let _ = Command::new("pkill")
                .args(&["-f", &format!("bitcoind.*regtest.*{}", config.rpc_port)])
                .output();
        }
        #[cfg(windows)]
        {
            // Windows implementation would use taskkill
            let _ = Command::new("taskkill")
                .args(&["/F", "/IM", "bitcoind.exe"])
                .output();
        }
        sleep(Duration::from_secs(2)).await;
    }

    /// Get RPC URL
    pub fn rpc_url(&self) -> String {
        format!("http://{}:{}", self.config.rpc_host, self.config.rpc_port)
    }

    /// Get RPC user
    pub fn rpc_user(&self) -> &str {
        &self.config.rpc_user
    }

    /// Get RPC password
    pub fn rpc_pass(&self) -> &str {
        &self.config.rpc_pass
    }

    /// Get RPC port
    pub fn rpc_port(&self) -> u16 {
        self.config.rpc_port
    }
}

impl Drop for RegtestNode {
    fn drop(&mut self) {
        // Stop bitcoind
        #[cfg(unix)]
        {
            let _ = Command::new(&self.binaries.bitcoin_cli)
                .args(&[
                    "-regtest",
                    &format!("-rpcuser={}", self.config.rpc_user),
                    &format!("-rpcpassword={}", self.config.rpc_pass),
                    &format!("-rpcport={}", self.config.rpc_port),
                    "stop",
                ])
                .output();
        }

        // Release port
        if let Some(ref pm) = self.port_manager {
            let port = self.config.rpc_port;
            let pm = pm.clone();
            tokio::spawn(async move {
                pm.release_port(port).await;
            });
        }

        // Clean up data directory (optional, can be kept for debugging)
        if let Ok(env_var) = std::env::var("KEEP_REGTEST_DATA") {
            if env_var == "1" || env_var.to_lowercase() == "true" {
                return;
            }
        }

        // Remove data directory
        let _ = std::fs::remove_dir_all(&self.config.data_dir);
    }
}

