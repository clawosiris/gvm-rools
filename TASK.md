# Code Review Fixes for gvm-rools

Fix the following issues from the code review. Work through them in order.
Commit each fix separately with a conventional commit message referencing the issue number.

## 1. HIGH: XML injection in GMP authentication (refs #11)

File: `src/main.rs` lines 118-121

The `<authenticate>` XML is built by string interpolation without escaping.
Fix: Use `quick_xml::escape::escape()` on both username and password before interpolation.

Add a unit test `test_xml_escape_in_credentials` that verifies `<`, `>`, `&`, `"` are escaped.

## 2. HIGH: Blocking I/O in async context (refs #6)

File: `src/main.rs` lines 94, 100-101

`rpassword::prompt_password()` and `std::io::read_to_string(stdin())` block the tokio runtime.
Fix: Wrap both in `tokio::task::spawn_blocking()`.

## 3. MEDIUM: Remove unused dependencies (refs #8)

Remove these from Cargo.toml (workspace and/or crate level):
- `thiserror` (declared but never imported)
- `gvm-client` and `gvm-gmp` (workspace deps not used by gvm-cli)
- `predicates` (dev-dep not imported in tests)

Run `cargo build && cargo test` to verify nothing breaks.

## 4. MEDIUM: SECURITY.md falsely claims deny(unsafe_code) (refs #9)

Option A (preferred): Replace raw libc termios calls with the `nix` crate's safe wrappers, then add `#![deny(unsafe_code)]` to `src/main.rs`.
Option B: If Option A is too complex, update SECURITY.md to accurately describe the unsafe usage.

## 5. MEDIUM: Credential strings not zeroized (refs #12)

Add `zeroize` crate. Wrap password `String` values in `Zeroizing<String>` in `resolve_gmp_password()` and anywhere SSH passwords are held.

## 6. MEDIUM: SSH --password visible in process list (refs #13)

Add `SSH_PASSWORD` env var support: `#[arg(long, env = "SSH_PASSWORD")]`
Add TTY prompt fallback for SSH password (similar to GMP password flow).

## 7. MEDIUM: test_non_2xx_raw_mode correctness (refs #14)

Fix `tests/cli_integration.rs:241-253`: Assert the response body content, not just exit code, to confirm a non-success GMP status was actually received.

## 8. LOW batch (refs #10)

- Fix `Tls {}` → `Tls` (unit variant)
- Remove redundant `default_value_t = false` on bool args
- Make UTF-8 handling consistent in `format_xml` (error on invalid in both paths)
- Add missing tests: XML special chars in credentials, timeout edge cases (-1, 0), GMP_PASSWORD env var, malformed XML in format_xml

## Rules
- Run `cargo clippy --workspace -- -D warnings` after all changes
- Run `cargo test --workspace` after all changes
- Each fix gets its own commit with message format: `fix: <description> (refs #N)`
