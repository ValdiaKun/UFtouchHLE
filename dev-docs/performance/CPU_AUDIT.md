# CPU / Dynarmic audit (read-only, no behaviour change)

Scope: `src/cpu/dynarmic_wrapper/lib.cpp`, the vendored Dynarmic AArch64 backend
(submodule `vendor/dynarmic` at `e0f6bd9`) and how `src/environment.rs` reacts
to CPU errors. Everything below was established by reading source. **Nothing was
profiled or benchmarked**; costs are described, not measured. Update this file
when a measurement confirms or refutes a point.

## What already exists (do not "add" these)

- **Page-table fast memory.** `DynarmicWrapper` fills a
  `std::array<uint8_t*, NUM_PAGE_TABLE_ENTRIES>` with the guest-memory base
  pointer and sets `absolute_offset_page_table = true`. Dynarmic's inline
  page-table path (`InlinePageTableEmitReadMemory` and the write/exclusive
  equivalents in `backend/arm64/emit_arm64_memory.cpp`) therefore does the
  access inline for every non-null page.
- **Null-page handling.** The first `null_page_count` entries are `nullptr`, so
  those accesses take the fallback path into the Rust memory callbacks, which
  report an error and call `HaltExecution(MemoryAbort)`.
- **Cycle counting** is on (Dynarmic default); `env.ticks_remaining` drives the
  scheduler (`remaining_ticks` in `environment.rs`).
- **`MHP2G_NO_FASTMEM=1`** disables the page table, forcing every access
  through callbacks. Useful only as a pessimistic reference, not a candidate.
- Dynarmic's *true* fastmem (`fastmem_pointer`, host fault handler) is not used
  (see the TODO in `lib.cpp`).

## Corrected finding: `check_halt_on_memory_access`

`lib.cpp` carries a `TODO: only do this in debug builds? it's probably
expensive` on `check_halt_on_memory_access = true`. In the vendored AArch64
backend the check (`EmitA32CheckMemoryAbort`: load-acquire of the halt word,
test, branch) is emitted by `emit_check_memory_abort` **inside the deferred
fallback block**, i.e. only after a memory *callback* returns. It is not on the
inline page-table fast path, so ordinary RAM accesses do not pay for it. The
x64 backend additionally disables `GetSetElimination` when the flag is set, but
the A32 AArch64 path does not.

What it protects: after a faulting callback the block exits at once with the PC
of the faulting instruction. `environment.rs` then routes the error to
`debug_cpu_error` (a fatal, debugger-facing path). Without the check the rest
of the block would keep executing (further loads and stores) before the halt is
noticed, giving an imprecise PC and possible secondary writes after the fault.

Conclusion: there is no evidence this flag matters for speed on Android, and it
keeps faults precise. **Leave it enabled.** Only revisit if a profile shows
significant time in fallback callbacks, and then reduce the *number of
fallbacks*, not the check.

## Candidates, in the order the evidence supports them

None of these is justified until a device baseline exists
(see [BENCHMARKING.md](BENCHMARKING.md)). Each is a separate experiment.

1. **Count fallbacks.** Add opt-in counters for memory-callback invocations per
   frame (and by kind: null page, other). If fallbacks are rare, memory access
   is not the bottleneck and the rest of this list is lower priority. This is
   the cheapest measurement and the one that decides the others.
2. **Page-table footprint.** `NUM_PAGE_TABLE_ENTRIES = 1 << (32 - 12)` entries
   of 8 bytes is **8 MiB**, filled with pointers at startup, so it is resident.
   It also costs a dependent load per inline access and competes for cache/TLB
   on small cores. Measure peak RSS contribution and memory-bound stalls
   (Simpleperf) before deciding anything.
3. **True fastmem** (`fastmem_pointer` plus a host fault handler). Removes the
   table and its load, but must keep guest address-space semantics, the null
   page fault and memory protection, and needs a reserved 4 GiB region with
   guard pages and signal-handler integration on Android. Highest potential and
   highest risk: only after 1 and 2 show memory access matters, and only with
   the regression tests below.
4. **Block cache and invalidation.** `invalidate_cache_range` and
   `swap_context` (per guest-thread switch) were not examined for frequency;
   count calls per frame first.

## Tests required before touching memory handling

These do not exist yet. They must be written and passing on the unchanged
baseline first, so a later change can be shown not to regress them:

- Guest load and store at address 0 and inside the null-page range abort with
  `CpuError::MemoryError`, at the faulting instruction's PC.
- A store *following* a faulting load in the same basic block does **not**
  execute (guards the property `check_halt_on_memory_access` provides).
- Accesses just below and above the null-page boundary behave as normal RAM.
- Existing `tests/integration.rs` suite still passes (needs the macOS build
  environment described in `AGENTS.md`).

## Limits of this audit

- GLES, audio, loading and memory-lifetime paths are not covered yet.
- Source reading only: no profile, no device, and the full crate could not be
  built in the audit environment.
