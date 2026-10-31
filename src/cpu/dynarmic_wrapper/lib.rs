/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! This is separated out into its own package so that we can avoid rebuilding
//! dynarmic more often than necessary, and to improve build-time parallelism.

// Allow the crate to have a non-snake-case name (touchHLE).
// This also allows items in the crate to have non-snake-case names.
#![allow(non_snake_case)]

/// Opaque type from C
#[allow(non_camel_case_types)]
pub type touchHLE_DynarmicWrapper = std::ffi::c_void;
/// Opaque type from Rust (this is the `Mem` type from the main crate, but
/// `c_void` is used here to avoid depending on it directly)
#[allow(non_camel_case_types)]
pub type touchHLE_Mem = std::ffi::c_void;

#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Debug)]
pub struct touchHLE_DynarmicContext {
    pub regs: [u32; 16],
    pub extregs: [u32; 64],
    pub cpsr: u32,
    pub fpscr: u32,
}

impl Default for touchHLE_DynarmicContext {
    fn default() -> Self {
        Self {
            regs: [0; 16],
            extregs: [0; 64],
            cpsr: 0,
            fpscr: 0,
        }
    }
}

impl touchHLE_DynarmicContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Event counters kept by lib.cpp. They only ever increase, so measure a period
/// by taking two snapshots and calling [touchHLE_CpuCounters::since]. They are
/// bumped only on paths that are already slow (callbacks into Rust, JIT exits,
/// cache flushes), never on dynarmic's inline page-table fast path.
///
/// Layout must match `touchHLE_CpuCounters` in lib.cpp (checked there by a
/// `static_assert` on the size, and here by a unit test).
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct touchHLE_CpuCounters {
    /// `run_or_step` calls.
    pub run_calls: u64,
    /// JIT exits for SVCs, i.e. host function calls and returns to the host.
    pub svc_exits: u64,
    /// Memory read callbacks (null page, or all reads without fast memory).
    pub callback_reads: u64,
    /// Memory write callbacks.
    pub callback_writes: u64,
    /// Exclusive-store callbacks.
    pub exclusive_writes: u64,
    /// Code reads made while dynarmic translates blocks.
    pub code_reads: u64,
    /// Calls to invalidate the JIT's code cache.
    pub cache_invalidations: u64,
    /// Total size of the ranges passed to those calls.
    pub cache_invalidated_bytes: u64,
    /// Guest thread context switches.
    pub context_swaps: u64,
}

impl touchHLE_CpuCounters {
    /// Events that happened between `earlier` and `self`. Saturates instead of
    /// wrapping if the snapshots are given in the wrong order.
    pub fn since(&self, earlier: &Self) -> Self {
        Self {
            run_calls: self.run_calls.saturating_sub(earlier.run_calls),
            svc_exits: self.svc_exits.saturating_sub(earlier.svc_exits),
            callback_reads: self.callback_reads.saturating_sub(earlier.callback_reads),
            callback_writes: self.callback_writes.saturating_sub(earlier.callback_writes),
            exclusive_writes: self
                .exclusive_writes
                .saturating_sub(earlier.exclusive_writes),
            code_reads: self.code_reads.saturating_sub(earlier.code_reads),
            cache_invalidations: self
                .cache_invalidations
                .saturating_sub(earlier.cache_invalidations),
            cache_invalidated_bytes: self
                .cache_invalidated_bytes
                .saturating_sub(earlier.cache_invalidated_bytes),
            context_swaps: self.context_swaps.saturating_sub(earlier.context_swaps),
        }
    }
}

type VAddr = u32;

// Import functions from lib.cpp, see build.rs. Note that lib.cpp depends on
// some functions being exported from Rust, but those are in the main crate.
extern "C" {
    pub fn touchHLE_DynarmicWrapper_new(
        dynamic_memory_access_ptr: *mut std::ffi::c_void,
        null_page_count: usize,
    ) -> *mut touchHLE_DynarmicWrapper;
    pub fn touchHLE_DynarmicWrapper_delete(cpu: *mut touchHLE_DynarmicWrapper);
    pub fn touchHLE_DynarmicWrapper_regs_const(cpu: *const touchHLE_DynarmicWrapper) -> *const u32;
    pub fn touchHLE_DynarmicWrapper_regs_mut(cpu: *mut touchHLE_DynarmicWrapper) -> *mut u32;
    pub fn touchHLE_DynarmicWrapper_cpsr(cpu: *const touchHLE_DynarmicWrapper) -> u32;
    pub fn touchHLE_DynarmicWrapper_set_cpsr(cpu: *mut touchHLE_DynarmicWrapper, cpsr: u32);
    pub fn touchHLE_DynarmicWrapper_counters(
        cpu: *const touchHLE_DynarmicWrapper,
        out: *mut touchHLE_CpuCounters,
    );
    pub fn touchHLE_DynarmicWrapper_swap_context(
        cpu: *mut touchHLE_DynarmicWrapper,
        context: *mut touchHLE_DynarmicContext,
    );
    pub fn touchHLE_DynarmicWrapper_invalidate_cache_range(
        cpu: *mut touchHLE_DynarmicWrapper,
        start: VAddr,
        size: u32,
    );
    pub fn touchHLE_DynarmicWrapper_run_or_step(
        cpu: *mut touchHLE_DynarmicWrapper,
        mem: *mut touchHLE_Mem,
        ticks: Option<&mut u64>,
    ) -> i32;

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_layout_matches_cpp() {
        // lib.cpp: nine uint64_t fields, asserted there with a static_assert.
        assert_eq!(std::mem::size_of::<touchHLE_CpuCounters>(), 9 * 8);
        assert_eq!(std::mem::align_of::<touchHLE_CpuCounters>(), 8);
    }

    #[test]
    fn counters_since_subtracts_and_saturates() {
        let a = touchHLE_CpuCounters {
            svc_exits: 10,
            callback_reads: 7,
            ..Default::default()
        };
        let b = touchHLE_CpuCounters {
            svc_exits: 25,
            callback_reads: 7,
            cache_invalidated_bytes: 4096,
            ..Default::default()
        };
        let d = b.since(&a);
        assert_eq!(d.svc_exits, 15);
        assert_eq!(d.callback_reads, 0);
        assert_eq!(d.cache_invalidated_bytes, 4096);
        // Wrong order must not wrap around.
        assert_eq!(a.since(&b).svc_exits, 0);
    }
}
