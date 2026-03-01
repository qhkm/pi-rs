# .pi Directory

This directory contains configuration and extensions for the pi AI coding agent.

## Directory Structure

```
.pi/
├── settings.json           # Project-specific settings
├── SYSTEM.md              # System prompt for this project
├── AGENTS.md              # Project-specific agent instructions
├── skills/                # Custom skills
│   └── my-skill/
│       └── SKILL.md
└── extensions/            # Custom extensions
    └── my-extension/
        └── extension.json
```

## Files

### settings.json
Project-specific configuration. See the example file for available options.

### SYSTEM.md
Custom system prompt that provides context to the AI about this project.

### AGENTS.md
Additional agent instructions specific to this codebase.

## User vs Project Config

- **User config**: `~/.pi/agent/settings.json` (applies to all projects)
- **Project config**: `.pi/settings.json` (applies to this project only)

Project config overrides user config.

## Skills

Place skill directories in `.pi/skills/`:

```bash
.pi/skills/my-skill/SKILL.md
```

Activate with: `pi /skill:enable my-skill`

## Extensions

Place extension directories in `.pi/extensions/`:

```bash
.pi/extensions/my-extension/extension.json
```

Extensions are automatically discovered and loaded.

## See Also

- [Documentation](../README.md)
- [Contributing Guide](../CONTRIBUTING.md)
