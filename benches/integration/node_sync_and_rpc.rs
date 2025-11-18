//! Integration benchmark: Sync 1000 blocks and run bitcoin-cli commands
//!
//! This benchmark:
//! 1. Starts a bllvm node in regtest mode
//! 2. Generates/syncs the first 1000 blocks
//! 3. Runs basic bitcoin-cli compatible RPC commands
//! 4. Measures performance

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use bllvm_consensus::{tx_inputs, tx_outputs};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tokio::time::timeout;
const TARGET_BLOCKS: u64 = 1000;
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const BLOCK_GENERATION_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes for 1000 blocks
/// RPC client for making bitcoin-cli compatible calls
struct RpcClient {
    url: String,
    client: reqwest::Client,
}
impl RpcClient {
    fn new(port: u16) -> Self {
        Self {
            url: format!("http://127.0.0.1:{}", port),
            client: reqwest::Client::new(),
        }
    }
    async fn call(&self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });
        let response = timeout(RPC_TIMEOUT, self.client
            .post(&self.url)
            .json(&request)
            .send())
            .await??;
        let json: serde_json::Value = response.json().await?;
        
        if let Some(error) = json.get("error") {
            anyhow::bail!("RPC error: {}", error);
        }
        Ok(json["result"].clone())
    }
    
    async fn wait_for_node(&self) -> anyhow::Result<()> {
        // Wait for node to be ready by polling getblockchaininfo
        for _ in 0..60 {
            if self.call("getblockchaininfo", serde_json::json!([])).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("Node did not become ready in time");
    }
    
    async fn get_block_count(&self) -> anyhow::Result<u64> {
        let result = self.call("getblockcount", serde_json::json!([])).await?;
        Ok(result.as_u64().unwrap_or(0))
    }
}

/// Setup and start a node for benchmarking
async fn setup_node(data_dir: PathBuf, rpc_port: u16) -> anyhow::Result<(Arc<bllvm_node::node::Node>, RpcClient)> {
    use bllvm_node::node::Node;
    use bllvm_protocol::ProtocolVersion;
    let network_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
    let rpc_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), rpc_port);
    // Create node in regtest mode
    let node = Node::new(
        data_dir.to_str().unwrap(),
        network_addr,
        rpc_addr,
        Some(ProtocolVersion::Regtest),
    )?;
    // Start RPC server in background
    // The node's RPC manager is already configured with dependencies
    // We need to start it, but since node.rpc() returns &RpcManager and start() needs &mut,
    // we'll create a new RPC manager with the same configuration
    // For benchmarking, we'll use the node's data directory to recreate storage
    let storage_arc = Arc::new(bllvm_node::storage::Storage::new(data_dir.to_str().unwrap())?);
    let mempool_arc = Arc::new(bllvm_node::node::mempool::MempoolManager::new());
    // Network manager doesn't implement Clone, so we'll create a minimal one
    let network_arc = Arc::new(bllvm_node::network::NetworkManager::new(network_addr));
    
    let mut rpc_mgr = bllvm_node::rpc::RpcManager::new(rpc_addr)
        .with_dependencies(storage_arc, mempool_arc)
        .with_network_manager(network_arc);
    tokio::spawn(async move {
        if let Err(e) = rpc_mgr.start().await {
            eprintln!("RPC server error: {}", e);
        }
    });
    // Give server a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;
    let rpc_client = RpcClient::new(rpc_port);
    // Wait for node to be ready
    rpc_client.wait_for_node().await?;
    Ok((Arc::new(node), rpc_client))
}

