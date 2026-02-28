# pi-rs vs pi-mono — Parity Comparison

> Generated: 2026-03-01 | pi-rs: 15.5K lines, 103 tests | pi-mono: 128K lines, 100+ test files

## Overall Parity: ~50%

---

## Codebase Size

| Metric | pi-mono (TS) | pi-rs (Rust) | Ratio |
|--------|-------------|-------------|-------|
| Source lines | ~128K | ~15.5K | 12% |
| Test files/tests | 100+ files | 103 tests | — |
| Packages/crates | 7 | 8 | — |

---

## Feature-by-Feature Parity

### 1. AI Providers (`pi-ai`) — 45%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| Anthropic Claude | Yes | Yes | **Parity** |
| OpenAI (GPT-4o, o1, etc.) | Yes | Yes | **Parity** |
| Google Gemini | Yes | Yes | **Parity** |
| Amazon Bedrock | Yes | No | Missing |
| Google Vertex AI | Yes | No | Missing |
| Azure OpenAI | Yes | No | Missing |
| Mistral AI (native) | Yes | Via OpenAI-compat | Partial |
| Groq (native) | Yes | Via OpenAI-compat | Partial |
| xAI Grok | Yes | Via OpenAI-compat | Partial |
| Cerebras | Yes | Via OpenAI-compat | Partial |
| OpenRouter | Yes | Via OpenAI-compat | Partial |
| GitHub Copilot OAuth | Yes | No | Missing |
| Google Gemini CLI OAuth | Yes | No | Missing |
| OpenAI Codex OAuth | Yes | No | Missing |
| Ollama/vLLM/LM Studio | Yes | Via OpenAI-compat | Partial |
| MiniMax, HuggingFace, etc. | Yes | No | Missing |
| Streaming (SSE) | Yes | Yes | **Parity** |
| Tool calling | Yes | Yes | **Parity** |
| Extended thinking (5 levels) | Yes | Yes | **Parity** |
| Vision/image input | Yes | Types only | Partial |
| Prompt caching | Yes | No | Missing |
| Cost tracking | Yes | Yes | **Parity** |
| Model registry | 200+ auto-gen | 30+ manual | Partial |
| OAuth framework (7 providers) | Yes | No | Missing |
| HTTP proxy support | Yes | No | Missing |
| Partial JSON parsing | Yes | No | Missing |

---

### 2. Agent Core (`pi-agent-core`) — 78%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| Agent loop (multi-turn) | Yes | Yes | **Parity** |
| Tool execution pipeline | Yes | Yes | **Parity** |
| Concurrent tool execution | Yes | Yes | **Parity** |
| Tool approval gate | Yes | Yes | **Parity** |
| Streaming events (15+ types) | Yes | Yes | **Parity** |
| State machine | Yes | Yes | **Parity** |
| Steering messages | Yes | Yes | **Parity** |
| Follow-up messages | Yes | Yes | **Parity** |
| Context compaction | Yes | Yes | **Parity** |
| Auto-compaction | Yes | Yes | **Parity** |
| Token budget tracking | Yes | Yes | **Parity** |
| Abort handling | Yes | Yes | **Parity** |
| Max-turns enforcement | Yes | Yes | **Parity** |
| Context transformation hooks | Yes | No | Missing |
| Custom message types | Yes | No | Missing |
| API key per-request override | Yes | No | Missing |
| Thinking budgets per-level | Yes | No | Missing |
| ProxyEvent transport | Yes | Yes | **Parity** |

---

### 3. Tools (`pi-coding-agent/tools`) — 80%

| Tool | pi-mono | pi-rs | Status |
|------|---------|-------|--------|
| Read (offset, limit, truncation) | Yes | Yes | **Parity** |
| Write (mkdir -p) | Yes | Yes | **Parity** |
| Edit (string replace) | Full diff | Basic replace | Partial |
| Bash (timeout, abort, env) | Yes | Yes | **Parity** |
| Grep (regex, glob, context) | Yes | Yes | **Parity** |
| Find (glob, ignore rules) | Yes | Yes | **Parity** |
| Ls (sizes, sorting) | Yes | Yes | **Parity** |
| Smart truncation util | Yes | No | Missing |
| Edit-diff mode | Yes | No | Missing |
| Path security (traversal) | Yes | Yes | **Parity** |
| FileOperations abstraction | Yes | Yes | **Parity** |

---

### 4. Modes — 45%

| Mode | pi-mono | pi-rs | Status |
|------|---------|-------|--------|
| Interactive (full TUI) | Full (146KB) | Basic REPL | ~15% |
| Print mode | Yes | Yes | **Parity** |
| JSON mode | Yes | Yes | **Parity** |
| RPC mode (JSON-RPC) | Full protocol | Core protocol | ~70% |
| SDK mode (programmatic) | Yes | No | Missing |

---

### 5. Session Management — 55%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| Session persistence (JSONL) | Yes | Yes | **Parity** |
| Session resume | Yes | Yes | **Parity** |
| Session branching | Yes | Yes | **Parity** |
| Session forking | Yes | Yes | **Parity** |
| Tree navigation | Yes | No | Missing |
| Session merging | Yes | No | Missing |
| HTML export | Full + ANSI-to-HTML | Basic | ~70% |
| Session metadata/tags | Yes | Basic | Partial |
| Schema migrations | Yes | No | Missing |
| Concurrent session safety | Yes | No | Missing |

---

