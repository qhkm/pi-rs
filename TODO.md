# pi-rs — Status & Roadmap

Rust rewrite of [badlogic/pi-mono](https://github.com/badlogic/pi-mono) (TypeScript AI agent toolkit).

---

## What's Done

### pi-ai — Unified LLM API ✅
- [x] Core types: `Message`, `Content`, `Usage`, `UsageCost`, `StopReason`, `ThinkingLevel`
- [x] `UserMessage`, `AssistantMessage`, `ToolResultMessage`, unified `Message` enum
- [x] `StreamEvent` hierarchy: Start, TextStart/Delta/End, ThinkingStart/Delta/End, ToolCallStart/Delta/End, Done, Error
- [x] `EventStreamSender` / `EventStreamReceiver` (async mpsc-based, implements `futures::Stream`)
- [x] SSE parser (`SseStream`) — W3C-compliant, works with reqwest byte streams
- [x] `LLMProvider` trait with `stream()`, `stream_simple()`, `complete()`
- [x] `ProviderCapabilities` (streaming, tool_calling, thinking, vision)
- [x] `Context` builder (system prompt, messages, tools)
- [x] `StreamOptions` / `SimpleStreamOptions` (temperature, max_tokens, reasoning level)
- [x] **Anthropic provider** — full Messages API with extended thinking, tool calling, SSE streaming
- [x] **OpenAI provider** — Chat Completions with compatibility layer for Groq, Mistral, xAI, Cerebras, OpenRouter
- [x] **Google provider** — Gemini GenerativeAI with tool calling and thinking blocks
- [x] Provider registry (`register_provider` / `get_provider` / `register_defaults`)
- [x] Model registry — 20+ models with pricing, context windows, capabilities
- [x] Cost calculation (`calculate_cost`, `annotate_usage`)
- [x] Auth — API key resolution from env vars for 11+ providers, placeholder detection
- [x] `ToolDefinition`, `ToolCall`, `ToolResult` types with JSON Schema support
- [x] Cross-provider message transforms: strip thinking, normalize tool IDs, merge consecutive messages
- [x] API key redaction in error messages (`redact_key()` — first 4 + `***` + last 4)
- [x] Google API key moved to `x-goog-api-key` header (not URL query param)
- [x] Provider registry recovers from poisoned locks (no panics)
- [x] SSE parser: fixed event drop bug (VecDeque buffer), fixed separator precedence
- [x] 22 unit tests passing

### pi-tui — Terminal UI Library ✅
- [x] `Terminal` trait abstraction with `ProcessTerminal` (real) and `VirtualTerminal` (test)
- [x] Raw mode, Kitty keyboard protocol (CSI >31u), bracketed paste
- [x] `DifferentialRenderer` — only redraws changed lines, tracks previous state
- [x] Synchronized output (CSI ?2026h/l) for flicker-free rendering
- [x] `Component` trait: `render()`, `handle_input()`, `invalidate()`, `is_dirty()`
- [x] **Input** — single-line editor with cursor nav, kill ring, undo (200 entries), horizontal scroll
- [x] **Editor** — multi-line editor with vertical/horizontal scroll, undo/redo stacks, kill ring, selection struct
- [x] **Markdown** — full renderer with headings, lists, tables (ASCII art), code blocks, links, styling via theme
- [x] **SelectList** — filterable item selection with descriptions, scroll indicator, callbacks
- [x] **Text / TruncatedText** — word wrapping, ellipsis truncation, ANSI-aware
- [x] **Container / TuiBox / Spacer** — layout primitives
- [x] **Loader** — animated braille spinner with configurable message
- [x] `OverlayManager` — anchoring (8 positions), sizing (absolute/percentage), clamping
- [x] Kitty keyboard protocol parser — CSI, SS3, control chars, modifiers
- [x] `KeybindingsManager` — 25 configurable editor actions with default emacs-style bindings
- [x] 11 unit tests passing

### pi-agent-core — Agent Runtime ✅
- [x] `Agent` struct with event-driven loop
- [x] `AgentConfig`: provider, model, system prompt, max turns, token budget, compaction, thinking level
- [x] `AgentState`: Idle, Streaming, ExecutingTools, Compacting, Aborted
- [x] `AgentEvent` enum: agent/turn/message start/end, tool execution start/update/end, auto-compaction, **tool approval**
- [x] `AgentEndReason`: Completed, MaxTurns, Aborted, Error, ContextOverflow
- [x] `AgentTool` trait with `execute()`, `execute_streaming()`, `requires_approval()`, `to_tool_definition()`
- [x] `ToolResult`, `ToolProgress`, `ToolContext` (with abort signal via `watch::Receiver<bool>`)
- [x] `ToolRegistry` — register/unregister/activate/deactivate tools, active subset management
- [x] `MessageQueue` — steering (interrupt mid-turn) and follow-up (after completion) queues
- [x] `AgentMessage` enum: Llm, SystemContext, CompactionSummary, Extension
- [x] `to_llm_messages()` — context pipeline (AgentMessage → pi_ai::Message)
- [x] Token estimation (`estimate_tokens`, chars/4 heuristic matching TS version)
- [x] `TokenBudget` — context window management with reserves
- [x] `CompactionSettings` with `find_compaction_split()` and `build_compaction_prompt()`
- [x] `ContextUsage` statistics
- [x] `ProxyEvent` — serializable wire format for browser→server streaming
- [x] `AgentError` enum with `thiserror` and `From` conversions
- [x] **Approval gate** — `ToolApprovalRequired`/`ToolApprovalResult` events, mpsc channel, 5-minute timeout
- [x] 2 unit tests passing

### pi-coding-agent — CLI Coding Agent ✅
- [x] `clap` CLI argument parsing (provider, model, thinking, mode, session, print, verbose, etc.)
- [x] 7 built-in tools:
  - [x] `read` — file reading with line offset/limit, numbered output
  - [x] `write` — file creation/overwrite with auto mkdir
  - [x] `edit` — exact string replacement with uniqueness validation
  - [x] `bash` — shell execution with timeout, approval required
  - [x] `grep` — regex search with glob filtering and result cap
  - [x] `find` — glob-based file search
  - [x] `ls` — directory listing with file sizes
- [x] `FileOperations` trait for SSH/remote delegation, `LocalFileOps` default
- [x] **Path traversal protection** — `resolve_and_validate_path()` with canonicalization + blocked system prefixes
- [x] Session persistence — JSONL format (version 3 header, tree structure with id/parentId)
- [x] `SessionManager` — create, append entries, list sessions
- [x] `SessionEntry` enum: Message, Compaction, ModelChange, ThinkingLevelChange, Label
- [x] Print mode — stream text deltas to stdout
- [x] JSON mode — emit JSONL events via ProxyEvent
- [x] Extension types — `ExtensionManifest`, `ExtensionToolDef`, `ExecutorType` (Shell/Binary/Wasm)
- [x] `main.rs` — full wiring: provider resolution, model lookup, tool registration, mode routing
- [x] 8 unit tests passing (path traversal)

### pi-mom — Slack Bot (structural) ✅
- [x] `SlackEvent`, `SlackEventType`, `SlackFile`, `SlackContext` types
- [x] `SocketModeClient` struct
- [x] `ChannelState` — per-channel paths (log.jsonl, MEMORY.md, scratch/)
- [x] `MomState` — global workspace state
- [x] `SandboxConfig` enum (Host/Docker) with `exec_in_sandbox()`
- [x] `ScheduledEvent` enum (Immediate/OneShot/Periodic)
- [x] `Skill` struct, `load_skills()` placeholder
- [x] `main.rs` — CLI with clap, token validation

### pi-web-ui — Web UI (structural) ✅
- [x] `SessionMetadata`, `SessionUsage` types
- [x] `AppSettings`, `ProviderKey`, `CustomProvider` types
- [x] `Artifact` enum (Html/Svg/Markdown)
- [x] `Attachment`, `AttachmentType` types

### pi-pods — GPU Pod Manager (structural) ✅
- [x] `Config` with `load()`/`save()` (JSON at ~/.pi/pods/config.json)
- [x] `Pod`, `Gpu`, `RunningModel` types
- [x] `ModelConfig` with known Qwen model presets
- [x] `PodCommand` subcommands (Setup, List, Active, Remove, Start, Stop, Logs, Shell)
- [x] `PodProvider` enum (DataCrunch, RunPod, Vast.ai, PrimeIntellect, AWS EC2)
- [x] `ssh_exec()` — remote command execution
- [x] `build_vllm_command()` — vLLM launch command builder
- [x] `main.rs` — List and Active commands implemented

---

## Code Review Fixes — All Completed ✅

All 6 Critical, 13 Important, and 8 Minor issues from the comprehensive code review have been fixed:

| ID | Issue | Fix | Tests |
|----|-------|-----|-------|
| C1 | Google API key leaked in URL query string | Moved to `x-goog-api-key` header | — |
| C2 | No path traversal protection on file tools | `resolve_and_validate_path()` with canonicalization, cwd boundary, blocked system prefixes | 8 |
| C3 | Bash tool approval gate missing | `ToolApprovalRequired`/`ToolApprovalResult` events, mpsc approval channel, 5-min timeout | — |
| C4 | SSE parser drops events + wrong separator precedence | `VecDeque` buffer for pending events, `min()` for first separator | 4 |
| C5 | API key leaked in error messages | `redact_key()` shows first 4 + `***` + last 4 | 6 + 1 doctest |
| C6 | Provider registry panics on poisoned lock | `.unwrap_or_else(\|e\| e.into_inner())` on all RwLock ops | — |

---

## Code Review Fixes — Important (All Fixed ✅)

| ID | Issue | Fix |
|----|-------|-----|
| I1 | Bash tool ignores abort signal | Rewrote with `tokio::select!` racing `child.wait()` vs `abort_rx.changed()`; kills child on abort/timeout |
| I2 | Excessive `partial.clone()` in stream handlers | Removed `partial` from `TextDelta`/`ThinkingDelta`/`ToolCallDelta` events; emit only deltas |
| I3 | Tool execution is sequential | Rewrote with `futures::future::join_all` for concurrent tool execution |
| I4 | OpenAI provider ignores system prompt | System prompt prepended as `"developer"` or `"system"` role message |
| I5 | `complete()` method deadlocks | Uses `tokio::join!` with 1024-capacity channel to drive producer+consumer concurrently |
| I6 | No timeout on LLM requests | All 3 providers use `reqwest::Client::builder().timeout(Duration::from_secs(300))` |
| I7 | OpenAI `stream_simple` ignores reasoning | Overridden to send `reasoning_effort` for o3/o4-mini models |
| I8 | Grep tool `--include` after `--` separator | Moved `--include` before `--` so grep treats it as an option |
| I9 | Agent state not set to Aborted on abort | State set to `AgentState::Aborted` when abort occurs |
| I10 | `broadcast::channel(256)` may drop events | Buffer increased to 4096 |
| I11 | `register_defaults()` silently skips providers | Returns `Vec<String>` of warnings for missing API keys |
| I12 | Follow-up messages never processed | Follow-ups re-enter the loop via `'outer: loop` wrapper |
| I13 | `built_in_models()` allocates every call | Cached in `static BUILT_IN_MODELS: LazyLock<Vec<Model>>` |

---

## Code Review Fixes — Minor (All Fixed ✅)

| ID | Issue | Fix |
|----|-------|-----|
| M1 | Dead `_agent_id` parameter | Now used in `tracing::debug!` |
| M2 | Missing `Default` impl for tool structs | Added `impl Default` for `BashTool`, `GrepTool`, `FindTool`, `LsTool` |
| M3 | `#[allow(dead_code)]` on serde structs | Removed unnecessary allows from Anthropic provider |
| M4 | Duplicate `is_error` check in bash tool | Merged into single `if/else` |
| M5 | Unused `Serialize` import | Removed from OpenAI provider |
| M6 | Interactive mode is a no-op | Full REPL with streaming output, tool display, and approval prompts |
| M7 | `once_cell` → `std::sync::LazyLock` | Replaced and removed `once_cell` dependency |
| M8 | `resolve_path` duplication | Unified in `operations.rs` (done during C2 fix) |

---

## Recently Ported from pi-mono ✅

| Feature | Crate | Key Files | Tests |
|---------|-------|-----------|-------|
| Context compaction (LLM call) | pi-agent-core | `context/compaction.rs` — prompts, `serialize_conversation()`, `should_compact()` | 4 |
| Auto-compaction in agent loop | pi-agent-core | `agent/agent_loop.rs` — `run_compaction()`, checks after each assistant message | — |
| Context injection (.pi/) | pi-coding-agent | `context/resource_loader.rs` — loads AGENTS.md, CLAUDE.md, SYSTEM.md, APPEND_SYSTEM.md | 12 |
| RPC mode (JSON-RPC) | pi-coding-agent | `modes/rpc.rs`, `modes/rpc_types.rs` — prompt, abort, get_state, get_messages | — |
| @filename expansion | pi-coding-agent | `input/file_processor.rs` — `<file>` tags, image extraction, base64 | 10 |
| Session branching (branch/fork) | pi-coding-agent | `session/manager.rs` — `branch()`, `fork()` methods | 4 |
| HTML export | pi-coding-agent | `export/html.rs` — standalone dark-themed HTML with tool calls, thinking | 9 |
| Skills discovery + install workflow | pi-coding-agent | `skills/mod.rs`, `modes/interactive.rs`, `main.rs` — SKILL.md discovery, frontmatter parsing, `/skill:*` commands, skill tool registration, local install command | 6 |
| Extension loader + shell/binary tools | pi-coding-agent | `extensions/mod.rs`, `main.rs` — local extension discovery, `extension.json` parsing, shell + binary extension tool registration | 4 |

---

## What's Left To Do (Features)

### pi-ai

- [ ] **Azure OpenAI provider** — separate auth (AD tokens), different base URL format
- [ ] **Amazon Bedrock provider** — AWS SigV4 signing, ConverseStream API
- [ ] **Google Vertex provider** — ADC/OAuth, regional endpoints
- [ ] **Ollama provider** — OpenAI-compatible but local, model management
- [ ] **vLLM provider** — OpenAI-compatible with custom model paths
- [ ] **OpenAI Responses API** — new streaming format (different from Chat Completions)
- [x] **Retry/backoff decorator** — exponential backoff with jitter for rate limits
- [ ] **Fallback provider decorator** — circuit breaker pattern, primary→secondary failover
- [x] **Vision provider wiring** — wire image Content through Anthropic/OpenAI/Google request builders
- [ ] **OAuth flows** — CLI login for Claude Pro/Max, ChatGPT Plus, Google
- [x] **Prompt caching** — explicit cache control headers (Anthropic)
- [ ] **Model auto-discovery** — runtime model listing from provider APIs
- [ ] **Batch API support** — async batch processing
- [ ] Integration tests against real provider APIs (behind `live-tests` feature flag)
- [ ] Mock provider for downstream testing (behind `mock-providers` feature)

### pi-tui

- [ ] **Syntax highlighting** — integrate `syntect` with the Editor component (infrastructure ready, `language` field exists)
- [ ] **Selection editing** — wire Selection struct to keyboard events (cut/copy/paste)
- [ ] **System clipboard** — integrate with OS clipboard (currently only internal kill ring)
- [ ] **Mouse support** — click-to-position, scroll, selection
- [ ] **Image component** — terminal image rendering (Kitty/iTerm2 protocol)
- [ ] **Redo keyboard binding** — Editor has redo stack but no key triggers it
- [ ] **YankPop fix** — Input component's `yank_pop()` should replace last yank, not append
- [ ] **Event loop framework** — high-level TUI app loop (create terminal → render loop → input dispatch → cleanup)
- [ ] **Keybinding config files** — load from JSON/TOML at runtime
- [ ] **Autocomplete** — slash commands + file path completion in Editor

### pi-agent-core ✅ COMPLETE

- [x] **Tool definition token estimation** — count tokens used by tool schemas in context
- [x] **Streaming tool execution** — use `execute_streaming()` with progress events in agent loop
- [x] **Session ID threading** — pass session IDs through to providers for cache reuse
- [x] **Dynamic thinking budgets** — adjust thinking level per-turn based on task complexity
- [x] **Agent abort cleanup** — ensure graceful cleanup of in-flight tool executions on abort
- [x] **Event persistence** — write events to a log for replay/debugging

### pi-coding-agent

- [ ] **Full interactive TUI mode** — replace stdin REPL with pi-tui components:
  - [ ] Multi-line Editor for input
  - [ ] Streaming Markdown for assistant responses
  - [ ] Tool call rendering (collapsible, syntax highlighted)
  - [ ] Status bar (model, tokens, cost, session)
  - [ ] Command palette (slash commands)
  - [ ] Session tree navigation (`/tree`, `/fork`)
- [ ] **Extension loader** — scan directories, load manifests, instantiate shell/binary/WASM tools
  - [x] Discover local extensions from `~/.pi/agent/extensions` and `.pi/extensions`
  - [x] Parse `extension.json` manifests
  - [x] Register shell extension tools in agent startup
  - [x] Binary extension executor wiring (JSON-RPC over stdio)
  - [ ] WASM extension executor wiring
- [ ] **WASM plugin executor** — sandboxed execution via `wasmtime`
- [ ] **Binary plugin executor** — JSON-RPC 2.0 over stdin/stdout with external processes
  - [x] Basic stdio JSON-RPC request/response execution path
  - [ ] Multi-message streaming protocol + robust lifecycle management
- [ ] **Custom commands** — register/execute extension commands
- [x] **Event hooks** — HookRegistry with 8 event types, before/after dispatch
- [x] **Prompt templates** — PromptRegistry with frontmatter parsing, variable expansion
- [ ] **Skills system** — discover SKILL.md files, convert to tools
  - [x] Discover `SKILL.md` from `~/.pi/skills` and `.pi/skills`
  - [x] Parse basic frontmatter (`name`, `description`)
  - [x] Interactive slash commands: `/skills`, `/skill:list`, `/skill:<name>`, `/skill:clear`
  - [x] Convert discovered skills into callable tools (`skill_<name>`)
  - [x] Local skill installation workflow (`/skill:install <path>`)
  - [ ] Remote/registry-based skill package installation
- [ ] **Themes** — configurable color schemes for TUI and markdown
- [x] **Model cycling** — `--models` flag for switching between models
- [ ] **Share** — generate shareable session URL

### pi-mom

- [ ] **Slack Socket Mode connection** — WebSocket handshake, reconnection, heartbeat
- [ ] **Event routing** — dispatch mentions/DMs to agent runner
- [ ] **Agent runner** — per-channel AgentSession with streaming responses
- [ ] **Message logging** — append to per-channel log.jsonl
- [ ] **Memory management** — load/save MEMORY.md, inject into system prompt
- [ ] **Skill creation** — mom auto-creates CLI tools for recurring workflows
- [ ] **Skill discovery** — scan workspace + channel skill directories
- [ ] **Attachment handling** — download Slack files, store in attachments/
- [ ] **Thread support** — reply in threads, maintain thread context
- [ ] **Events system** — cron scheduling, one-shot timers, immediate triggers
- [ ] **Artifacts server** — serve generated HTML/JS with live reload
- [ ] **Docker sandbox** — full container lifecycle management
- [ ] **Multi-channel** — concurrent independent agent sessions per channel
- [ ] **Message formatting** — convert markdown to Slack mrkdwn
- [ ] **File upload** — share tool outputs as Slack files
- [ ] **Typing indicator** — show typing while agent is working
- [ ] **Silent mode** — `[SILENT]` response suppression
- [ ] **Self-installation** — auto-install tools (apt/npm/apk) as needed

### pi-web-ui

- [ ] **HTTP backend server** — serve the web UI and proxy LLM requests
- [ ] **WebSocket streaming** — real-time event forwarding to browser
- [ ] **IndexedDB storage backend** — settings, sessions, provider keys via WASM bridge
- [ ] **Storage transactions** — atomic multi-store operations
- [ ] **ChatPanel component** — messages + input + artifacts (keep Lit frontend, Rust backend)
- [ ] **AgentInterface** — message display with streaming, tool call rendering
- [ ] **ArtifactsPanel** — render HTML/SVG/Markdown documents
- [ ] **JavaScript REPL tool** — sandboxed browser execution
- [ ] **Document extraction tool** — text extraction from URLs (CORS proxy)
- [ ] **Attachment handling** — PDF/DOCX/XLSX/PPTX text extraction
- [ ] **Settings dialog** — provider keys, model selection, theme
- [ ] **Session list** — browse/resume/delete sessions
- [ ] **Custom providers dialog** — configure Ollama, LM Studio, vLLM endpoints
- [ ] **Quota tracking** — IndexedDB usage monitoring
- [ ] **WASM bindings** — export Rust types/functions to JS via `wasm-bindgen`
- [ ] **CORS proxy** — automatic proxying for browser environments
- [ ] **Internationalization** — configurable translations

### pi-pods

- [ ] **Setup command** — SSH into pod, install vLLM + CUDA, detect GPUs
- [ ] **Start command** — launch vLLM model with auto GPU assignment
- [ ] **Stop command** — kill running model process
- [ ] **Logs command** — stream vLLM stdout/stderr
- [ ] **Shell command** — interactive SSH session
- [ ] **SSH command** — run arbitrary command on pod
- [ ] **GPU auto-assignment** — round-robin, least-used-first selection
- [ ] **Multi-GPU support** — tensor parallelism, data parallelism configuration
- [ ] **Model configs** — expand known model database (GLM-4.5, GPT-OSS, etc.)
- [ ] **DataCrunch integration** — API for pod provisioning, shared NFS
- [ ] **RunPod integration** — network volume management
- [ ] **Vast.ai / PrimeIntellect / AWS EC2** — provider-specific pod management
- [ ] **Agent mode** — interactive chat with a running model (reuse pi-agent-core)
- [ ] **Health checks** — monitor model availability and performance
- [ ] **vLLM version management** — release/nightly/gpt-oss builds
- [ ] **API key authentication** — configure per-model API keys

### Cross-cutting

- [ ] **CI/CD** — GitHub Actions for `cargo check`, `cargo test`, `cargo clippy`
- [ ] **Benchmarks** — criterion benchmarks for agent loop, context building, SSE parsing
- [ ] **Rustdoc** — comprehensive API documentation for all public types
- [ ] **Examples** — usage examples for each crate
- [ ] **Release automation** — lockstep versioning, crates.io publishing
- [ ] **Clippy compliance** — zero warnings across workspace
- [ ] **Error messages** — user-friendly errors with suggestions (miette or similar)
- [ ] **Configuration** — unified config system (~/.pi/ directory)
- [ ] **Logging** — structured tracing throughout all crates

---

## Stats

| Crate | Tests |
|-------|-------|
| pi-ai | 69 |
| pi-tui | 11 |
| pi-agent-core | 23 |
| pi-coding-agent | 151 |
| pi-mom | 0 |
| pi-web-ui | 0 |
| pi-pods | 0 |
| **Total** | **265** |
