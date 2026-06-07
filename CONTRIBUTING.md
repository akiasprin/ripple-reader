# Contributing to ripple-reader

Thank you for your interest in contributing! This document outlines how to report issues, propose features, and submit changes.

## Bug Reports

Before opening a bug report, please:

1. Search existing issues to avoid duplicates
2. Include:
   - A clear description of the bug
   - Steps to reproduce
   - Expected vs. actual behavior
   - Your environment (OS, Rust version, PostgreSQL version)
   - Relevant logs or error messages

## Feature Requests

Feature requests are welcome! Please open an issue with:

- A clear description of the feature
- The motivation / use case
- Any ideas for implementation

## Pull Requests

### Workflow

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes
4. Run the checks below
5. Commit with a clear message
6. Open a pull request

### Code Style

All code must pass:

```bash
# Formatting
cargo fmt --check

# Linting
cargo clippy -- -D warnings

# Tests
cargo test

# Build with no warnings
cargo build
```

### Commit Messages

- Use the present tense ("Add feature" not "Added feature")
- Use the imperative mood ("Move cursor to..." not "Moves cursor to...")
- Reference issues where applicable (`Fixes #123`)

## Development Setup

See [README.md](README.md) for the full local development setup.

## License

By contributing, you agree that your contributions will be licensed under the same terms as the project (MIT OR Apache-2.0).
