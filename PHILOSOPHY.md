# Philosophy of π (pi)

> **Minimal core. Maximum extensibility. Your workflow, your way.**

## Why pi?

Pi is a minimal terminal coding harness. Adapt pi to your workflows, not the other way around. Extend it with Rust extensions, skills, prompt templates, and themes. Bundle them as pi packages and share via git or npm.

## Primitives, Not Features

Features that other agents bake in, you can build yourself. Extensions are Rust modules with access to tools, commands, keyboard shortcuts, events, and the full TUI.

**What this means:**
- **No sub-agents** → Spawn pi instances via tmux, or build your own with extensions
- **No plan mode** → Write plans to files, or build it with extensions
- **No permission popups** → Run in a container, or build your own confirmation flow
- **No built-in to-dos** → Use a TODO.md file, or build your own with extensions
- **No background bash** → Use tmux. Full observability, direct interaction
- **No MCP** → Build CLI tools as skills, or add MCP support via extensions

**Why?** Because your workflow is unique. Pi provides the primitives—you shape it to fit how you work.

## Context Engineering

Pi's minimal system prompt and extensibility let you do actual context engineering. Control what goes into the context window and how it's managed.

### Mechanisms

| Mechanism | Purpose |
|-----------|---------|
| **AGENTS.md** | Project instructions loaded from parent directories |
| **SYSTEM.md** | Replace or append to the default system prompt |
| **Skills** | Capability packages loaded on-demand |
| **Prompt templates** | Reusable prompts as Markdown files |
| **Extensions** | Dynamic context injection before each turn |
| **Compaction** | Auto-summarizes older messages (customizable) |

## Four Modes, One Core

Pi adapts to your integration needs:

1. **Interactive** — Full TUI with streaming, tool visualization, and keyboard shortcuts
2. **Print/JSON** — Scriptable output for automation
3. **RPC** — JSON protocol for non-Rust integrations
4. **SDK** — Embed pi in your applications

## Tree-Structured Sessions

Sessions are stored as trees, not linear history:
- Navigate to any previous point with `/tree`
- Branch from any message
- All branches live in a single file
- Export to HTML or share via GitHub gists

## Provider Agnostic

15+ providers, 100+ models:
- Anthropic, OpenAI, Google, Azure, AWS
- Mistral, Groq, Cerebras, xAI, Hugging Face
- OpenRouter, MiniMax, Perplexity, DeepSeek

Switch models mid-session. Add custom providers via configuration.

## Steering & Follow-up

Submit messages while the agent works:
- **Enter** — Steering message (interrupts after current tool)
- **Alt+Enter** — Follow-up (waits until completion)

## Installation & Sharing

Bundle extensions, skills, prompts, and themes as packages:

```bash
# Install from git
pi install git:github.com/user/pi-tools

# Install with version
pi install git:github.com/user/pi-tools@v1.2.0

# Test without installing
pi -e git:github.com/user/pi-tools
```

## What We Believe

1. **Tools should adapt to humans**, not the other way around
2. **Composition beats configuration** — primitives can be combined infinitely
3. **Code is context** — your project files are the best prompt
4. **Transparency matters** — see what the AI sees
5. **Speed matters** — sub-50ms startup, native performance
6. **Terminal is king** — the best IDE is the one you already use

## Comparison

| Feature | Others | Pi |
|---------|--------|-----|
| Sub-agents | Baked-in | Build your own |
| Plan mode | Built-in | Extension |
| Permissions | Popups | Your choice |
| To-dos | Built-in | File-based |
| MCP | Required | Optional |
| Size | 100MB+ | ~15MB |
| Startup | 1-3s | <50ms |

## Read More

- [Blog post](https://pi.dev/blog) — Full rationale
- [Extensions](docs/extensions.md) — Build your own
- [Skills](docs/skills.md) — Capability packages
- [Packages](docs/packages.md) — Share and install

---

**Pi doesn't dictate. It enables.**
