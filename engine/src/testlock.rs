//! Serializes tests that mutate the process-global `ARVOLO_CONFIG_DIR`, which
//! several stores read; without this they race under the parallel test runner.

use std::sync::Mutex;
pub static ENV: Mutex<()> = Mutex::new(());
