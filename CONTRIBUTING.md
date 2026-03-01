# Contributing to π (pi)

Thank you for your interest in contributing to pi! This document provides guidelines and instructions for contributing.

## Code of Conduct

Be respectful, constructive, and inclusive in all interactions.

## How to Contribute

### Reporting Bugs

1. Check if the issue already exists
2. Create a new issue with:
   - Clear title and description
   - Steps to reproduce
   - Expected vs actual behavior
   - Environment details (OS, Rust version, etc.)

### Suggesting Features

1. Open a discussion first for major features
2. Describe the use case and proposed solution
3. Be open to feedback and iteration

### Pull Requests

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Add tests for new functionality
5. Ensure all tests pass (`cargo test --workspace`)
6. Run formatting (`cargo fmt`)
7. Run linting (`cargo clippy --workspace`)
8. Commit with clear messages
9. Push and open a PR

## Development Setup

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/pi-rs
cd pi-rs

# Build
cargo build

# Run tests
cargo test --workspace

# Run the CLI
cargo run --bin pi
```

## Project Structure

```
crates/
  pi-ai/           # LLM providers, OAuth, models
  pi-agent-core/   # Agent loop, tools, context
  pi-tui/          # Terminal UI framework
  pi-coding-agent/ # CLI, modes, extensions
  pi-mom/          # Slack bot (optional)
  pi-pods/         # GPU pods (optional)
  pi-web-ui/       # Web components (optional)
```

## Coding Standards

### Rust Style

- Follow standard Rust conventions
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Document public APIs with doc comments
- Write tests for new functionality

### Commit Messages

Use conventional commits:
```
feat: add new feature
fix: fix bug
docs: update documentation
refactor: refactor code
test: add tests
chore: maintenance tasks
```

### Testing

- Unit tests in `mod tests` blocks
- Integration tests in `tests/` directories
- Aim for >80% coverage for new code

## Adding a New LLM Provider

1. Add provider to `crates/pi-ai/src/providers/`
2. Implement `LLMProvider` trait
3. Add model definitions to registry
4. Add tests
5. Update documentation

## Adding a New Tool

1. Implement `AgentTool` trait in `crates/pi-agent-core/src/tools/`
2. Add parameter schema
3. Handle abort signals properly
4. Add tests
5. Register in tool registry

## Adding a TUI Component

1. Implement `Component` trait in `crates/pi-tui/src/components/`
2. Handle input events
3. Implement proper rendering
4. Add theme support
5. Export in `lib.rs`

## Review Process

1. All PRs require at least one review
2. CI checks must pass
3. Address review feedback
4. Maintainers will merge when ready

## Release Process

1. Version bumps follow semver
2. Update CHANGELOG.md
3. Tag releases
4. CI handles publishing

## Questions?

- Open an issue for questions
- Join discussions
- Check existing documentation

Thank you for contributing!
