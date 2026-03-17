# gvm-rools — gvm-cli OpenSpec

## 1. Overview

This spec defines **`gvm-cli`**, a Rust reimplementation of `greenbone/gvm-tools` `gvm-cli`, built on top of the `rust-gvm` crates.

**Goal:** Provide a small, reliable command-line tool to send GMP XML commands to `gvmd` and print responses.

### 1.1 Goals
- Compatibility-oriented CLI surface with `gvm-tools` `gvm-cli` for the most common workflows
- Transport support (in priority order):
  1. Unix socket
  2. SSH (direct-streamlocal)
  3. TLS (deferred until rust-gvm TLS transport lands)
- Ability to send raw XML via:
  - `--xml "<get_version/>"`
  - a file argument
  - stdin
- Output modes:
  - default: treat non-2xx as error (exit 1) but still print response
  - `--raw`: never treat response as error
- Optional authentication (send `<authenticate .../>` first)
- Deterministic behavior suitable for scripting/automation

### 1.2 Non-goals (MVP)
- OSP support (rust-gvm is GMP-focused)
- An interactive shell (that is `gvm-pyshell` scope)
- A full scripting runtime (that is `gvm-script` scope)
- Pretty-printing XML beyond basic "as-is" output (may be added)

### 1.3 Success Criteria
- Equivalent to gvm-tools `gvm-cli` for: connect → authenticate → send command → print response
- Works against `gvm-mock-server` in CI integration tests
- Works in a developer environment against a real gvmd over unix socket

---

## 2. CLI Interface

### 2.1 Command shape

```
gvm-cli [GLOBAL_OPTS] <transport> [TRANSPORT_OPTS] [--xml <xml> | <infile>]
```

### 2.2 Global options
- `--gmp-username <user>` (env: `GMP_USERNAME`)
- `--gmp-password <pass>` (env: `GMP_PASSWORD`)
  - if username is set and password is missing, prompt from TTY (planned)
- `-X, --xml <xml>`
- `-r, --raw`
- `--pretty` (optional)
- `--duration`

### 2.3 Transport subcommands

#### socket
- `gvm-cli socket --path /run/gvmd/gvmd.sock [--timeout <seconds|-1>]`

#### ssh
- `gvm-cli ssh --hostname <host> [--port 22] [--username gvm] [--password <pw>|(agent)] [--remote-socket /run/gvmd/gvmd.sock]`

#### tls
- `gvm-cli tls ...` (stub until rust-gvm TLS is implemented)

---

## 3. Architecture

### 3.1 Components
- `gvm-rools-cli` (this repo): CLI argument parsing, command I/O, printing
- `gvm-connection` (rust-gvm): Unix socket + SSH transports, framed reads
- `gvm-protocol` (rust-gvm): response parsing helpers (`Response` status_code/status_text)

### 3.2 Request/Response model
- Send exactly the XML bytes provided by the user
- Read until a full XML response frame is returned by `GvmConnection::read()`
- For `--raw=false`:
  - treat non-2xx as error: exit code 1
  - print response to stdout for debugging

---

## 4. Testing Strategy

### 4.1 Unit tests
- Arg parsing and configuration mapping
- XML input selection precedence: `--xml` > file arg > stdin

### 4.2 Integration tests (CI)
Use `gvm-mock-server` in `Stateful` mode:
- `gvm-cli socket -X '<get_version/>'` returns 200 and contains `<version>22.5</version>`
- `gvm-cli socket --gmp-username admin --gmp-password admin -X '<get_tasks/>'`:
  - sends authenticate first
  - returns 200
- non-2xx behavior:
  - default exits 1
  - `--raw` exits 0 (or exits 0 but prints raw) — define expected behavior

### 4.3 Manual E2E (not CI)
- Reuse the proposed GVM Community container harness (rust-gvm issue #22) to validate against real gvmd.

---

## 5. Implementation Phases

### Phase 1 — MVP (unix socket)
- socket transport
- xml/file/stdin input
- default/raw modes
- duration measurement

### Phase 2 — SSH
- implement ssh transport CLI + docs
- add ssh integration tests with mock server SSH listener (feature-gated)

### Phase 3 — TLS (blocked)
- implement tls transport after rust-gvm TLS lands

---

## 6. Open Questions
- Should `--raw` suppress non-2xx exit codes (match gvm-tools) or always exit 0?
- Pretty-printing: adopt a lightweight XML formatter or keep as-is?
