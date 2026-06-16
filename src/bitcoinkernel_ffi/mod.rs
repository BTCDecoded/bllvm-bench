//! Unsafe FFI to Bitcoin Core `libbitcoinkernel` (C API in `bitcoinkernel.h`).
//!
//! Pin the Core revision used to build the `.so`; this module matches the tree at
//! build time via manual declarations.
//!
//! **Parallel note:** [`KernelSession::process_serialized_block`](KernelSession::process_serialized_block)
//! extends the **active** tip. Comparing arbitrary historical windows in parallel either needs one
//! chainstate manager per process (separate datadirs, as in `scripts/kernel-diff-parallel-lanes.sh`)
//! or a **future** fork API (e.g. validate a detached block against an imported UTXO view without
//! rewinding global tip). BLVM can resume from deltas cheaply; the kernel side is what still fans out.

use anyhow::{Context, Result};
use libc::{c_char, c_int, c_void, size_t};
use std::ffi::CString;
use std::path::Path;
use std::sync::Mutex;

pub const BTCK_CHAIN_TYPE_MAINNET: u8 = 0;

pub const BTCK_VALIDATION_MODE_VALID: u8 = 0;
pub const BTCK_VALIDATION_MODE_INVALID: u8 = 1;
pub const BTCK_VALIDATION_MODE_INTERNAL_ERROR: u8 = 2;

pub const BTCK_BLOCK_VALIDATION_RESULT_UNSET: u32 = 0;
pub const BTCK_BLOCK_VALIDATION_RESULT_CONSENSUS: u32 = 1;
pub const BTCK_BLOCK_VALIDATION_RESULT_CACHED_INVALID: u32 = 2;
pub const BTCK_BLOCK_VALIDATION_RESULT_INVALID_HEADER: u32 = 3;
pub const BTCK_BLOCK_VALIDATION_RESULT_MUTATED: u32 = 4;
pub const BTCK_BLOCK_VALIDATION_RESULT_MISSING_PREV: u32 = 5;
pub const BTCK_BLOCK_VALIDATION_RESULT_INVALID_PREV: u32 = 6;
pub const BTCK_BLOCK_VALIDATION_RESULT_TIME_FUTURE: u32 = 7;
pub const BTCK_BLOCK_VALIDATION_RESULT_HEADER_LOW_WORK: u32 = 8;

#[repr(C)]
pub struct ValidationInterfaceCallbacks {
    pub user_data: *mut c_void,
    pub user_data_destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub block_checked: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
    pub pow_valid_block: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
    pub block_connected: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
    pub block_disconnected: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
}

#[repr(C)]
struct CheckedPayload {
    mode: u8,
    result: u32,
}

pub struct BlockCheckedSlot {
    inner: Mutex<Option<CheckedPayload>>,
}

impl BlockCheckedSlot {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn clear(&self) {
        *self.inner.lock().expect("block_checked mutex") = None;
    }

    fn take_payload(&self) -> Option<CheckedPayload> {
        self.inner.lock().expect("block_checked mutex").take()
    }
}

unsafe extern "C" fn on_block_checked(
    user_data: *mut c_void,
    _block: *mut c_void,
    state: *const c_void,
) {
    if user_data.is_null() || state.is_null() {
        return;
    }
    let slot = &*(user_data as *const BlockCheckedSlot);
    let mode = btck_block_validation_state_get_validation_mode(state);
    let result = btck_block_validation_state_get_block_validation_result(state);
    *slot.inner.lock().expect("block_checked mutex") = Some(CheckedPayload { mode, result });
}

