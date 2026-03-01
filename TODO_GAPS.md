# Pi-rs Gap List - Remaining Work for 100% Parity

> Generated from gap analysis: ~94% → 100% parity  
> Last updated: 2026-03-01

---

## Current Status

**Completed:** #13, #16, #12, #4, #8, #10, #9, #1, #2, #7, #11  
**In Progress:** None  
**Remaining:** 6 items

---

## 🟡 HIGH PRIORITY (Medium Effort, High Impact)

### #9: Wire Hook Dispatch into Agent Loop
**Status:** ✅ COMPLETED in commit 8b68539

---

### #10: Tool Wrapper Execution Wiring  
**Status:** ✅ COMPLETED in commit 8b68539

- Added `wrapper_registry` field to `RuntimeExtensionTool`
- Modified `execute()` to call `execute_wrapper_hook()` for before/after hooks
- Added `WrapperHookType` enum for before/after distinction
- Path traversal protection already in place

---

### #1: Merge Test Coverage
**Status:** ✅ COMPLETED in commit f6a29b9

Added tests for:
- Branched trees with multiple branches
- ID collisions with UUID-like IDs
- Forked session preservation

---

## 🟡 REMAINING HIGH PRIORITY
**Impact:** 5% | **File:** `crates/pi-agent-core/src/agent/agent_loop.rs`

**What:**
- Call `hook_registry.dispatch(BeforeTurn)` before each agent iteration
- Call `hook_registry.dispatch(AfterTurn)` after each turn completes
- Call `hook_registry.dispatch(BeforeCompact)` before context compaction
- Call `hook_registry.dispatch(AfterCompact)` after compaction

**Where in agent_loop.rs:**
- Find the main agent loop (likely around line 80-150)
- Add hook calls at appropriate lifecycle points
- Handle `HookResult::Cancel` to abort operations
- Handle `HookResult::Modified(data)` to use modified data

**Example pattern:**
```rust
// Before turn
let ctx = HookContext { event: BeforeTurn, data: json!({"turn": turn_count }) };
let results = hook_registry.dispatch(&ctx).await;
// Check for Cancel or Modified results

// After turn
let ctx = HookContext { event: AfterTurn, data: json!({"turn": turn_count, "result": ... }) };
hook_registry.dispatch(&ctx).await;
```

---

### #1: Merge Test Coverage
**Impact:** 5% | **File:** `crates/pi-coding-agent/src/session/manager.rs` tests

**Add tests for:**
1. **Branched trees:** Merge session with multiple branches, verify IDs remap correctly
2. **ID collisions:** Merge when both sessions have overlapping entry IDs
3. **Concurrent merges:** Test merging same source to multiple targets simultaneously
4. **Forked-session merging:** Create fork, add entries to both, merge them

**Test pattern:**
```rust
#[tokio::test]
async fn merge_branched_tree_remaps_all_ids() {
    // Create source with branches
    // Create target
    // Merge
    // Verify no duplicate IDs in result
    // Verify parent chain integrity
}
```

---

### #2: Harden Schema Migrations
**Status:** ✅ COMPLETED in commit e1d3bf9

**Implemented:**
1. ✅ Header corruption repair with `parse_header_with_repair()` and `repair_header()`
2. ✅ Timestamp preservation via `extract_timestamp_from_entry()` - only falls back to `Utc::now()`
3. ✅ v0 → v1/v2/v3 migration path with proper type inference
4. ✅ ID collision handling with automatic remapping
5. ✅ Malformed entry marking with `_malformed` flag
6. ✅ Parent ID remapping during collision resolution

**New tests (+5):**
- `migrate_session_handles_header_corruption`
- `migrate_session_preserves_existing_timestamps`
- `migrate_session_handles_v0_entries`
- `migrate_session_handles_id_collisions`
- `migrate_session_marks_malformed_entries`

---

### #7: @directory Expansion with Globs
**Status:** ✅ COMPLETED in commit b48976b

**Implemented:**
- ✅ `@dirname/` → includes all files in directory (non-recursive)
- ✅ `@dirname/**/*.rs` → glob pattern expansion with `**` support
- ✅ `@"path with spaces/"` → quoted directory paths with spaces

