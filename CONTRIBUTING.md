# Contributing to Astapa

Thanks for your interest in contributing!

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/astapa.git`
3. Create a branch: `git checkout -b my-feature`
4. Make your changes
5. Run tests: `cargo test`
6. Format code: `cargo fmt`
7. Check lints: `cargo clippy`
8. Commit and push
9. Open a Pull Request

## Development

```bash
# Setup
cp .env.example .env
# Edit .env with your database URL

# Run locally
cargo run

# Run tests
cargo test

# Format + lint
cargo fmt && cargo clippy
```

## Pull Request Guidelines

- Keep PRs focused on a single change
- Include tests for new functionality
- Update documentation if needed
- Follow the existing code style

## Reporting Issues

- Check existing issues first
- Include steps to reproduce
- Include Rust version (`rustc --version`)
- Include relevant logs

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
