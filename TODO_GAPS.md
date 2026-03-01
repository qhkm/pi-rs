# Pi-rs Gap List - Remaining Work for 100% Parity

> Generated from gap analysis: ~94% → 100% parity  
> Last updated: 2026-03-01

---

## Current Status

**Completed:** #13, #16, #12, #4, #8, #10, #9  
**In Progress:** None  
**Remaining:** 10 items

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

## 🟡 REMAINING HIGH PRIORITY

### #1: Merge Test Coverage
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
**Impact:** 5% | **File:** `crates/pi-coding-agent/src/session/manager.rs`

**Current issues:**
1. Uses `Utc::now()` for missing timestamps instead of preserving original
2. No handling for malformed entries (just skips them)
3. No v0 → v1 migration path
4. No handling for header corruption

**Fixes needed:**
1. Extract timestamp from entry if possible, only use `Utc::now()` as fallback
2. Add v0 detection (entries without `type` field)
3. Add repair mode for corrupted headers
4. Preserve unknown fields during migration

**Code location:** `migrate_session()` function around line 570+

---

### #7: @directory Expansion with Globs
**Impact:** 4% | **File:** `crates/pi-coding-agent/src/input/file_processor.rs`

**Add support for:**
- `@dirname/` → include all files in directory
- `@dirname/**/*.rs` → glob pattern expansion
- `@"path with spaces/"` → quoted directory paths

**Implementation:**
- Add glob dependency or use simple pattern matching
- In `process_input()`, detect trailing `/` in reference
- Expand to individual files before processing
- Handle recursion depth limit

---

## 🟢 MEDIUM PRIORITY

### #11: WASM Safety Hardening
**Impact:** 3% | **File:** `crates/pi-coding-agent/src/extensions/wasm.rs`

**Current:** Basic fuel limit only  
**Needed:**
1. Instruction count limits (not just fuel)
2. Memory bounds checking at offset 1024
3. I/O timeout for WASM module stdio
4. Stack depth limits

**Code:** `WasmModule::execute()` around line 85+

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
