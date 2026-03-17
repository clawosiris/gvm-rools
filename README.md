# gvm-rools

Rust reimplementation of [`greenbone/gvm-tools`](https://github.com/greenbone/gvm-tools), built on top of the **rust-gvm** library crates.

## Scope (initial)

We are focusing first on **`gvm-cli`** (GMP only):
- Send raw GMP XML commands to `gvmd`
- Unix socket + SSH transports (TLS deferred until rust-gvm TLS exists)
- Script-friendly exit codes and output

See spec: [`spec/gvm-cli/openspec.md`](spec/gvm-cli/openspec.md)

## Usage (MVP)

```bash
# Unix socket
cargo run -p gvm-rools-cli -- socket -X '<get_version/>'

# Read from file
cargo run -p gvm-rools-cli -- socket my-command.xml

# SSH (agent auth)
cargo run -p gvm-rools-cli -- ssh --hostname 127.0.0.1 -X '<get_version/>'
```

## Security & SBOM

This repo mirrors the rust-gvm security posture:
- `SECURITY.md` for vulnerability reporting
- SBOM generation (CycloneDX 1.5) on nightly + release
- `sbomqs` quality gate (≥ 7.0) with post-processing to inject CC0 data license, build lifecycle metadata, and supplier hints

## License

AGPL-3.0-or-later