/// Generate blocks using storage and consensus directly
/// This is a simplified approach for benchmarking
async fn generate_blocks_via_storage(
    storage: &bllvm_node::storage::Storage,
    _protocol: &bllvm_protocol::BitcoinProtocolEngine,
    target: u64,
) -> anyhow::Result<()> {
    use bllvm_protocol::Block;
    use bllvm_protocol::types::BlockHeader;
    // Get current height
    let current_height = storage.chain().get_height()?.unwrap_or(0);
    for height in current_height..(current_height + target) {
        // Get tip for prev_block_hash
        let (prev_block_hash, bits) = if let Some(tip_header) = storage.chain().get_tip_header()? {
            let tip_hash = storage.chain().get_tip_hash()?.unwrap_or([0u8; 32]);
            (tip_hash, tip_header.bits)
        } else {
            // Genesis block
            ([0u8; 32], 0x207fffff) // Regtest difficulty
        };
        // Create a simple coinbase transaction
        let coinbase_tx = bllvm_protocol::Transaction {
            version: 1,
            inputs: bllvm_protocol::tx_inputs![],
            outputs: bllvm_protocol::tx_outputs![bllvm_protocol::TransactionOutput {
                value: 5000000000, // 50 BTC
                script_pubkey: vec![0x76, 0xa9, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0xac],
            }],
            lock_time: 0,
        };
        // Calculate merkle root
        let mut txs = vec![coinbase_tx];
        use bllvm_protocol::mining::calculate_merkle_root;
        let merkle_root = calculate_merkle_root(&mut txs)
            .map_err(|e| anyhow::anyhow!("Failed to calculate merkle root: {}", e))?;
        // Create block header
        let header = BlockHeader {
            version: 1,
            prev_block_hash,
            merkle_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            bits,
            nonce: height, // Simple nonce for regtest
        };
        // Create block
        let block = Block {
            header: header.clone(),
            transactions: txs.into_boxed_slice(),
        };
        // Store block and get hash from blockstore
        storage.blocks().store_block(&block)?;
        let block_hash = storage.blocks().get_block_hash(&block);
        storage.blocks().store_height(height + 1, &block_hash)?;
        storage.blocks().store_recent_header(height + 1, &header)?;
        // Update chain state
        storage.chain().update_tip(&block_hash, &header, height + 1)?;
        if (height + 1) % 100 == 0 {
            eprintln!("Generated {} blocks", height + 1);
        }
    }
    Ok(())
}

/// Run basic RPC commands
async fn run_basic_rpc_commands(rpc_client: &RpcClient) -> anyhow::Result<()> {
    // Test getblockchaininfo
    let info = rpc_client.call("getblockchaininfo", serde_json::json!([])).await?;
    black_box(info);
    // Test getblockcount
    let count = rpc_client.get_block_count().await?;
    black_box(count);
    // Test getblockhash for block 0 (genesis)
    let genesis_hash = rpc_client.call("getblockhash", serde_json::json!([0])).await?;
    let genesis_hash_clone = genesis_hash.clone();
    black_box(genesis_hash);
    // Test getblock for genesis block
    if let Some(hash_str) = genesis_hash_clone.as_str() {
        let block = rpc_client.call("getblock", serde_json::json!([hash_str, 0])).await?;
        black_box(block);
        // Test getblockheader
        let header = rpc_client.call("getblockheader", serde_json::json!([hash_str, false])).await?;
        black_box(header);
    }
    // Test getnetworkinfo
    let network_info = rpc_client.call("getnetworkinfo", serde_json::json!([])).await?;
    black_box(network_info);
    // Test getmininginfo
    let mining_info = rpc_client.call("getmininginfo", serde_json::json!([])).await?;
    black_box(mining_info);
    Ok(())
}

fn benchmark_node_sync_and_rpc(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.bench_function("sync_1000_blocks_and_rpc", |b| {
        b.iter(|| {
            rt.block_on(async {
            // Create temporary directory for node data
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let rpc_port = 18443; // Standard regtest RPC port
            // Setup node
            let (node, rpc_client) = setup_node(data_dir.clone(), rpc_port)
                .await
                .expect("Failed to setup node");
            // Generate blocks using storage directly
            let start = std::time::Instant::now();
            
            let storage = node.storage();
            let protocol = node.protocol();
            if let Err(e) = timeout(BLOCK_GENERATION_TIMEOUT, generate_blocks_via_storage(storage, protocol, TARGET_BLOCKS)).await {
                eprintln!("Warning: Block generation failed or timed out: {:?}", e);
            }
            // Wait for blocks to be processed and verify
            let mut attempts = 0;
            while attempts < 200 {
                if let Ok(count) = rpc_client.get_block_count().await {
                    if count >= TARGET_BLOCKS {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                attempts += 1;
            }
            let sync_time = start.elapsed();
            let final_count = rpc_client.get_block_count().await.unwrap_or(0);
            let sync_time_ms = sync_time.as_millis() as f64;
            eprintln!("Synced {} blocks in {:.2} ms", final_count, sync_time_ms);
            // Run basic RPC commands
            let rpc_start = std::time::Instant::now();
            run_basic_rpc_commands(&rpc_client)
                .await
                .expect("Failed to run RPC commands");
            let rpc_time = rpc_start.elapsed();
            let rpc_time_ms = rpc_time.as_millis() as f64;
            eprintln!("RPC commands completed in {:.2} ms", rpc_time_ms);
            // Cleanup
            drop(node);
            drop(temp_dir);
            black_box((sync_time, rpc_time))
            })
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10) // Fewer samples for integration test
        .measurement_time(Duration::from_secs(60))
        .warm_up_time(Duration::from_secs(5));
    targets = benchmark_node_sync_and_rpc
}
criterion_main!(benches);
