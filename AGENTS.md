# π (pi) — AI Coding Agent

## Project Identity
- **Name**: π (pi) - A Rust-based AI coding agent
- **Type**: Terminal-native AI coding assistant with TUI
- **Origin**: Rust port of pi-mono by Mario Zechner
- **Philosophy**: Minimal core, maximum extensibility. Primitives, not features.

## Architecture

### Workspace Layout
```
pi-rs/
├── crates/
│   ├── pi-ai/           # LLM providers (Anthropic, OpenAI, Google, etc.)
│   ├── pi-agent-core/   # Agent loop, tool system, context management
│   ├── pi-tui/          # Terminal UI components (ratatui-based)
│   └── pi-coding-agent/ # CLI entry point, modes, extensions
├── Cargo.toml           # Workspace manifest
└── README.md            # User documentation
```

### Key Components
1. **pi-ai**: OAuth, API keys, 17+ LLM providers, streaming
2. **pi-agent-core**: Agent loop, 7 built-in tools, compaction, hooks
3. **pi-tui**: Fuzzy finder, diff viewer, image rendering
4. **pi-coding-agent**: Interactive/print/JSON/RPC modes, extensions

## Built-in Tools
- `read` - Read file contents
- `write` - Write new files
- `edit` - Edit existing files (diff-based)
- `bash` - Execute shell commands
- `grep` - Search file contents
- `find` - Find files by name
- `ls` - List directory contents

## Extension System
- **Shell extensions**: Bash scripts as tools
- **Binary extensions**: JSON-RPC over stdio
- **WASM extensions**: Sandboxed WebAssembly plugins
- **Skills**: Markdown-based knowledge (SKILL.md format)

## Key Features
- 101+ LLM models (Claude, GPT-4, Gemini, Llama, etc.)
- Streaming responses with syntax highlighting
- Tool approval gates for security
- Session management and conversation history
- Fuzzy file search with @file expansion
