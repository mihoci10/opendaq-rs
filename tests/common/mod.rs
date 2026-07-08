//! Shared helpers for the integration tests.

#![allow(dead_code)]

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Tests that create openDAQ instances (loading device modules, spinning up
/// schedulers, ...) hold this lock so they run one at a time within a test
/// binary; pure coretypes/coreobjects tests don't need it.
pub fn instance_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A fresh instance over the reference device simulator.
pub fn make_test_instance() -> opendaq::Instance {
    opendaq::Instance::new().expect("failed to create an openDAQ instance")
}

/// The reference channel of a freshly added `daqref://device0`.
pub fn make_ref_channel(instance: &opendaq::Instance) -> opendaq::Channel {
    instance
        .add_device("daqref://device0")
        .expect("add_device failed")
        .expect("no device");
    instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")
        .expect("find_component failed")
        .expect("reference channel not found")
        .cast::<opendaq::Channel>()
        .expect("cast to Channel failed")
}
