# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Build & Run

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run -- --id <ID> --server <SERVER> --key <KEY> --password <PW>
./target/debug/rustshell --help
```

## Architecture

`src/main.rs` (~550 lines) — single-file binary crate. No library, no workspace.

### Dependency tree

```
rustshell
├── hbb_common (git: rustdesk/hbb_common)  ← protobuf, crypto, networking, config
│   └── sodiumoxide (NaCl: box_, secretbox, sign)
├── crossterm  ← cross-platform raw terminal I/O
├── clap       ← CLI argument parsing
├── sha2       ← SHA-256 for password hashing
├── base64     ← key decoding
├── uuid       ← terminal service_id generation
├── rpassword  ← interactive password prompt
├── anyhow     ← error handling
└── tokio      ← async runtime
```

### Connection flow (`run()` function)

```
Phase 1: Rendezvous
  connect_tcp(server:port)
  → attempt_secure_tcp()          // optional KeyExchange with rendezvous server
  → send PunchHoleRequest         // { id, key, conn_type: TERMINAL, force_relay: false }
  → recv PunchHoleResponse or RelayResponse

Phase 2: Connect
  if PunchHoleResponse with peer address:
    → try direct TCP to peer      // faster, no relay overhead
    → on failure: fall back to relay
  else (RelayResponse or no direct addr):
    → connect_tcp(relay_server:21117)
    → send RequestRelay           // { id, uuid, key, conn_type: TERMINAL }

Phase 3: E2E Key Exchange
  recv SignedId
  → verify RelayResponse.pk with rendezvous key → get peer signing key
  → verify SignedId with peer signing key → get peer box public key
  → send PublicKey { asymmetric, symmetric } = NaCl box key exchange
  → conn.set_key() — all subsequent traffic encrypted with secretbox

Phase 4: Authentication
  recv Hash { salt, challenge }
  → compute SHA256(SHA256(password + salt) + challenge)
  → send LoginRequest { password: hash, terminal: { service_id } }
  → recv LoginResponse

Phase 5: Terminal I/O
  → send OpenTerminal { rows, cols }
  → recv TerminalResponse::Opened
  → inject locale fix (export LANG / chcp 65001)
  → bidirectional loop: tokio::select! { conn.next(), poll_key_event(), keepalive }
```

### Key design decisions

- **No lib dependence**: Does NOT depend on `librustdesk` — all connection logic (secure_tcp, key exchange, auth) is reimplemented in `main.rs` using only `hbb_common` types. This avoids pulling in `scrap` (screen capture) and other heavy desktop dependencies.
- **Direct-first with relay fallback**: Sets `force_relay: false` to let the rendezvous server provide peer address. Tries direct TCP connection first; falls back to relay if direct fails. This matches RustDesk's connection strategy — faster when peers are reachable, reliable relay when they're not.
- **Platform-specific output**: On Windows, `ENABLE_VIRTUAL_TERMINAL_PROCESSING` is set on the console output handle so UTF-8 and VT100 escape sequences work correctly. On Unix, crossterm's raw mode is sufficient.
- **Locale injection**: After the remote PTY starts, the client sends `export LANG=en_US.UTF-8` (macOS/Linux) or `chcp 65001` (Windows) to ensure the shell is in UTF-8 mode. Platform detection based on `PeerInfo.platform` from the login response.

### protobuf message types used

All from `hbb_common`:
- **rendezvous_proto**: `RendezvousMessage`, `PunchHoleRequest`, `RelayResponse`, `RequestRelay`, `KeyExchange`
- **message_proto**: `Message`, `SignedId`, `PublicKey`, `Hash`, `LoginRequest`, `LoginResponse`, `TerminalAction`, `TerminalResponse`, `TerminalData`, `OpenTerminal`, `CloseTerminal`, `ResizeTerminal`, `Terminal`

### Crypto primitives

All via `hbb_common::sodiumoxide`:
- `sign::verify()` — Ed25519 signature verification
- `box_::gen_keypair()` + `box_::seal()` — NaCl box key exchange
- `secretbox::gen_key()` + `secretbox::seal/open()` — symmetric encryption (used by `conn.set_key()`)

### RustDesk compatibility

The binary is compatible with standard RustDesk server infrastructure:
- Works with any `hbbs` (rendezvous server) + `hbbr` (relay server)
- The remote device must have terminal access enabled (`enable-terminal` option)
- Uses the same protobuf protocol as the official RustDesk client
- Tested with RustDesk server v1.x
