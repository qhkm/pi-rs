# Pi-rs Gap List - Remaining Work for 100% Parity

> Generated from gap analysis: ~94% → 100% parity  
> Last updated: 2026-03-01

---

## Current Status

**Completed:** #13, #16, #12, #4, #8, #10, #9, #1, #2, #7, #11, #3, #5  
**In Progress:** None  
**Remaining:** 4 items (all low priority)

---

## ✅ COMPLETED

### #4: HTML Export Tests
**Status:** ✅ Already complete - end-to-end tests exist in `export/html.rs`

### #8: Quoted @mention Parsing
**Status:** ✅ COMPLETED - `@"path with spaces/file.txt"` syntax supported

### #12: Wrapper Hook Path Validation  
**Status:** ✅ COMPLETED - Path traversal protection with `canonicalize()` checks

### #13: wrap_line() UTF-8 Bug
**Status:** ✅ COMPLETED - Replaced byte slicing with character-based iteration

### #16: yank_pop() TODO
**Status:** ✅ COMPLETED - Proper kill-ring rotation with `last_yank` tracking

### #9: Wire Hook Dispatch into Agent Loop
**Status:** ✅ COMPLETED in commit 8b68539

- Added `HookRegistry`, `HookEvent`, `HookContext`, `HookResult` types
- Dispatch at 4 lifecycle points: `BeforeTurn`, `AfterTurn`, `BeforeCompact`, `AfterCompact`
- Added `register_hook()` method to Agent
- Handles `Cancel` and `Modified` results appropriately

### #10: Tool Wrapper Execution Wiring  
**Status:** ✅ COMPLETED in commit 8b68539

- Added `wrapper_registry` field to `RuntimeExtensionTool`
- Modified `execute()` to call `execute_wrapper_hook()` for before/after hooks
- Added `WrapperHookType` enum for before/after distinction
- Path traversal protection already in place

### #1: Merge Test Coverage
**Status:** ✅ COMPLETED in commit f6a29b9

Added tests for:
- Branched trees with multiple branches
- ID collisions with UUID-like IDs
- Forked session preservation

### #2: Harden Schema Migrations
**Status:** ✅ COMPLETED in commit e1d3bf9

Implemented:
1. Header corruption repair with `parse_header_with_repair()` and `repair_header()`
2. Timestamp preservation via `extract_timestamp_from_entry()`
3. v0 → v1/v2/v3 migration path with proper type inference
4. ID collision handling with automatic remapping
5. Malformed entry marking with `_malformed` flag
6. Parent ID remapping during collision resolution

**New tests (+5):**
- `migrate_session_handles_header_corruption`
- `migrate_session_preserves_existing_timestamps`
- `migrate_session_handles_v0_entries`
- `migrate_session_handles_id_collisions`
- `migrate_session_marks_malformed_entries`

### #7: @directory Expansion with Globs
**Status:** ✅ COMPLETED in commit b48976b

Implemented:
- `@dirname/` → includes all files in directory (non-recursive)
- `@dirname/**/*.rs` → glob pattern expansion with `**` support
- `@"path with spaces/"` → quoted directory paths with spaces

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

### #11: WASM Safety Hardening
**Status:** ✅ COMPLETED in commit 11d5636

Implemented:
1. Instruction count limits via fuel metering (1 fuel ≈ 1 instruction)
2. Memory bounds checking at configurable offset (default 1024)
3. I/O timeout enforcement via epoch interruption + tokio timeout
4. Stack depth limits via max_wasm_stack configuration

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

### #3: Integration Tests
**Status:** ✅ COMPLETED in commit bc78ba5

Added tests in `crates/pi-coding-agent/tests/session_workflow.rs`:
- `branch_merge_export_workflow`: Tests branching with multiple branches
- `session_file_merge_workflow`: Tests merging sessions file-to-file
- `large_session_stress_test`: 1000 entries, tree building, navigation
- `merge_performance_test`: Performance testing with 500 entries

### #5: Circular Branch Reference Handling
**Status:** ✅ COMPLETED in commit bc78ba5

Implemented cycle detection:
- `detect_cycles()`: Finds all cycles in parent-child relationships using DFS
- `dfs_detect_cycle()`: DFS traversal for cycle detection
- `get_tree()`: Automatically detects and breaks cycles (logs warnings)

**New tests:**
- `get_tree_detects_simple_cycle`: A->B->C->A cycle detection
- `get_tree_handles_self_referential_entry`: Self-pointing entries
- `navigate_to_handles_cycle_gracefully`: Navigation with cycles
- `merge_detects_and_breaks_cycles`: Merge with cycle handling
- `get_tree_validates_parent_chain_integrity`: Orphan parent handling

---

## 🔵 LOW PRIORITY (Large Effort, Lower Impact)

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

## 📊 Summary

### Test Count Progress
- Initial: 339 tests (~94% parity)
- After quick wins: 369 tests (~97% parity)
- After this PR: 398 tests (~99% parity)

### Remaining for 100%
- #14: iTerm2 image protocol (4% impact)
- #15: Kitty image protocol (3% impact)
- Additional test coverage to reach 400+ tests

### Files Modified in This PR
```
crates/pi-agent-core/src/agent/agent_loop.rs       (+169 lines)
crates/pi-coding-agent/src/extensions/mod.rs       (+171 lines)
crates/pi-coding-agent/src/extensions/wasm.rs      (+208 lines)
crates/pi-coding-agent/src/input/file_processor.rs (+220 lines)
crates/pi-coding-agent/src/session/manager.rs      (+858 lines)
crates/pi-coding-agent/tests/session_workflow.rs   (+237 lines, new)
```

### Commits
- 8b68539 feat: implement #9 hook dispatch and #10 tool wrapper wiring
- f6a29b9 test: add merge test coverage for branched trees and ID collisions (#1)
- e1d3bf9 feat: harden schema migrations (#2)
- b48976b feat: add @directory glob expansion support (#7)
- 11d5636 feat: harden WASM safety with additional limits (#11)
- bc78ba5 feat: add integration tests and circular branch detection (#3, #5)