// `build.rs` emits `cargo:rustc-link-lib` for the *library* target only; binaries in this crate
// must still pull `libbitcoinkernel` at final link. See rustc book: native library links on rlibs.
#[link(name = "bitcoinkernel", kind = "static")]
unsafe extern "C" {
    fn btck_chain_parameters_create(chain_type: u8) -> *mut c_void;
    fn btck_chain_parameters_destroy(chain_parameters: *mut c_void);

    fn btck_context_options_create() -> *mut c_void;
    fn btck_context_options_set_chainparams(
        context_options: *mut c_void,
        chain_parameters: *const c_void,
    );
    fn btck_context_options_set_validation_interface(
        context_options: *mut c_void,
        validation_interface_callbacks: ValidationInterfaceCallbacks,
    );
    fn btck_context_options_destroy(context_options: *mut c_void);

    fn btck_context_create(context_options: *const c_void) -> *mut c_void;
    fn btck_context_destroy(context: *mut c_void);

    fn btck_chainstate_manager_options_create(
        context: *const c_void,
        data_directory: *const c_char,
        data_directory_len: size_t,
        blocks_directory: *const c_char,
        blocks_directory_len: size_t,
    ) -> *mut c_void;
    fn btck_chainstate_manager_options_set_worker_threads_num(
        chainstate_manager_options: *mut c_void,
        worker_threads: c_int,
    );
    fn btck_chainstate_manager_options_set_wipe_dbs(
        chainstate_manager_options: *mut c_void,
        wipe_block_tree_db: c_int,
        wipe_chainstate_db: c_int,
    ) -> c_int;
    fn btck_chainstate_manager_options_set_defer_activate_best_chains(
        chainstate_manager_options: *mut c_void,
        defer: c_int,
    );
    fn btck_chainstate_manager_options_set_coins_cache_bytes(
        chainstate_manager_options: *mut c_void,
        cache_bytes: usize,
    );
    fn btck_chainstate_manager_options_set_skip_scripts(chainstate_manager_options: *mut c_void);
    fn btck_chainstate_manager_options_update_block_tree_db_in_memory(
        chainstate_manager_options: *mut c_void,
        block_tree_db_in_memory: c_int,
    );
    fn btck_chainstate_manager_options_update_chainstate_db_in_memory(
        chainstate_manager_options: *mut c_void,
        chainstate_db_in_memory: c_int,
    );
    fn btck_chainstate_manager_options_destroy(chainstate_manager_options: *mut c_void);

    fn btck_chainstate_manager_create(chainstate_manager_options: *const c_void) -> *mut c_void;
    fn btck_chainstate_manager_destroy(chainstate_manager: *mut c_void);

    fn btck_chainstate_manager_import_blvm_utxo_snapshot_fixed_v1(
        chainstate_manager: *mut c_void,
        path: *const c_char,
        path_len: size_t,
    ) -> c_int;
    fn btck_chainstate_manager_seed_headless(
        chainstate_manager: *mut c_void,
        path: *const c_char,
        path_len: size_t,
        block_headers: *const u8,
        n_headers: size_t,
    ) -> c_int;
    fn btck_chainstate_manager_seed_headless_restore(
        chainstate_manager: *mut c_void,
        block_headers: *const u8,
        n_headers: size_t,
    ) -> c_int;

    fn btck_chainstate_manager_process_block(
        chainstate_manager: *mut c_void,
        block: *const c_void,
        new_block: *mut c_int,
    ) -> c_int;

    fn btck_chainstate_manager_get_active_chain(chainstate_manager: *const c_void)
        -> *const c_void;
    fn btck_chain_get_height(chain: *const c_void) -> c_int;

    fn btck_block_create(raw_block: *const c_void, raw_block_len: size_t) -> *mut c_void;
    fn btck_block_destroy(block: *mut c_void);

    fn btck_block_validation_state_get_validation_mode(state: *const c_void) -> u8;
    fn btck_block_validation_state_get_block_validation_result(state: *const c_void) -> u32;
}

