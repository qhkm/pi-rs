# π (pi) — AI Coding Agent

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-330+-green.svg)]()

> **Minimal core. Maximum extensibility. Your workflow, your way.**

A high-performance, terminal-native AI coding agent built in Rust. This is a Rust port of [pi-mono](https://github.com/badlogic/pi-mono), originally created by [Mario Zechner](https://github.com/badlogic). Features 101+ LLM models across 17 cloud providers, advanced TUI with streaming responses, and extensible plugin architecture.

**Philosophy:** Pi provides primitives, not features. Sub-agents, plan mode, permission gates—build them yourself or install a package. Adapt pi to your workflows, not the other way around. [Read more →](PHILOSOPHY.md)

## 🚀 Quick Start

```bash
# Clone and build
git clone https://github.com/yourusername/pi-rs
cd pi-rs
cargo build --release

# Run interactive mode
./target/release/pi

# Or with a specific model
pi --model claude-3-opus

# Run a one-off task
pi "Refactor src/main.rs to use async/await"
```

## ✨ Features

### 🧩 Philosophy: Primitives, Not Features

| What Others Bake In | How Pi Does It |
|---------------------|----------------|
| Sub-agents | Spawn via tmux, or build with extensions |
| Plan mode | Write to files, or build with extensions |
| Permission popups | Run in container, or build your own flow |
| Built-in to-dos | Use TODO.md, or build with extensions |
| MCP integration | Build as skills, or add via extensions |

**The result:** A 15MB binary that starts in <50ms instead of 100MB+ and 1-3s.

### 🤖 AI Providers (17+ Supported)
- **Anthropic**: Claude 3 Opus, Sonnet, Haiku
- **OpenAI**: GPT-4o, GPT-4.1, o1, o3, o4-mini
- **Google**: Gemini 2.5 Pro, 2.0 Flash
- **Azure**: OpenAI GPT-4o, GPT-4.1
- **AWS**: Bedrock (Claude, Llama 4)
- **Google Vertex**: Gemini 2.5, Claude
- **Native**: Mistral, Groq, xAI (Grok), Cerebras, OpenRouter, MiniMax, HuggingFace, Perplexity, DeepSeek

### 🖥️ Terminal UI
- **Streaming responses** with animated cursor
- **Syntax highlighting** for code blocks
- **Diff viewer** (unified & side-by-side)
- **Model selector** (Ctrl+L)
- **Thinking level selector** (Shift+Tab)
- **Tool execution visualization**
- **Fuzzy file search** (@file expansion)
- **Theme system** with hot-reload

### 🛠️ Developer Tools
- **7 built-in tools**: Read, Write, Edit, Bash, Grep, Find, Ls
- **Smart truncation** with context preservation
- **Diff editing** with conflict resolution
- **Concurrent tool execution**
- **Path security** (prevents directory traversal)

### 🔌 Extension System
- **Shell extensions**: Bash scripts as tools
- **Binary extensions**: JSON-RPC over stdio
- **WASM extensions**: Sandboxed WebAssembly plugins
- **Hook system**: Before/after turn events
- **Command registration**: Custom slash commands

### 📝 Skills System
- **Markdown-based**: Simple SKILL.md format
- **YAML frontmatter**: Full metadata support
- **Git install**: `pi /skill:install https://github.com/user/skill`
- **Tag-based discovery**: Search by categories
- **Auto-converted to tools**: Skills become available as agent tools

### 🔐 Authentication
- **OAuth 2.0 Device Flow**: GitHub Copilot, Google Gemini, OpenAI Codex
- **Encrypted storage**: XOR + keyring for tokens
- **Auto-refresh**: Tokens refreshed before expiration
- **API key fallback**: Environment variable support

## 📦 Installation

### From Source
```bash
cargo install --path crates/pi-coding-agent
```

### Homebrew (macOS/Linux)
```bash
brew tap yourusername/pi
brew install pi
```

## 🎯 Usage

### Interactive Mode
```bash
pi                          # Start interactive session
pi --model gpt-4o          # Use specific model
pi --thinking high         # Set reasoning level
```

### One-shot Mode
```bash
pi "explain this codebase"
pi --file src/main.rs "refactor this"
```

### JSON Mode
```bash
pi --json "list all TODOs" | jq .
```

### RPC Mode
```bash
pi --rpc-server            # Start JSON-RPC server
```

### Slash Commands
| Command | Description |
|---------|-------------|
| `/clear` | Clear conversation history |
| `/compact` | Compact context to save tokens |
| `/model <name>` | Switch AI model |
| `/thinking <level>` | Set thinking level |
| `/settings` | Open settings |
| `/export` | Export conversation |
| `/help` | Show help |

## ⚙️ Configuration

### Settings Hierarchy
```
~/.pi/settings.json        # User settings
./.pi/settings.json        # Project settings (overrides user)
```

### Example `.pi/settings.json`
```json
{
  "model": "claude-3-opus",
  "thinking_level": "medium",
  "auto_compact": true,
  "max_turns": 50,
  "theme": "dark"
}
```

### AGENTS.md
Place an `AGENTS.md` in your project root for project-specific instructions:
```markdown
# Project Guidelines

## Code Style
- Use snake_case for functions
- Use PascalCase for types
- Always handle errors explicitly
```

## 🧩 Creating Extensions

### Extension Manifest (`extension.json`)
```json
{
  "name": "my-extension",
  "version": "1.0.0",
  "description": "My custom tools",
  "tools": [
    {
      "name": "lint",
      "description": "Run linter",
      "executor": "shell",
      "command": "cargo clippy -- -D warnings"
    }
  ]
}
```

### Installing Extensions
```bash
# Place in .pi/extensions/my-extension/extension.json
pi /extension:enable my-extension
```

## 📝 Creating Skills

### Skill File (`SKILL.md`)
```markdown
---
name: rust-best-practices
description: Rust coding standards
version: 1.0.0
author: Your Name
tags:
  - rust
  - style
---

# Rust Best Practices

- Use `?` operator for error propagation
- Prefer `&str` over `String` for parameters
- Document all public APIs
```

### Installing Skills
```bash
pi /skill:install ./path/to/skill
pi /skill:install https://github.com/user/skill-repo
```

## 🎨 Theming

Built-in themes: `dark`, `light`, `high-contrast`

Custom theme (`~/.pi/theme.json`):
```json
{
  "name": "custom",
  "fg": {
    "accent": "cyan",
    "success": "green"
  },
  "syntax": {
    "keyword": "magenta",
    "string": "green"
  }
}
```

## 🏗️ Architecture

```
┌─────────────────┐
│   pi (binary)   │
├─────────────────┤
│  pi-coding-agent│  ← CLI, modes, interactive
├─────────────────┤
│  pi-agent-core  │  ← Agent loop, tools, hooks
├─────────────────┤
│     pi-ai       │  ← LLM providers, streaming
├─────────────────┤
│     pi-tui      │  ← Terminal UI components
└─────────────────┘
```

### Workspace Crates
| Crate | Purpose | Tests |
|-------|---------|-------|
| `pi-ai` | LLM providers, OAuth, models | 81 |
| `pi-agent-core` | Agent loop, tools, context | 157 |
| `pi-tui` | Terminal UI framework | 51 |
| `pi-coding-agent` | CLI, modes, extensions | 28 |

## 🧪 Testing

```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p pi-ai
cargo test -p pi-agent-core
cargo test -p pi-tui
```

## 📊 Comparison

| Metric | pi (Rust) | Others (Node/TS) |
|--------|-----------|------------------|
| Lines of Code | ~35K | ~128K+ |
| Binary Size | ~15MB | 100MB+ (bundled) |
| Startup Time | <50ms | 1-3s |
| Memory Usage | ~50MB | 200MB+ |
| Test Coverage | 330+ tests | Varies |

## 🤝 Contributing

Contributions welcome! Please read our [Contributing Guide](CONTRIBUTING.md).

### Development
```bash
# Run with logging
RUST_LOG=debug cargo run --bin pi

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy --workspace
```

## 📄 License

This project is licensed under the [MIT License](LICENSE).

## 🙏 Acknowledgments

- Based on [pi-mono](https://github.com/badlogic/pi-mono) by [Mario Zechner](https://github.com/badlogic) — thank you for the original inspiration
- Inspired by Claude Code and similar AI coding assistants
- Built with [Tokio](https://tokio.rs), [Ratatui](https://github.com/ratatui-org/ratatui) concepts, and the Rust ecosystem
- Thanks to all contributors

---

<div align="center">

**[Philosophy](PHILOSOPHY.md)** • **[Documentation](https://docs.rs/pi-coding-agent)** • **[Issues](https://github.com/yourusername/pi-rs/issues)** • **[Discussions](https://github.com/yourusername/pi-rs/discussions)**

</div>
