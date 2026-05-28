# rustshell

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
    │   ◄── RelayResponse{pk, uuid} ────┤                                  │
    │                                    │                                  │
    ├── TCP connect ──────────────────► relay server (:21117)               │
    │   RequestRelay{id, uuid}          │                                  │
    │                                    ├── bridge ───────────────────►   │
    │   ◄── SignedId ───────────────────────────────────────────────────   │
    │   ──── PublicKey ───────────────────────────────────────────────►   │
    │   ◄── Hash challenge ────────────────────────────────────────────   │
    │   ──── LoginRequest{terminal} ──────────────────────────────────►   │
    │   ◄── Terminal I/O (stdin/stdout) ──────────────────────────────   │
    │                                                                      │
    ▼                                                                      ▼
local terminal                                                     remote shell
(raw mode)                                                        (bash/zsh/sh)
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
- rustshell automatically injects `export LANG=en_US.UTF-8` on macOS/Linux and `chcp 65001` on Windows
- If you still see issues, manually run `export LANG=en_US.UTF-8` in the remote shell

**Connection drops after idle:**
- A keepalive heartbeat is sent every 15 seconds; the relay or server may have a shorter timeout
- Check the relay server's timeout configuration

## License

AGPL-3.0