pub fn block_validation_result_label(code: u32) -> &'static str {
    match code {
        BTCK_BLOCK_VALIDATION_RESULT_UNSET => "UNSET",
        BTCK_BLOCK_VALIDATION_RESULT_CONSENSUS => "CONSENSUS",
        BTCK_BLOCK_VALIDATION_RESULT_CACHED_INVALID => "CACHED_INVALID",
        BTCK_BLOCK_VALIDATION_RESULT_INVALID_HEADER => "INVALID_HEADER",
        BTCK_BLOCK_VALIDATION_RESULT_MUTATED => "MUTATED",
        BTCK_BLOCK_VALIDATION_RESULT_MISSING_PREV => "MISSING_PREV",
        BTCK_BLOCK_VALIDATION_RESULT_INVALID_PREV => "INVALID_PREV",
        BTCK_BLOCK_VALIDATION_RESULT_TIME_FUTURE => "TIME_FUTURE",
        BTCK_BLOCK_VALIDATION_RESULT_HEADER_LOW_WORK => "HEADER_LOW_WORK",
        _ => "UNKNOWN",
    }
}

/// Outcome from [`KernelSession::process_serialized_block`].
#[derive(Debug, Clone)]
pub enum CoreBlockOutcome {
    /// `block_checked` reported valid.
    Valid,
    /// `block_checked` reported invalid or internal error (with detail).
    Invalid {
        mode: u8,
        result_code: u32,
        detail: String,
    },
    /// `process_block` returned success, `new_block == 0`, and no `block_checked` (typical duplicate).
    DuplicateNoCallback,
    /// `btck_chainstate_manager_process_block` returned non-zero.
    ProcessFailed { rc: i32 },
    /// Failed to parse wire bytes into a `btck_Block`.
    BadBlockWire,
}

impl CoreBlockOutcome {
    pub fn is_valid(&self) -> bool {
        matches!(
            self,
            CoreBlockOutcome::Valid | CoreBlockOutcome::DuplicateNoCallback
        )
    }

    pub fn summary(&self) -> String {
        match self {
            CoreBlockOutcome::Valid => "valid".to_string(),
            CoreBlockOutcome::Invalid {
                mode,
                result_code,
                detail,
            } => format!("invalid(mode={mode},result={result_code},{detail})"),
            CoreBlockOutcome::DuplicateNoCallback => "valid_duplicate".to_string(),
            CoreBlockOutcome::ProcessFailed { rc } => format!(
                "process_failed(rc={rc}); Core prints `BLVM CheckBlock/AcceptBlock FAIL` / `ActivateBestChain FAIL` to stderr (e.g. bad-diffbits, prev-blk-not-found)"
            ),
            CoreBlockOutcome::BadBlockWire => "bad_block_wire".to_string(),
        }
    }
}

/// Owns kernel context + chainstate manager + validation callback slot.
pub struct KernelSession {
    checked_slot: Box<BlockCheckedSlot>,
    context: *mut c_void,
    chainman: *mut c_void,
}

unsafe impl Send for KernelSession {}

/// Tuning for [`KernelSession::open_with_options`].
#[derive(Debug, Clone)]
pub struct KernelSessionOptions {
    /// Parallel validation worker threads (clamped 0–15 per libbitcoinkernel).
    pub worker_threads: i32,
    /// Keep block index DB in RAM (faster, **much** more RAM).
    pub block_tree_db_in_memory: bool,
    /// Keep chainstate (UTXO) DB in RAM (faster, **enormous** RAM at high height).
    pub chainstate_db_in_memory: bool,
    /// Wipe the coins/chainstate DB when opening.
    ///
    /// When used **without** [`Self::wipe_block_tree_db`], keeps the LevelDB block tree / header
    /// index on disk (needed for classic snapshot import where the tip height must match the index).
    ///
    /// Headless [`KernelSession::seed_headless`] rebuilds `CBlockIndex` from provided headers; a
    /// **stale** on-disk block tree can leave wrong heights on `InsertBlockIndex` collisions and
    /// corrupt retargets (`bad-diffbits` at 2016 boundaries). For that path, wipe the block tree too.
    pub wipe_chainstate_db: bool,
    /// Wipe the block tree / block index DB when opening (blkindex / headers LevelDB under the datadir).
    pub wipe_block_tree_db: bool,
    /// Skip `ActivateBestChains` during session open (UTXO stays empty until `seed_headless`).
    pub defer_activate_best_chains: bool,
    /// In-memory coins cache size in MiB. `None` = use `DEFAULT_KERNEL_CACHE` (450 MiB).
    /// For differential-testing tools, 50–100 MiB is plenty at early chain heights and
    /// cuts ~400 MiB off Core's RSS without hurting BPS significantly.
    pub coins_cache_mb: Option<u64>,
    /// Skip all script/signature verification in ConnectBlock.
    /// Use when historical script validity is established and only UTXO/consensus logic is under test.
    pub skip_scripts: bool,
}

