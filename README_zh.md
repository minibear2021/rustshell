# rustshell

[English](README.md)

跨平台远程 Shell 客户端。通过 RustDesk 中继基础设施连接任意运行
RustDesk 的设备，并打开远程终端会话。

支持 **Windows**、**macOS**、**Linux**。

## 快速开始

```bash
# 编译
cargo build --release

# 连接远程设备
./target/release/rustshell \
  --id <设备ID> \
  --server <中继服务器地址> \
  --key <许可证密钥> \
  --password <设备密码>
```

## 用法

```
rustshell [OPTIONS] --id <ID> --server <SERVER>

选项:
  -i, --id <ID>              远程设备 ID
  -s, --server <SERVER>      ID 服务器地址 (host:port 或 IP)
  -p, --port <PORT>          ID 服务器端口 [默认: 21116]
  -k, --key <KEY>            许可证密钥 [默认: 内置公钥]
  -w, --password <PASSWORD>  设备密码 (留空则交互式输入)
  -d, --debug                启用调试日志
  -h, --help                 打印帮助
```

## 环境变量

所有 CLI 参数也可通过环境变量设置（前缀 `RUSTSHELL_`）。
CLI 参数优先级高于环境变量。

| 变量 | CLI 参数 | 说明 |
|------|----------|------|
| `RUSTSHELL_ID` | `--id` | 远程设备 ID |
| `RUSTSHELL_SERVER` | `--server` | ID 服务器地址 |
| `RUSTSHELL_PORT` | `--port` | ID 服务器端口 |
| `RUSTSHELL_KEY` | `--key` | 许可证密钥 |
| `RUSTSHELL_PASSWORD` | `--password` | 设备密码 |
| `RUSTSHELL_DEBUG` | `--debug` | 设为 `1` 或 `true` |

```bash
# 全部通过环境变量配置
export RUSTSHELL_ID=123456789
export RUSTSHELL_SERVER=myserver.example.com
export RUSTSHELL_KEY="MyKeyBase64..."
export RUSTSHELL_PASSWORD="mypassword"
rustshell

# 环境变量 + CLI 参数覆盖
RUSTSHELL_ID=123456789 RUSTSHELL_SERVER=myserver.example.com \
  rustshell -k "MyKey..." -w mypassword
```

## 示例

```bash
# 自建服务器 + 自定义密钥
rustshell -i 123456789 -s myserver.example.com -k "MyKeyBase64..." -w mypassword

# 自定义端口
rustshell -i 123456789 -s 192.168.1.100 -p 61116 -k "MyKey..." -w mypassword

# 交互式密码输入（更安全，密码不出现在命令行）
rustshell -i 123456789 -s myserver.example.com -k "MyKey..."

# 调试模式
rustshell -i 123456789 -s myserver.example.com -k "MyKey..." -w mypassword -d
```

## 工作原理

```
rustshell                         RustDesk 基础设施                 远程设备
    │                                    │                            │
    ├── TCP 连接 ────────────────────► ID 服务器 (:21116)              │
    │   PunchHoleRequest{id, key}        │                            │
    │   ◄── PunchHoleResponse ──────────┤                            │
    │   {peer_addr, relay_fallback}      │                            │
    │                                    │                            │
    ├── 直连 TCP ────────────────(尝试)──┼────────────────────────►   │
    │   (失败则降级)                                    │             │
    │   ─── 中继 TCP ────────────────► 中继 (:21117)    │             │
    │       RequestRelay{id, uuid}      │               │             │
    │                                    ├── 桥接 ─────►│             │
    │                                    │                            │
    │   ◄══ 端到端加密通道 ════════════════════════════════════════   │
    │   ◄── SignedId ────────────────────────────────────────────    │
    │   ──── PublicKey (NaCl 密钥交换) ─────────────────────────►    │
    │   ◄── Hash 质询 ──────────────────────────────────────────    │
    │   ──── LoginRequest{terminal} ────────────────────────────►    │
    │   ◄══ 终端 I/O (stdin/stdout) ══════════════════════════════   │
    │                                                                 │
    ▼                                                                 ▼
 本地终端                                                         远程 Shell
 (raw mode)                                                  (bash/zsh/sh)
```

1. **信令**：连接 ID 服务器，请求连接到目标设备
2. **中继**：ID 服务器分配中继服务器；双方连接到中继
3. **密钥交换**：基于 NaCl 的端到端加密 (Curve25519 + XSalsa20-Poly1305)
4. **认证**：SHA-256 质询-响应，使用设备密码
5. **终端**：在远端打开 PTY，本地进入 raw 模式，双向 I/O

## 环境要求

- Rust 1.75+
- 运行中的 [RustDesk 服务端](https://github.com/rustdesk/rustdesk-server) (hbbs + hbbr)
- 目标设备上运行 RustDesk，且已开启终端访问权限

## 快捷键

| 按键 | 操作 |
|------|------|
| Ctrl+C | 关闭终端并退出 |
| Ctrl+D | 关闭终端并退出 |

## 故障排除

**连接立即断开：**
- 确认远程设备 ID 正确且设备在线
- 检查 ID 服务器地址和端口是否正确
- 确认许可证密钥与服务器配置一致

**中文/CJK 字符显示为乱码：**
- 远端 Shell 的 locale 可能未设置为 UTF-8
- rustshell 连接后会打印相应的修复命令提示
- macOS/Linux：复制并执行 `export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`
- Windows：复制并执行 `chcp 65001`

**Windows 远端输入 `exit` 后连接挂起：**
- 这是 RustDesk 服务端的[已知 bug](https://github.com/rustdesk/rustdesk/blob/caadd72ab2db8cc66e3d237e3e1cb60edbab7bc5/src/server/terminal_service.rs#L1267-L1270)：Windows ConPTY 在子进程退出时不发送 EOF 信号，导致服务端无法检测到会话已结束
- **变通方案**：用 Ctrl+C 或 Ctrl+D 替代 `exit` 来关闭会话。这两种操作会发送显式的 `CloseTerminal` 消息，服务端能正确处理
- 此问题仅影响 Windows 远端；macOS 和 Linux 远端使用 `exit` 正常工作

**空闲时连接断开：**
- 每 15 秒发送一次心跳保活；中继或服务端的超时可能更短
- 检查中继服务器的超时配置

## 许可证

AGPL-3.0
