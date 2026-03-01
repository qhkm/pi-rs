# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability in pi-rs, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### How to Report

1. Email **pi-rs@proton.me** with:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if any)

2. You will receive an acknowledgment within **48 hours**.

3. We will work with you to understand and address the issue before any public disclosure.

### What to Expect

- **Acknowledgment**: Within 48 hours of your report
- **Assessment**: We will assess the severity and impact within 7 days
- **Fix**: Critical issues will be patched as quickly as possible
- **Disclosure**: We will coordinate public disclosure with you after a fix is available

### Scope

The following are in scope:
- Code execution vulnerabilities in tool execution (Bash, WASM, binary plugins)
- Path traversal in file operations (Read, Write, Edit, Find)
- Token/credential exposure
- Session data leakage
- Extension system sandbox escapes

The following are out of scope:
- Vulnerabilities in third-party dependencies (report to upstream)
- Social engineering attacks
- Denial of service through normal usage

## Security Design

pi-rs includes several security measures:

- **Path traversal protection**: All file operations validate paths against directory traversal attacks
- **WASM sandboxing**: WebAssembly extensions run with memory limits, fuel consumption, and wall-clock timeouts
- **Tool approval gates**: Dangerous operations require explicit approval
- **Encrypted token storage**: OAuth tokens are encrypted at rest
- **Input validation**: All external inputs are validated before processing

## Thank You

We appreciate the security research community's efforts in helping keep pi-rs and its users safe.
