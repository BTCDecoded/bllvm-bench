//! Bitcoin Core RPC Client
//!
//! This module provides a Rust wrapper around Bitcoin Core's RPC interface
//! for differential testing.

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// RPC client configuration
#[derive(Debug, Clone)]
pub struct RpcConfig {
    /// RPC URL (e.g., "http://127.0.0.1:18443")
    pub url: String,
    /// RPC username
    pub user: String,
    /// RPC password
    pub pass: String,
    /// Request timeout
    pub timeout: Duration,
}

impl RpcConfig {
    /// Create from regtest node
    pub fn from_regtest_node(node: &crate::regtest_node::RegtestNode) -> Self {
        Self {
            url: node.rpc_url(),
            user: node.rpc_user().to_owned(),
            pass: node.rpc_pass().to_owned(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Create from environment variables (supports remote nodes)
    /// 
    /// Environment variables:
    /// - `BITCOIN_RPC_HOST` (default: "127.0.0.1")
    /// - `BITCOIN_RPC_PORT` (default: 8332 for mainnet, 18443 for regtest)
    /// - `BITCOIN_RPC_USER` (default: "test")
    /// - `BITCOIN_RPC_PASSWORD` (default: "test")
    /// - `BITCOIN_NETWORK` (default: "mainnet") - used to determine default port
    pub fn from_env() -> Self {
        let rpc_host = std::env::var("BITCOIN_RPC_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
        let rpc_user = std::env::var("BITCOIN_RPC_USER")
            .unwrap_or_else(|_| "test".to_string());
        
        let rpc_pass = std::env::var("BITCOIN_RPC_PASSWORD")
            .unwrap_or_else(|_| "test".to_string());
        
        // Determine default port based on network
        let default_port = match std::env::var("BITCOIN_NETWORK")
            .as_ref()
            .map(|s| s.as_str())
        {
            Some("mainnet") | Some("main") => 8332,
            Some("testnet") | Some("test") => 18332,
            Some("regtest") => 18443,
            Some("signet") => 38332,
            _ => 8332, // Default to mainnet
        };
        
        let rpc_port = std::env::var("BITCOIN_RPC_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(default_port);
        
        let url = format!("http://{}:{}", rpc_host, rpc_port);
        
        Self {
            url,
            user: rpc_user,
            pass: rpc_pass,
            timeout: Duration::from_secs(30),
        }
    }

    /// Create with explicit parameters (for programmatic use)
    pub fn new(url: String, user: String, pass: String) -> Self {
        Self {
            url,
            user,
            pass,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Bitcoin Core RPC client
pub struct CoreRpcClient {
    client: Client,
    config: RpcConfig,
}

impl CoreRpcClient {
    /// Create a new RPC client
    pub fn new(config: RpcConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    /// Make an RPC call
    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        let response = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.pass))
            .json(&body)
            .send()
            .await
            .context("RPC request failed")?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("RPC request failed with status: {}", status);
        }

        let json: Value = response
            .json()
            .await
            .context("Failed to parse RPC response")?;

        if let Some(error) = json.get("error") {
            if !error.is_null() {
                anyhow::bail!("RPC error: {}", error);
            }
        }

        json.get("result")
            .cloned()
            .context("RPC response missing result")
    }

    /// Test if a transaction would be accepted to mempool
    pub async fn testmempoolaccept(&self, tx_hex: &str) -> Result<TestMempoolAcceptResult> {
        let params = serde_json::json!([tx_hex]);
        let result = self.call("testmempoolaccept", params).await?;

        // Parse result
        if let Some(results) = result.as_array() {
            if let Some(first) = results.first() {
                let allowed = first
                    .get("allowed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let reject_reason = first
                    .get("reject-reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                return Ok(TestMempoolAcceptResult {
                    allowed,
                    reject_reason,
                });
            }
        }

        anyhow::bail!("Unexpected testmempoolaccept response format")
    }

    /// Submit a block
    pub async fn submitblock(&self, block_hex: &str) -> Result<SubmitBlockResult> {
        let params = serde_json::json!([block_hex]);
        let result = self.call("submitblock", params).await?;

        // submitblock returns null on success, or error string
        if result.is_null() {
            Ok(SubmitBlockResult {
                accepted: true,
                error: None,
            })
        } else if let Some(error) = result.as_str() {
            Ok(SubmitBlockResult {
                accepted: false,
                error: Some(error.to_string()),
            })
        } else {
            anyhow::bail!("Unexpected submitblock response format")
        }
    }

    /// Get block information
    pub async fn getblock(&self, block_hash: &str, verbosity: u8) -> Result<Value> {
        let params = serde_json::json!([block_hash, verbosity]);
        self.call("getblock", params).await
    }

    /// Get block count
    pub async fn getblockcount(&self) -> Result<u64> {
        let result = self.call("getblockcount", serde_json::json!([])).await?;
        result
            .as_u64()
            .or_else(|| result.as_i64().map(|i| i as u64))
            .context("Invalid getblockcount response")
    }

    /// Get block hash by height
    pub async fn getblockhash(&self, height: u64) -> Result<String> {
        let params = serde_json::json!([height]);
        let result = self.call("getblockhash", params).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .context("Invalid getblockhash response")
    }

    /// Get raw block hex (verbosity=0 returns hex string)
    pub async fn getblock_raw(&self, block_hash: &str) -> Result<String> {
        let params = serde_json::json!([block_hash, 0]);
        let result = self.call("getblock", params).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .context("Invalid getblock response (expected hex string with verbosity=0)")
    }

    /// Generate blocks (regtest only)
    pub async fn generatetoaddress(&self, nblocks: u64, address: &str) -> Result<Vec<String>> {
        let params = serde_json::json!([nblocks, address]);
        let result = self.call("generatetoaddress", params).await?;
        if let Some(blocks) = result.as_array() {
            Ok(blocks
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect())
        } else {
            anyhow::bail!("Unexpected generatetoaddress response format")
        }
    }

    /// Get new address
    pub async fn getnewaddress(&self) -> Result<String> {
        let result = self.call("getnewaddress", serde_json::json!([])).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .context("Invalid getnewaddress response")
    }

    /// Get blockchain info (includes network/chain type)
    pub async fn getblockchaininfo(&self) -> Result<serde_json::Value> {
        self.call("getblockchaininfo", serde_json::json!([])).await
    }

    /// Detect network type from running node
    pub async fn detect_network(&self) -> Result<BitcoinNetwork> {
        let info = self.getblockchaininfo().await?;
        let chain = info
            .get("chain")
            .and_then(|c| c.as_str())
            .context("Missing 'chain' field in getblockchaininfo")?;

        match chain {
            "main" => Ok(BitcoinNetwork::Mainnet),
            "test" | "testnet" => Ok(BitcoinNetwork::Testnet),
            "regtest" => Ok(BitcoinNetwork::Regtest),
            "signet" => Ok(BitcoinNetwork::Signet),
            _ => anyhow::bail!("Unknown network type: {}", chain),
        }
    }

    /// Get pruning information from node
    /// Returns (is_pruned, prune_height) where prune_height is the minimum available block height
    pub async fn get_pruning_info(&self) -> Result<(bool, Option<u64>)> {
        let info = self.getblockchaininfo().await?;
        
        let is_pruned = info
            .get("pruned")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);
        
        let prune_height = if is_pruned {
            info.get("pruneheight")
                .and_then(|h| h.as_u64())
                .or_else(|| {
                    info.get("pruneheight")
                        .and_then(|h| h.as_i64())
                        .map(|i| i as u64)
                })
        } else {
            None
        };
        
        Ok((is_pruned, prune_height))
    }

    /// Test if this RPC connection is working
    pub async fn test_connection(&self) -> Result<bool> {
        match self.getblockcount().await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

/// Auto-discover Bitcoin Core nodes
pub struct NodeDiscovery;

impl NodeDiscovery {
    /// Discover nodes by reading Bitcoin Core config files
    pub fn discover_from_config_files() -> Vec<RpcConfig> {
        let mut configs = Vec::new();
        
        // Common Bitcoin Core config file locations
        let config_paths = vec![
            // Mainnet
            dirs::home_dir().map(|h| h.join(".bitcoin/bitcoin.conf")),
            PathBuf::from("/etc/bitcoin/bitcoin.conf"),
            // Testnet
            dirs::home_dir().map(|h| h.join(".bitcoin/testnet3/bitcoin.conf")),
            // Regtest
            dirs::home_dir().map(|h| h.join(".bitcoin/regtest/bitcoin.conf")),
        ];

        for path_opt in config_paths {
            if let Some(path) = path_opt {
                if let Ok(config) = Self::parse_bitcoin_conf(&path) {
                    configs.push(config);
                }
            }
        }

        configs
    }

    /// Parse Bitcoin Core config file
    fn parse_bitcoin_conf(path: &PathBuf) -> Result<RpcConfig> {
        use std::fs;
        
        let content = fs::read_to_string(path)
            .context(format!("Failed to read config file: {}", path.display()))?;
        
        let mut rpc_port: Option<u16> = None;
        let mut rpc_user: Option<String> = None;
        let mut rpc_password: Option<String> = None;
        let mut rpc_bind: Option<String> = None;
        let mut testnet = false;
        let mut regtest = false;

        for line in content.lines() {
            // Remove comments
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }

            // Parse key=value
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_lowercase();
                let value = value.trim();

                match key.as_str() {
                    "rpcport" => {
                        if let Ok(port) = value.parse::<u16>() {
                            rpc_port = Some(port);
                        }
                    }
                    "rpcuser" => {
                        rpc_user = Some(value.to_string());
                    }
                    "rpcpassword" => {
                        rpc_password = Some(value.to_string());
                    }
                    "rpcbind" => {
                        rpc_bind = Some(value.to_string());
                    }
                    "testnet" => {
                        testnet = value == "1" || value == "true";
                    }
                    "regtest" => {
                        regtest = value == "1" || value == "true";
                    }
                    _ => {}
                }
            }
        }

        // Determine default port if not set
        let port = rpc_port.unwrap_or_else(|| {
            if regtest {
                18443
            } else if testnet {
                18332
            } else {
                8332
            }
        });

        // Default credentials if not set
        let user = rpc_user.unwrap_or_else(|| "test".to_string());
        let password = rpc_password.unwrap_or_else(|| "test".to_string());
        let host = rpc_bind.unwrap_or_else(|| "127.0.0.1".to_string());

        Ok(RpcConfig {
            url: format!("http://{}:{}", host, port),
            user,
            pass: password,
            timeout: Duration::from_secs(5),
        })
    }

    /// Discover nodes by trying common configurations
    pub async fn discover_common_configs() -> Vec<RpcConfig> {
        let mut configs = Vec::new();

        // Common ports
        let ports = vec![8332, 18332, 18443, 38332]; // mainnet, testnet, regtest, signet
        
        // Common credential combinations
        let credentials = vec![
            ("test", "test"),
            ("rpcuser", "rpcpassword"),
            ("bitcoin", "bitcoin"),
            ("", ""), // Some nodes allow no auth
        ];

        // Try localhost first
        for port in &ports {
            for (user, pass) in &credentials {
                let config = RpcConfig {
                    url: format!("http://127.0.0.1:{}", port),
                    user: user.to_string(),
                    pass: pass.to_string(),
                    timeout: Duration::from_secs(2),
                };
                let client = CoreRpcClient::new(config.clone());
                if client.test_connection().await.unwrap_or(false) {
                    configs.push(config);
                }
            }
        }

        configs
    }

    /// Auto-discover and return a random working node
    /// Tries multiple methods:
    /// 1. Environment variables
    /// 2. Config files
    /// 3. Common local configurations
    pub async fn auto_discover() -> Result<RpcConfig> {
        let mut candidates = Vec::new();

        // 1. Check environment variables first (highest priority)
        if std::env::var("BITCOIN_RPC_HOST").is_ok() {
            let config = RpcConfig::from_env();
            let client = CoreRpcClient::new(config.clone());
            if client.test_connection().await.unwrap_or(false) {
                return Ok(config);
            }
        }

        // 2. Try config files
        candidates.extend(Self::discover_from_config_files());

        // 3. Try common configurations
        candidates.extend(Self::discover_common_configs().await);

        // Test all candidates and filter working ones
        let mut working = Vec::new();
        for config in candidates {
            let client = CoreRpcClient::new(config.clone());
            if client.test_connection().await.unwrap_or(false) {
                working.push(config);
            }
        }

        if working.is_empty() {
            anyhow::bail!("No Bitcoin Core nodes found. Set BITCOIN_RPC_HOST or ensure a node is running.");
        }

        // Return random node from working candidates
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..working.len());
        Ok(working.remove(idx))
    }
}

/// Bitcoin network types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Regtest,
    Signet,
}

impl BitcoinNetwork {
    pub fn default_rpc_port(&self) -> u16 {
        match self {
            BitcoinNetwork::Mainnet => 8332,
            BitcoinNetwork::Testnet => 18332,
            BitcoinNetwork::Regtest => 18443,
            BitcoinNetwork::Signet => 38332,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BitcoinNetwork::Mainnet => "mainnet",
            BitcoinNetwork::Testnet => "testnet",
            BitcoinNetwork::Regtest => "regtest",
            BitcoinNetwork::Signet => "signet",
        }
    }
}

/// Result of testmempoolaccept
#[derive(Debug, Clone)]
pub struct TestMempoolAcceptResult {
    /// Whether transaction is allowed
    pub allowed: bool,
    /// Reject reason if not allowed
    pub reject_reason: Option<String>,
}

/// Result of submitblock
#[derive(Debug, Clone)]
pub struct SubmitBlockResult {
    /// Whether block was accepted
    pub accepted: bool,
    /// Error message if not accepted
    pub error: Option<String>,
}
