# RustShell

[中文文档](README_zh.md)

Cross-platform remote shell client. Connects to any device running RustDesk
and opens a remote terminal session via the RustDesk relay infrastructure.

Works on **Windows**, **macOS**, and **Linux**.

## Quick Start

```bash
# Build
cargo build --release

# Connect to a remote device
./target/release/rustshell \
  --id <DEVICE_ID> \
  --server <RENDEZVOUS_SERVER> \
  --key <LICENCE_KEY> \
  --password <DEVICE_PASSWORD>
```

## Usage

```
rustshell [OPTIONS] --id <ID> --server <SERVER>

Options:
  -i, --id <ID>              Remote device ID (required)
  -s, --server <SERVER>      Rendezvous server host:port or IP (required)
  -p, --port <PORT>          Rendezvous server port [default: 21116]
  -k, --key <KEY>            Licence key [default: built-in public key]
  -w, --password <PASSWORD>  Device password (omit for interactive prompt)
  -d, --debug                Enable debug logging
  -h, --help                 Print help
```

## Environment Variables

All CLI arguments can also be set via environment variables (prefixed with `RUSTSHELL_`).
CLI arguments take precedence when both are provided.

| Variable | CLI flag | Description |
|----------|----------|-------------|
| `RUSTSHELL_ID` | `--id` | Remote device ID |
| `RUSTSHELL_SERVER` | `--server` | Rendezvous server address |
| `RUSTSHELL_PORT` | `--port` | Rendezvous server port |
| `RUSTSHELL_KEY` | `--key` | Licence key |
| `RUSTSHELL_PASSWORD` | `--password` | Device password |
| `RUSTSHELL_DEBUG` | `--debug` | Set to `1` or `true` |

```bash
# All configuration via environment variables
export RUSTSHELL_ID=123456789
export RUSTSHELL_SERVER=myserver.example.com
export RUSTSHELL_KEY="MyKeyBase64..."
export RUSTSHELL_PASSWORD="mypassword"
rustshell

# Override specific values with CLI flags
RUSTSHELL_ID=123456789 RUSTSHELL_SERVER=myserver.example.com \
  rustshell -k "MyKey..." -w mypassword
```

## Examples

```bash
# Self-hosted server with custom key
rustshell -i 123456789 -s myserver.example.com -k "MyKeyBase64..." -w mypassword

# Custom port
rustshell -i 123456789 -s 192.168.1.100 -p 61116 -k "MyKey..." -w mypassword

# Interactive password prompt (more secure)
rustshell -i 123456789 -s myserver.example.com -k "MyKey..."

# Debug mode for troubleshooting
rustshell -i 123456789 -s myserver.example.com -k "MyKey..." -w mypassword -d
```

## How It Works

```
rustshell                         RustDesk infrastructure              Remote device
    │                                    │                                  │
    ├── TCP connect ──────────────────► rendezvous server (:21116)          │
    │   PunchHoleRequest{id, key}        │                                  │
    │   ◄── PunchHoleResponse ──────────┤                                  │
    │   {peer_addr, relay_fallback}      │                                  │
    │                                    │                                  │
    ├── direct TCP ────────────────(try)──┼────────────────────────────►   │
    │   (fallback on failure)                                 │             │
    │   ─── relay TCP ────────────────► relay (:21117)       │             │
    │       RequestRelay{id, uuid}      │                     │             │
    │                                    ├── bridge ────────►│             │
    │                                    │                                    │
    │   ◄══ E2E encrypted channel ═══════════════════════════════════════   │
    │   ◄── SignedId ───────────────────────────────────────────────────   │
    │   ──── PublicKey (NaCl key exchange) ───────────────────────────►   │
    │   ◄── Hash challenge ────────────────────────────────────────────   │
    │   ──── LoginRequest{terminal} ──────────────────────────────────►   │
    │   ◄══ Terminal I/O (stdin/stdout) ═══════════════════════════════   │
    │                                                                      │
    ▼                                                                      ▼
local terminal                                                     remote shell
(raw mode)                                                   (bash/zsh/PowerShell)
```

1. **Rendezvous**: Connects to the ID server, requests connection to target device
2. **Relay**: ID server assigns a relay server; both sides connect to it
3. **Key exchange**: NaCl-based E2E encryption (Curve25519 + XSalsa20-Poly1305)
4. **Authentication**: SHA-256 challenge-response with the device password
5. **Terminal**: Opens a PTY on the remote, enters raw mode locally, bi-directional I/O

## Requirements

- Rust 1.75+
- A running [RustDesk server](https://github.com/rustdesk/rustdesk-server) (hbbs + hbbr)
- RustDesk running on the target device with terminal access enabled

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Ctrl+C | Close terminal and exit |
| Ctrl+D | Close terminal and exit |

## Troubleshooting

**Connection closed immediately:**
- Verify the remote device ID is correct and the device is online
- Check that the rendezvous server address and port are correct
- Ensure the licence key matches the server configuration

**Chinese/CJK characters display as garbled text:**
- The remote shell's locale may not be set to UTF-8
- RustShell prints a hint with the appropriate fix command after connecting
- macOS/Linux: copy and run `export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`
- Windows: copy and run `chcp 65001`

**Connection hangs after typing `exit` on Windows remote:**
- This is a [known bug](https://github.com/rustdesk/rustdesk/blob/caadd72ab2db8cc66e3d237e3e1cb60edbab7bc5/src/server/terminal_service.rs#L1267-L1270) in the RustDesk server: Windows ConPTY does not signal EOF when the shell exits, so the server never detects the session has ended
- **Workaround**: use Ctrl+C or Ctrl+D to close the session instead of typing `exit`. These send an explicit `CloseTerminal` message that the server handles correctly
- This issue only affects Windows remotes; macOS and Linux remotes work correctly with `exit`

**Connection drops after idle:**
- A keepalive heartbeat is sent every 15 seconds; the relay or server may have a shorter timeout
- Check the relay server's timeout configuration

## License

AGPL-3.0
