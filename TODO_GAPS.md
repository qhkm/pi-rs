# Pi-rs Gap List - Remaining Work for 100% Parity

> Generated from gap analysis: ~94% → 100% parity
> Last updated: 2026-03-01

---

## Current Status

**Test count:** 418 (was 357)
**Parity:** ~98%

**Completed:** #13, #16, #12, #4, #8, #10, #9, #1, #2, #5, #7, #11, #3
**Remaining:** 2 items (#14, #15 — terminal image protocols)

---

## ✅ Completed Items

### File Corruption Fix (CRITICAL) — ✅ RESOLVED
The reported corruption in `extensions/mod.rs` was already fixed in a prior commit.
`execute_executor` and `execute_wasm` are properly separated.

### #9: Hook Dispatch in Agent Loop — ✅ DONE
- Moved `HookRegistry`, `HookEvent`, `HookContext`, `HookResult` to `pi-agent-core`
- Added `HookOutcome` enum and `resolve_hook_results()` helper
- Wired 4 dispatch points in `agent_loop.rs`:
  - `BeforeTurn` — before each turn starts; `Cancel` skips the turn
  - `AfterTurn` — after each turn completes
  - `BeforeCompact` — before auto-compaction; `Cancel` skips compaction
  - `AfterCompact` — after successful compaction
- `pi-coding-agent::extensions::hooks` re-exports from `pi-agent-core`
- **+9 tests** in `pi-agent-core`, +4 smoke tests in `pi-coding-agent`

### #1: Merge Test Coverage — ✅ DONE
- `merge_branched_tree_remaps_all_ids` — branch summary ID remapping
- `merge_id_collision_remaps_correctly` — overlapping entry IDs
- `merge_forked_session_preserves_integrity` — fork + merge tree integrity
- **+3 tests**

### #2: Schema Migration Hardening — ✅ DONE
- Header corruption repair (non-JSON headers get synthetic repair)
- v0 detection (entries without `type` field treated as `message`)
- Timestamp preservation (`created_at`, `time`, `date`, `ts` extraction before fallback)
- Malformed entry wrapping (non-JSON lines wrapped in valid entries)
- Unknown field preservation (operates on raw `serde_json::Value`)
- **+9 tests**

### #5: Circular Branch Reference Handling — ✅ DONE
- `append_branch_summary()` validates `branch_entry_id` exists in session
- Ancestor chain walk with `HashSet` detects cycles
- `get_tree()` has Pass 2.5 cycle detection
- **+3 tests**

### #7: @directory Expansion with Globs — ✅ DONE
- `@dirname/` → all files in directory (non-recursive)
- `@dirname/**/*.rs` → glob pattern expansion (recursive)
- `@"path with spaces/"` → quoted directory paths
- Max 100 files per expansion (configurable `MAX_EXPANSION_FILES`)
- **+18 tests**

### #11: WASM Safety Hardening — ✅ DONE
- Stack depth limit: `max_wasm_stack(512 KiB)` in `wasmtime::Config`
- Fuel consumption fix: `consume_fuel(true)` now enabled (was a latent bug)
- Memory limiter: `ResourceLimiter` enforces `max_memory_mb`
- Wall-clock I/O timeout wrapping entire WASM execution
- Output size constant: `MAX_OUTPUT_BYTES = 65536`
- **+8 tests**

### #3: Integration Tests — ✅ DONE
- `branch_merge_export_workflow` — end-to-end branch + merge + HTML export
- `branch_diverge_independent_trees` — branch point with 2 arms
- `large_session_merge_stress` — 1000-entry merge
- `merge_two_forks_into_same_target` — merge 2 forks into 1 target
- **+4 tests** in `tests/integration_workflows.rs`

### #10: Tool Wrapper Hooks — ✅ DONE (prior commit)
`ToolWrapperRegistry` with global and per-tool wrappers, `execute_wrapper_hook()`.

---

## 🔵 LOW PRIORITY (Remaining)

### #14: iTerm2 Inline Image Protocol
**Impact:** 4% | **File:** `crates/pi-tui/src/` (new image rendering module)

**Protocol:** OSC 1337 `File=...` escape sequences
**Reference:** https://iterm2.com/documentation-images.html

**Implementation:**
- Detect iTerm2 terminal via `TERM_PROGRAM` env var
- Convert images to base64
- Emit OSC 1337 sequences inline with text

---

### #15: Kitty Graphics Protocol
**Impact:** 3% | **File:** `crates/pi-tui/src/` (new image rendering module)

**Protocol:** APC `Ga=T,f=...` escape sequences
**Reference:** https://sw.kovidgoyal.net/kitty/graphics-protocol/

**Implementation:**
- Detect Kitty via `TERM` or `KITTY_WINDOW_ID` env var
- Use temporary file or shared memory transmission
- Handle placement and deletion

---

## 🎯 Final Statistics

| Metric | Before | After |
|--------|--------|-------|
| Tests | 357 | 418 |
| New tests added | — | +61 |
| Items completed | 5 | 13 |
| Items remaining | 12 | 2 |
| Parity | ~94% | ~98% |

The remaining 2 items (#14, #15) are terminal image protocol support — large effort,
low urgency, purely additive features that don't affect core functionality.
