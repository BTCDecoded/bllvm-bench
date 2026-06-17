//! Sort-Merge Differential Validation
//!
//! Gold-standard differential testing using external sort + merge-join.
//! All operations are memory-bounded via streaming I/O.
//!
//! ## Strategy
//!
//! Transform random prevout lookups into sequential I/O:
//!
//! 1. **Extract Inputs**: Record (prevout_txid, prevout_idx, block, tx, input) for each input
//! 2. **Sort by Prevout**: External sort by (prevout_txid, prevout_idx)
//! 3. **Extract Outputs**: Record (txid, output_idx, value, scriptPubKey) for spent outputs
//! 4. **Merge Join**: Match inputs with outputs to get prevout data
//! 5. **Sort by Location**: External sort by (block, tx, input)
//! 6. **Verify Scripts**: Stream blocks + prevouts in lockstep, verify scripts in parallel
//!
//! ## Memory Usage
//!
//! All steps are streaming - peak memory ~1-2GB for buffers.
//! Intermediate files total ~25GB on disk.

pub mod input_refs;
pub mod merge_join;
pub mod output_refs;
pub mod verify;

pub use input_refs::extract_input_refs;
pub use merge_join::merge_join;
pub use output_refs::extract_outputs;
pub use verify::verify_scripts;
