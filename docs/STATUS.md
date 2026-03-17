# Implementation Status

Last updated: 2026-03-17

## Tools

| Tool | Status | Notes |
|------|--------|-------|
| gvm-cli (GMP) | 🚧 In progress | Unix socket + SSH planned; TLS deferred |
| gvm-script | 📋 Planned | Re-evaluate scripting runtime approach |
| gvm-pyshell | 📋 Planned | Interactive shell (future) |

## CI

- CI: fmt + clippy + tests + MSRV
- Security: cargo-audit + cargo-machete
- Nightly/Release: SBOM generation + sbomqs quality gate