### 6. Context & Configuration — 60%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| .pi/ directory loading | Yes | Yes | **Parity** |
| AGENTS.md/CLAUDE.md walking | Yes | Yes | **Parity** |
| SYSTEM.md loading | Yes | Yes | **Parity** |
| @file expansion | Yes | Yes | **Parity** |
| @image detection + base64 | Yes | Yes | **Parity** |
| System prompt assembly | Yes | Yes | **Parity** |
| Settings hierarchy (project>user) | Deep merge | No | Missing |
| Settings persistence | Yes | No | Missing |
| Compaction settings | Yes | Basic | Partial |
| Branch summarization | Yes | No | Missing |

---

### 7. Extension & Plugin System — 5%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| Extension types/manifest | 45KB types | Basic types | ~10% |
| Extension loader (npm/local) | Yes | No | Missing |
| Extension runner (hooks) | Yes | No | Missing |
| Hook system (before/after turn) | Yes | No | Missing |
| Tool registration via extension | Yes | No | Missing |
| Command registration | Yes | No | Missing |
| UI integration hooks | Yes | No | Missing |
| Tool wrapping | Yes | No | Missing |

---

### 8. Skills System — 35%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| Skill struct + metadata | Yes | Yes | **Parity** |
| Frontmatter parsing | Yes | Basic (`name`, `description`) | Partial |
| /skill:name commands | Yes | Basic (`/skill:*`) | Partial |
| Discovery from .pi/skills/ | Yes | Yes | **Parity** |
| Package installation | Yes | No | Missing |

---

### 9. Interactive TUI Features — 25%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| TUI framework (diff rendering) | Yes | Yes | **Parity** |
| Components (built-in) | 12+ | 7 | ~60% |
| Keyboard protocol (Kitty) | Yes | Yes | **Parity** |
| Overlay/popup system | Yes | Yes | **Parity** |
| Keybinding configuration | Yes | Yes | **Parity** |
| Autocomplete (file/@//) | 22KB | No | Missing |
| Terminal image rendering | Kitty+iTerm2 | No | Missing |
| Fuzzy matching | Yes | No | Missing |
| Model selector (Ctrl+L) | Yes | No | Missing |
| Thinking selector (Shift+Tab) | Yes | No | Missing |
| Theme system + hot-reload | Yes | No | Missing |
| Session/tree selector | Yes | No | Missing |
| Tool execution visualization | 30KB | No | Missing |
| Streaming message display | Yes | No | Missing |
| Slash commands (20+) | Yes | No | Missing |
| Footer (tokens, cost, model) | Yes | No | Missing |
| Diff display | Yes | No | Missing |

---

### 10. Authentication — 30%

| Feature | pi-mono | pi-rs | Status |
|---------|---------|-------|--------|
| API key from env vars | Yes | Yes | **Parity** |
| Key validation/redaction | Yes | Yes | **Parity** |
| OAuth (4 providers) | Yes | No | Missing |
| Encrypted token storage | Yes | No | Missing |
| Token refresh | Yes | No | Missing |

---

### 11. Peripheral Crates — 12%

| Crate | pi-mono | pi-rs | Status |
|-------|---------|-------|--------|
| pi-mom (Slack bot) | Full | Stubs | ~15% |
| pi-pods (GPU pods) | Full | Stubs | ~15% |
| pi-web-ui (web components) | Full Lit-based | Types only | ~5% |

---

## Weighted Score

| Area | Weight | Parity | Weighted |
|------|--------|--------|----------|
| AI Providers | 15% | 45% | 6.8% |
| Agent Core | 20% | 78% | 15.6% |
| Tools | 10% | 80% | 8.0% |
| Modes | 10% | 45% | 4.5% |
| Session Management | 8% | 55% | 4.4% |
| Context & Config | 7% | 60% | 4.2% |
| Extensions/Plugins | 8% | 5% | 0.4% |
| Skills | 5% | 35% | 1.8% |
| Interactive TUI | 10% | 25% | 2.5% |
| Authentication | 4% | 30% | 1.2% |
| Peripheral (mom/pods/web) | 3% | 12% | 0.4% |
| **Total** | **100%** | — | **49.8%** |

---

## Strength & Gap Analysis

### Strongest Areas (70%+)
- Agent core loop, state machine, events
- Tool suite (7/7 tools implemented)
- Context compaction + auto-compaction
- Streaming infrastructure
- Session persistence + branching

### Biggest Gaps
1. **Extensions/plugins** (5%) — No loader, runner, or hook dispatch
2. **Interactive TUI app layer** (25%) — Framework exists, application UI missing
3. **Skills system** (35%) — Discovery + slash commands landed; tool conversion/installer missing
4. **OAuth** (0%) — No OAuth flows for any provider
5. **Cloud providers** — Bedrock, Vertex, Azure all missing

### Top 5 Items to Close the Gap Fastest

| Priority | Item | Estimated Impact |
|----------|------|-----------------|
| 1 | Interactive TUI application layer (streaming display, selectors, slash commands) | +7% |
| 2 | Extension/plugin system (loader, runner, hook dispatch) | +6% |
| 3 | Skills system (frontmatter parsing, discovery, /skill commands) | +5% |
| 4 | Cloud providers (Bedrock, Vertex, Azure) | +4% |
| 5 | OAuth framework (token storage, refresh, provider flows) | +3% |

Completing all 5 would bring parity from **48% to ~73%**.