impl Default for KernelSessionOptions {
    fn default() -> Self {
        Self {
            worker_threads: 8,
            block_tree_db_in_memory: false,
            chainstate_db_in_memory: false,
            wipe_chainstate_db: false,
            wipe_block_tree_db: false,
            defer_activate_best_chains: false,
            coins_cache_mb: None,
            skip_scripts: false,
        }
    }
}

impl KernelSession {
    /// `data_dir` is the Core datadir root (chainstate + indexes); `blocks_dir` is usually `data_dir/blocks`.
    pub fn open(data_dir: &Path, blocks_dir: &Path) -> Result<Self> {
        Self::open_with_options(data_dir, blocks_dir, KernelSessionOptions::default())
    }

    /// Same as [`KernelSession::open`] with explicit libbitcoinkernel options.
    pub fn open_with_options(
        data_dir: &Path,
        blocks_dir: &Path,
        opts: KernelSessionOptions,
    ) -> Result<Self> {
        let checked_slot = Box::new(BlockCheckedSlot::new());
        let slot_ptr = std::ptr::from_ref(&*checked_slot) as *mut c_void;

        let chain_params = unsafe { btck_chain_parameters_create(BTCK_CHAIN_TYPE_MAINNET) };
        if chain_params.is_null() {
            anyhow::bail!("btck_chain_parameters_create returned null");
        }

        let ctx_opts = unsafe { btck_context_options_create() };
        if ctx_opts.is_null() {
            unsafe { btck_chain_parameters_destroy(chain_params) };
            anyhow::bail!("btck_context_options_create returned null");
        }

        unsafe {
            btck_context_options_set_chainparams(ctx_opts, chain_params);
            let vi = ValidationInterfaceCallbacks {
                user_data: slot_ptr,
                user_data_destroy: None,
                block_checked: Some(on_block_checked),
                pow_valid_block: None,
                block_connected: None,
                block_disconnected: None,
            };
            btck_context_options_set_validation_interface(ctx_opts, vi);
        }

        let context = unsafe { btck_context_create(ctx_opts) };
        unsafe {
            btck_context_options_destroy(ctx_opts);
            btck_chain_parameters_destroy(chain_params);
        }
        if context.is_null() {
            anyhow::bail!("btck_context_create returned null");
        }

        let data_c = path_to_cstring(data_dir)?;
        let blocks_c = path_to_cstring(blocks_dir)?;

        let csm_opts = unsafe {
            btck_chainstate_manager_options_create(
                context,
                data_c.as_ptr(),
                data_c.as_bytes().len(),
                blocks_c.as_ptr(),
                blocks_c.as_bytes().len(),
            )
        };
        if csm_opts.is_null() {
            unsafe { btck_context_destroy(context) };
            anyhow::bail!("btck_chainstate_manager_options_create returned null");
        }

        unsafe {
            let wt = opts.worker_threads.clamp(0, 15);
            btck_chainstate_manager_options_set_worker_threads_num(csm_opts, wt);
            if opts.wipe_chainstate_db || opts.wipe_block_tree_db {
                let wipe_bt = if opts.wipe_block_tree_db { 1 } else { 0 };
                let wipe_cs = if opts.wipe_chainstate_db { 1 } else { 0 };
                let rc = btck_chainstate_manager_options_set_wipe_dbs(csm_opts, wipe_bt, wipe_cs);
                if rc != 0 {
                    btck_chainstate_manager_options_destroy(csm_opts);
                    btck_context_destroy(context);
                    anyhow::bail!(
                        "btck_chainstate_manager_options_set_wipe_dbs(block_tree={wipe_bt}, chainstate={wipe_cs}) failed (rc={rc})"
                    );
                }
            }
            if opts.block_tree_db_in_memory {
                btck_chainstate_manager_options_update_block_tree_db_in_memory(csm_opts, 1);
            }
            if opts.chainstate_db_in_memory {
                btck_chainstate_manager_options_update_chainstate_db_in_memory(csm_opts, 1);
            }
            if opts.defer_activate_best_chains {
                btck_chainstate_manager_options_set_defer_activate_best_chains(csm_opts, 1);
            }
            if let Some(mb) = opts.coins_cache_mb {
                btck_chainstate_manager_options_set_coins_cache_bytes(
                    csm_opts,
                    (mb as usize) * 1024 * 1024,
                );
            }
            if opts.skip_scripts {
                btck_chainstate_manager_options_set_skip_scripts(csm_opts);
            }
        }

        let chainman = unsafe { btck_chainstate_manager_create(csm_opts) };
        unsafe { btck_chainstate_manager_options_destroy(csm_opts) };
        if chainman.is_null() {
            unsafe { btck_context_destroy(context) };
            anyhow::bail!(
                "btck_chainstate_manager_create returned null (check CORE_DIFF_DATADIR is writable and chainstate can load)"
            );
        }

        Ok(KernelSession {
            checked_slot,
            context,
            chainman,
        })
    }

