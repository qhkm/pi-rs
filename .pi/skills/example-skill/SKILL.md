---
name: example-skill
description: An example skill demonstrating the SKILL.md format
version: 1.0.0
author: pi contributors
tags:
  - example
  - documentation
  - rust
---

# Example Skill

This is an example skill file. Skills allow you to provide custom instructions
to the AI agent for specific tasks or coding styles.

## When to Use This Skill

Use this skill when you want to see how the SKILL.md format works.

## Guidelines

1. **Keep it concise**: Skills should be focused and to the point
2. **Be specific**: Provide concrete examples and patterns
3. **Update regularly**: Keep skills current with your evolving needs

## Example Patterns

```rust
// Good: Clear, idiomatic Rust
fn calculate_sum(numbers: &[i32]) -> i32 {
    numbers.iter().sum()
}

// Avoid: Unnecessary complexity
fn calculate_sum_bad(numbers: &[i32]) -> i32 {
    let mut sum = 0;
    for i in 0..numbers.len() {
        sum = sum + numbers[i];
    }
    sum
}
```

## Resources

- Place this file in `.pi/skills/example-skill/SKILL.md`
- Activate with: `pi /skill:enable example-skill`