**Implementation:**
- Added `process_directory()` for directory expansion using glob
- Added `process_glob()` for pattern matching (resolves relative to cwd)
- Added `process_single_file()` for shared file processing logic
- Modified `process_file_ref()` to detect trailing `/` or glob patterns

**New tests (+7):**
- `test_directory_expansion`
- `test_glob_pattern_expansion`
- `test_recursive_glob_expansion`
- `test_quoted_directory_path`
- `test_directory_not_found_kept_as_text`
- `test_glob_no_matches_kept_as_text`
- `test_directory_with_images`

---

## 🟢 MEDIUM PRIORITY

### #11: WASM Safety Hardening
**Status:** ✅ COMPLETED in commit 11d5636

**Implemented:**
1. ✅ Instruction count limits via fuel metering (1 fuel ≈ 1 instruction)
2. ✅ Memory bounds checking at configurable offset (default 1024)
3. ✅ I/O timeout enforcement via epoch interruption + tokio timeout
4. ✅ Stack depth limits via max_wasm_stack configuration

**Additional safety features:**
- Configurable max_io_ops (default 10,000)
- Configurable max_output_bytes (default 64KB)
- Input size limit (1MB max)
- Pointer alignment validation (4-byte aligned)
- Null pointer validation
- Memory bounds validation for all allocations

**New config fields:**
- `max_stack_depth`: Maximum WASM stack frames
- `max_io_ops`: Maximum I/O operations per execution
- `max_output_bytes`: Maximum output size in bytes
- `memory_alloc_offset`: Minimum allocation offset

**New tests (+4):**
- `test_wasm_config_custom`
- `test_wasm_memory_bounds_config`
- `test_wasm_safety_limits`
- `test_wasm_input_size_limit`

---

### #3: Integration Tests
**Impact:** 5% | **Location:** `crates/pi-coding-agent/tests/`

**Add end-to-end tests:**
1. **branch→merge→export workflow:**
   - Create session, branch, add entries to both branches
   - Merge branches back together
   - Export to HTML, verify content

2. **Concurrent fork+merge:**
   - Fork session in two different ways simultaneously
   - Merge both forks back to original
   - Verify tree integrity

3. **Large session stress test:**
   - Create session with 10K+ entries
   - Measure merge performance
   - Verify no stack overflow

---

### #5: Circular Branch Reference Handling
**Impact:** 3% | **File:** `crates/pi-coding-agent/src/session/manager.rs`

**Problem:** `branch_entry_id` in BranchSummary could create cycles

**Fix:**
- In `append_branch_summary()`, validate branch_entry_id doesn't create cycle
- In `get_tree()`, detect cycles and break them
- Add cycle detection to validation

---

## 🔵 LOW PRIORITY (Large Effort)

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

## 📋 Quick Reference: Files to Modify

| File | Tasks |
|------|-------|
| `extensions/mod.rs` | **CRITICAL:** Fix corruption, complete #10 |
| `agent/agent_loop.rs` | #9: Hook dispatch |
| `session/manager.rs` | #1, #2, #5: Merge tests, migrations, cycles |
| `input/file_processor.rs` | #7: Directory/glob expansion |
| `extensions/wasm.rs` | #11: WASM safety |
| `tests/integration.rs` | #3: Integration tests |
| `pi-tui/src/image.rs` (new) | #14, #15: Terminal images |

---

## 🎯 Success Criteria

After completing items #9, #10, #1, #2, #7:
- Test count: 339 → 370+
- Coverage: ~94% → ~98%

After completing all items:
- Test count: 370 → 400+
- Coverage: ~98% → 100%
- PARITY.md updated with all features verified

---

## 🚨 Blockers

1. **File corruption in extensions/mod.rs** - Must fix before #10
2. Missing test infrastructure for integration tests
3. No existing image rendering in pi-tui (needs new module)

---

## 💡 Tips for Next Agent

1. **Start with file corruption fix** - use git to see original state
2. **For #9:** Look for existing hook calls in agent_loop.rs as reference
3. **For #1:** Use existing merge tests as template, add edge cases
4. **For #2:** Add debug logging to migration to see what's happening
5. **For #7:** Consider using `glob` crate from crates.io
6. Run `cargo test --workspace` after each change
7. Update PARITY.md as items are completed