    /// Load a **fixed-v1** BLVM UTXO snapshot (`BLVMUX01`) into Core chainstate via
    /// `btck_chainstate_manager_import_blvm_utxo_snapshot_fixed_v1`.
    ///
    /// **Preconditions:** coins DB must be empty (e.g. wiped chainstate) while the block index tip
    /// height matches the snapshot height in the file. See Core fork `LoadBlvmUtxoSnapshotFixedV1`.
    pub fn import_blvm_utxo_snapshot_fixed_v1(&self, path: &Path) -> Result<()> {
        let c = path_to_cstring(path)?;
        let rc = unsafe {
            btck_chainstate_manager_import_blvm_utxo_snapshot_fixed_v1(
                self.chainman,
                c.as_ptr(),
                c.as_bytes().len(),
            )
        };
        if rc != 0 {
            anyhow::bail!(
                "btck_chainstate_manager_import_blvm_utxo_snapshot_fixed_v1 failed (rc={rc}); see Core logs"
            );
        }
        Ok(())
    }

    /// Seed a **headless** Core chainstate from a fixed-v1 snapshot and raw block headers —
    /// no pre-built LevelDB block index required.
    ///
    /// Pass the last ≤11 raw 80-byte block headers ending at height H in ascending order.
    /// Providing fewer than 11 is allowed but will give incorrect `GetMedianTimePast()` for
    /// the first blocks in the comparison window (affects nLockTime / CSV validation).
    ///
    /// After this returns successfully, call [`process_serialized_block`] for blocks H+1 onward.
    ///
    /// **Preconditions:** coins DB must be empty (wipe with `wipe_chainstate_db: true` option).
    pub fn seed_headless(&self, snapshot_path: &Path, raw_headers: &[[u8; 80]]) -> Result<()> {
        let c = path_to_cstring(snapshot_path)?;
        // Lay the headers out as a flat byte slice: n × 80 bytes.
        let flat: Vec<u8> = raw_headers.iter().flat_map(|h| h.iter().copied()).collect();
        let rc = unsafe {
            btck_chainstate_manager_seed_headless(
                self.chainman,
                c.as_ptr(),
                c.as_bytes().len(),
                flat.as_ptr(),
                raw_headers.len(),
            )
        };
        if rc != 0 {
            anyhow::bail!("btck_chainstate_manager_seed_headless failed (rc={rc}); see Core logs");
        }
        Ok(())
    }

