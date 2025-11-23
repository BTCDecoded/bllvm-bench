//! Bitcoin Core RPC Client
//!
//! This module provides a Rust wrapper around Bitcoin Core's RPC interface
//! for differential testing.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
            user: node.rpc_user().to_string(),
            pass: node.rpc_pass().to_string(),
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