    /// Restore a headless chainstate after process restart **without** re-loading the UTXO snapshot.
    ///
    /// This is a lightweight alternative to [`seed_headless`] for use when the coins DB is already
    /// populated from a prior run (so we can skip the expensive 57M-UTXO import). It rebuilds only
    /// the dummy pprev stub chain and patches the dangling pointer on the first real stub, then
    /// restores `m_chain.Tip()` to the actual current best block from the coins DB.
    ///
    /// Eliminates the ~10–50 GiB glibc heap fragmentation caused by cycling `CCoinsViewCache`
    /// through multiple `Flush()` calls during a full seed restart.
    ///
    /// **Preconditions:**
    /// - The `KernelSession` was opened with `defer_activate_best_chains: true`.
    /// - Coins DB is already populated (non-null `GetBestBlock()`).
    /// - Block index has been loaded from LevelDB (at least one `process_block` ran before shutdown).
    /// - `raw_headers` is the same header window originally passed to [`seed_headless`].
    pub fn seed_headless_restore(&self, raw_headers: &[[u8; 80]]) -> Result<()> {
        let flat: Vec<u8> = raw_headers.iter().flat_map(|h| h.iter().copied()).collect();
        let rc = unsafe {
            btck_chainstate_manager_seed_headless_restore(
                self.chainman,
                flat.as_ptr(),
                raw_headers.len(),
            )
        };
        if rc != 0 {
            anyhow::bail!(
                "btck_chainstate_manager_seed_headless_restore failed (rc={rc}); see Core logs"
            );
        }
        Ok(())
    }

    /// Active chain tip height, or `-1` if the chain is empty (Core convention).
    pub fn tip_height(&self) -> i32 {
        unsafe {
            let chain = btck_chainstate_manager_get_active_chain(self.chainman);
            if chain.is_null() {
                return -1;
            }
            btck_chain_get_height(chain)
        }
    }

    pub fn process_serialized_block(&self, raw: &[u8]) -> CoreBlockOutcome {
        self.checked_slot.clear();

        let block = unsafe { btck_block_create(raw.as_ptr() as *const c_void, raw.len()) };
        if block.is_null() {
            return CoreBlockOutcome::BadBlockWire;
        }

        let mut new_block: c_int = 0;
        let rc = unsafe {
            btck_chainstate_manager_process_block(
                self.chainman,
                block as *const c_void,
                &mut new_block,
            )
        };
        unsafe { btck_block_destroy(block) };

        if rc != 0 {
            return CoreBlockOutcome::ProcessFailed { rc };
        }

        if let Some(p) = self.checked_slot.take_payload() {
            return match p.mode {
                BTCK_VALIDATION_MODE_VALID => CoreBlockOutcome::Valid,
                BTCK_VALIDATION_MODE_INVALID => CoreBlockOutcome::Invalid {
                    mode: p.mode,
                    result_code: p.result,
                    detail: block_validation_result_label(p.result).to_string(),
                },
                BTCK_VALIDATION_MODE_INTERNAL_ERROR | _ => CoreBlockOutcome::Invalid {
                    mode: p.mode,
                    result_code: p.result,
                    detail: "INTERNAL_ERROR".to_string(),
                },
            };
        }

        if new_block == 0 {
            CoreBlockOutcome::DuplicateNoCallback
        } else {
            CoreBlockOutcome::Invalid {
                mode: BTCK_VALIDATION_MODE_INTERNAL_ERROR,
                result_code: BTCK_BLOCK_VALIDATION_RESULT_UNSET,
                detail: "no_block_checked_callback".to_string(),
            }
        }
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        unsafe {
            if !self.chainman.is_null() {
                btck_chainstate_manager_destroy(self.chainman);
            }
            if !self.context.is_null() {
                btck_context_destroy(self.context);
            }
        }
    }
}

fn path_to_cstring(p: &Path) -> Result<CString> {
    let s = p.to_str().context("datadir path must be valid UTF-8")?;
    CString::new(s).context("datadir path contains NUL")
}
