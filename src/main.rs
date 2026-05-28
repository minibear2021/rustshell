use anyhow::{bail, Context, Result};
use clap::Parser;
use hbb_common::{
    bytes,
    config::{CONNECT_TIMEOUT, RELAY_PORT, RS_PUB_KEY},
    log,
    message_proto::*,
    protobuf::Message as ProtoMessage,
    rendezvous_proto::{
        ConnType, KeyExchange, NatType, PunchHoleRequest, RequestRelay, RendezvousMessage,
    },
    socket_client,
    sodiumoxide::crypto::{box_, secretbox, sign},
    tokio::{self, time},
    Stream,
};
use sha2::{Digest, Sha256};
use std::io::Write;

const APP_NAME: &str = "rustshell";
const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── CLI arguments ──────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = APP_NAME, about = "Cross-platform remote shell via RustDesk")]
struct Args {
    #[arg(short = 'i', long)] id: String,
    #[arg(short = 's', long)] server: String,
    #[arg(short = 'p', long, default_value = "21116")] port: u16,
    #[arg(short = 'k', long, default_value = "")] key: String,
    #[arg(short = 'w', long, default_value = "")] password: String,
    #[arg(short = 'd', long, default_value = "false")] debug: bool,
}

// ── Crypto helpers ─────────────────────────────────────────────────

fn get_pk(pk: &[u8]) -> Option<[u8; 32]> {
    if pk.len() == 32 {
        let mut tmp = [0u8; 32];
        tmp[..].copy_from_slice(pk);
        Some(tmp)
    } else { None }
}

fn get_rs_pk(str_base64: &str) -> Option<sign::PublicKey> {
    use base64::Engine;
    get_pk(&base64::engine::general_purpose::STANDARD.decode(str_base64).ok()?).map(sign::PublicKey)
}

fn decode_id_pk(signed: &[u8], key: &sign::PublicKey) -> Result<(String, [u8; 32])> {
    let raw = sign::verify(signed, key).map_err(|_| anyhow::anyhow!("Signature mismatch"))?;
    let id_pk = IdPk::parse_from_bytes(&raw)?;
    let pk = get_pk(&id_pk.pk).ok_or_else(|| anyhow::anyhow!("Wrong public key length"))?;
    Ok((id_pk.id, pk))
}

fn create_symmetric_key_msg(their_pk_b: [u8; 32]) -> (Vec<u8>, Vec<u8>, secretbox::Key) {
    let their_pk_b = box_::PublicKey(their_pk_b);
    let (our_pk_b, our_sk_b) = box_::gen_keypair();
    let key = secretbox::gen_key();
    let nonce = box_::Nonce([0u8; box_::NONCEBYTES]);
    let sealed_key = box_::seal(&key.0, &nonce, &their_pk_b, &our_sk_b);
    (our_pk_b.0.to_vec(), sealed_key, key)
}

// ── Key event encoding ─────────────────────────────────────────────

use crossterm::event::{KeyCode, KeyModifiers};

fn key_event_to_bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Char(c) => {
            if ctrl {
                let c_lower = c.to_ascii_lowercase();
                if ('a'..='z').contains(&c_lower) { vec![(c_lower as u8) - b'a' + 1] }
                else {
                    match c_lower {
                        '[' => vec![0x1b], ']' => vec![0x1d],
                        '\\' => vec![0x1c], '^' => vec![0x1e],
                        _ => vec![c as u8],
                    }
                }
            } else if alt {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                v.extend_from_slice(s.as_bytes());
                v
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],       KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],          KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'], KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'], KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'], KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'], KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'], KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::F(1) => vec![0x1b, b'O', b'P'], KeyCode::F(2) => vec![0x1b, b'O', b'Q'],
        KeyCode::F(3) => vec![0x1b, b'O', b'R'], KeyCode::F(4) => vec![0x1b, b'O', b'S'],
        KeyCode::F(5) => vec![0x1b, b'[', b'1', b'5', b'~'], KeyCode::F(6) => vec![0x1b, b'[', b'1', b'7', b'~'],
        KeyCode::F(7) => vec![0x1b, b'[', b'1', b'8', b'~'], KeyCode::F(8) => vec![0x1b, b'[', b'1', b'9', b'~'],
        KeyCode::F(9) => vec![0x1b, b'[', b'2', b'0', b'~'], KeyCode::F(10) => vec![0x1b, b'[', b'2', b'1', b'~'],
        KeyCode::F(11) => vec![0x1b, b'[', b'2', b'3', b'~'], KeyCode::F(12) => vec![0x1b, b'[', b'2', b'4', b'~'],
        _ => vec![],
    }
}

// ── Windows console helpers ────────────────────────────────────────

#[cfg(windows)]
mod win_console {
    extern "system" {
        pub fn GetStdHandle(nStdHandle: u32) -> isize;
        pub fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: isize, mode: u32) -> i32;
        pub fn SetConsoleCP(code_page: u32) -> i32;
        pub fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
    pub const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
    pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    pub const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;
}

/// Write bytes to stdout.
fn write_stdout(data: &[u8]) {
    let mut stdout = std::io::stdout();
    stdout.write_all(data).ok();
    stdout.flush().ok();
}

// ── Terminal setup ─────────────────────────────────────────────────

struct ConsoleGuard;
impl ConsoleGuard {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()
            .context("Failed to enable raw mode")?;
        // On Windows, enable VT100 processing on output.
        // This lets WriteFile (stdout) handle UTF-8 + escape sequences
        // natively, matching Unix terminal behavior.
        #[cfg(windows)]
        unsafe {
            let handle = win_console::GetStdHandle(win_console::STD_OUTPUT_HANDLE);
            let mut mode: u32 = 0;
            if win_console::GetConsoleMode(handle, &mut mode) != 0 {
                win_console::SetConsoleMode(handle, mode
                    | win_console::ENABLE_VIRTUAL_TERMINAL_PROCESSING
                    | win_console::DISABLE_NEWLINE_AUTO_RETURN);
            }
        }
        Ok(Self)
    }
}
impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Poll keyboard input (cross-platform, uses crossterm).
fn poll_key_event() -> Option<Vec<u8>> {
    use crossterm::event::{self, Event, KeyEventKind};
    if !event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
        return None;
    }
    match event::read() {
        Ok(Event::Key(key_event)) if key_event.kind != KeyEventKind::Release => {
            Some(key_event_to_bytes(key_event.code, key_event.modifiers))
        }
        _ => None,
    }
}

// ── Stream helpers ─────────────────────────────────────────────────

async fn recv_raw(conn: &mut Stream, step: &str) -> Result<bytes::BytesMut> {
    match conn.next().await {
        Some(Ok(b)) => { log::debug!("[{step}] received {} bytes", b.len()); Ok(b) }
        Some(Err(e)) => bail!("[{step}] stream error: {e}"),
        None => bail!("[{step}] connection closed by peer"),
    }
}

async fn recv_msg(conn: &mut Stream, step: &str) -> Result<Message> {
    let bytes = recv_raw(conn, step).await?;
    Message::parse_from_bytes(&bytes)
        .with_context(|| format!("[{step}] failed to parse Message"))
}

async fn recv_rendezvous_msg(conn: &mut Stream, step: &str) -> Result<RendezvousMessage> {
    let bytes = recv_raw(conn, step).await?;
    RendezvousMessage::parse_from_bytes(&bytes)
        .with_context(|| format!("[{step}] failed to parse RendezvousMessage"))
}

async fn send_msg(conn: &mut Stream, msg: &impl ProtoMessage, step: &str) -> Result<()> {
    hbb_common::timeout(CONNECT_TIMEOUT, conn.send(msg)).await
        .with_context(|| format!("[{step}] timeout sending message"))??;
    log::debug!("[{step}] sent message");
    Ok(())
}

// ── Main ───────────────────────────────────────────────────────────

fn main() {
    // Windows: set console to UTF-8 codepage
    #[cfg(windows)]
    unsafe {
        win_console::SetConsoleCP(65001);
        win_console::SetConsoleOutputCP(65001);
    }

    let args = Args::parse();
    if args.id.is_empty() { eprintln!("Error: --id is required"); std::process::exit(1); }
    if args.server.is_empty() { eprintln!("Error: --server is required"); std::process::exit(1); }

    let log_level = if args.debug { "debug" } else { "info" };
    hbb_common::env_logger::init_from_env(
        hbb_common::env_logger::Env::default()
            .filter_or(hbb_common::env_logger::DEFAULT_FILTER_ENV, log_level),
    );

    let password = if args.password.is_empty() {
        match rpassword::prompt_password("Enter password: ") {
            Ok(p) => p,
            Err(e) => { eprintln!("Failed to read password: {}", e); std::process::exit(1); }
        }
    } else { args.password };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all().build().expect("tokio runtime");

    if let Err(e) = rt.block_on(run(args.id, args.key, args.server, args.port, password)) {
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

async fn run(
    device_id: String, licence_key: String,
    server: String, port: u16, password: String,
) -> Result<()> {
    let rendezvous_addr = format!("{}:{}", server, port);
    log::info!("Connecting to rendezvous server {}...", rendezvous_addr);

    // Phase 1: Connect to rendezvous server
    let mut socket = socket_client::connect_tcp(rendezvous_addr.clone(), CONNECT_TIMEOUT).await
        .with_context(|| format!("Failed to connect to {}", rendezvous_addr))?;
    log::info!("TCP connected to rendezvous server");

    let key_str: &str = if licence_key.is_empty() { RS_PUB_KEY } else { &licence_key };
    attempt_secure_tcp(&mut socket, key_str).await?;

    // Send PunchHoleRequest
    let mut msg_out = RendezvousMessage::new();
    msg_out.set_punch_hole_request(PunchHoleRequest {
        id: device_id.clone(), licence_key: licence_key.clone(),
        conn_type: ConnType::TERMINAL.into(),
        nat_type: NatType::SYMMETRIC.into(), force_relay: false,
        version: VERSION.to_owned(), ..Default::default()
    });
    log::info!("Requesting connection to device {}...", device_id);
    send_msg(&mut socket, &msg_out, "punch_hole_request").await?;

    // Wait for response
    let rmsg = recv_rendezvous_msg(&mut socket, "wait_rendezvous_response").await?;
    let (peer_pk_from_server, relay_server, relay_uuid, try_direct) = match rmsg.union {
        Some(hbb_common::rendezvous_proto::rendezvous_message::Union::PunchHoleResponse(ph)) => {
            if !ph.socket_addr.is_empty() {
                let addr = hbb_common::AddrMangle::decode(&ph.socket_addr);
                let relay = if ph.relay_server.is_empty() {
                    socket_client::increase_port(&rendezvous_addr, 1)
                } else { socket_client::check_port(ph.relay_server.clone(), RELAY_PORT) };
                log::info!("Peer address: {} (local: {}), relay fallback: {}", addr, ph.is_local(), relay);
                (ph.pk.to_vec(), relay, String::new(), Some(addr))
            } else {
                use hbb_common::rendezvous_proto::punch_hole_response::Failure;
                let reason = match ph.failure.enum_value() {
                    Ok(Failure::ID_NOT_EXIST) => "ID does not exist",
                    Ok(Failure::OFFLINE) => "Remote device is offline",
                    Ok(Failure::LICENSE_MISMATCH) => "Key mismatch",
                    Ok(Failure::LICENSE_OVERUSE) => "Key overuse",
                    _ => &ph.other_failure,
                };
                bail!("Connection refused: {}", reason);
            }
        }
        Some(hbb_common::rendezvous_proto::rendezvous_message::Union::RelayResponse(rr)) => {
            let relay = if rr.relay_server.is_empty() {
                socket_client::increase_port(&rendezvous_addr, 1)
            } else { socket_client::check_port(rr.relay_server, RELAY_PORT) };
            log::info!("Relay assigned: {} (uuid: {})", relay, rr.uuid);
            let pk = match rr.union {
                Some(hbb_common::rendezvous_proto::relay_response::Union::Pk(pk)) => pk.to_vec(),
                _ => Vec::new(),
            };
            (pk, relay, rr.uuid, None)
        }
        other => bail!("Unexpected response: {:?}", other.map(|_| "unknown")),
    };

    // Phase 2: Connect — try direct first, fall back to relay
    let mut conn = if let Some(addr) = try_direct {
        let direct_addr = format!("{}:{}", addr.ip(), addr.port());
        log::info!("Trying direct connection to {}...", direct_addr);
        match socket_client::connect_tcp(direct_addr, CONNECT_TIMEOUT).await {
            Ok(c) => {
                log::info!("Direct connection established");
                c
            }
            Err(e) => {
                log::info!("Direct failed ({}), falling back to relay {}", e, relay_server);
                let mut c = socket_client::connect_tcp(relay_server.clone(), CONNECT_TIMEOUT).await
                    .with_context(|| format!("Failed to connect to relay {}", relay_server))?;
                // Send RequestRelay for relay
                let mut msg_out = RendezvousMessage::new();
                msg_out.set_request_relay(RequestRelay {
                    id: device_id.clone(), uuid: relay_uuid,
                    licence_key: licence_key.clone(),
                    conn_type: ConnType::TERMINAL.into(), ..Default::default()
                });
                send_msg(&mut c, &msg_out, "request_relay").await?;
                c
            }
        }
    } else {
        log::info!("Connecting via relay server {}...", relay_server);
        let mut c = socket_client::connect_tcp(relay_server.clone(), CONNECT_TIMEOUT).await
            .with_context(|| format!("Failed to connect to relay {}", relay_server))?;
        let mut msg_out = RendezvousMessage::new();
        msg_out.set_request_relay(RequestRelay {
            id: device_id.clone(), uuid: relay_uuid,
            licence_key: licence_key.clone(),
            conn_type: ConnType::TERMINAL.into(), ..Default::default()
        });
        send_msg(&mut c, &msg_out, "request_relay").await?;
        c
    };

    // Phase 3: E2E key exchange
    let rs_pk = get_rs_pk(key_str).context("Invalid rendezvous server key")?;
    let peer_sign_pk = if !peer_pk_from_server.is_empty() {
        let (vouched_id, pk) = decode_id_pk(&peer_pk_from_server, &rs_pk)
            .context("Failed to verify peer key from rendezvous")?;
        log::debug!("Peer key vouched: {}", vouched_id);
        Some(sign::PublicKey(pk))
    } else { None };

    let msg_in = recv_msg(&mut conn, "wait_signed_id").await?;
    let signed_id = match msg_in.union {
        Some(message::Union::SignedId(si)) => si,
        other => bail!("Expected SignedId, got: {:?}", other.map(|_| "other")),
    };
    let peer_sign_pk = peer_sign_pk
        .ok_or_else(|| anyhow::anyhow!("No peer public key from rendezvous server"))?;
    let (peer_id, their_pk) = decode_id_pk(&signed_id.id, &peer_sign_pk)?;
    log::info!("Peer identity verified: {}", peer_id);

    let (av, sv, enc_key) = create_symmetric_key_msg(their_pk);
    let mut pk_msg = Message::new();
    pk_msg.set_public_key(PublicKey { asymmetric_value: av.into(), symmetric_value: sv.into(), ..Default::default() });
    send_msg(&mut conn, &pk_msg, "public_key").await?;
    conn.set_key(enc_key);
    log::info!("End-to-end encryption established");

    // Phase 4: Password authentication
    let msg_in = recv_msg(&mut conn, "wait_hash").await?;
    let hash = match msg_in.union {
        Some(message::Union::Hash(h)) => h,
        _ => bail!("Expected Hash challenge"),
    };
    let mut h1 = Sha256::new();
    h1.update(password.as_bytes()); h1.update(hash.salt.as_bytes());
    let mut h2 = Sha256::new();
    h2.update(&h1.finalize()[..]); h2.update(hash.challenge.as_bytes());
    let pw_response: Vec<u8> = h2.finalize()[..].into();

    // Phase 5: Login with Terminal
    let mut lr = LoginRequest::new();
    lr.username = device_id.clone(); lr.password = pw_response.into();
    lr.my_id = format!("rustshell-{}", std::process::id());
    lr.version = VERSION.to_owned();
    let mut terminal = Terminal::new();
    terminal.service_id = format!("ts_{}", uuid::Uuid::new_v4());
    lr.set_terminal(terminal);
    let mut lr_msg = Message::new();
    lr_msg.set_login_request(lr);
    send_msg(&mut conn, &lr_msg, "login_request").await?;
    log::info!("Login request sent");

    let bytes = recv_raw(&mut conn, "wait_login_response").await?;
    // Some server versions send LoginResponse directly (not wrapped in Message).
    // Try Message first, then fall back to raw LoginResponse.
    let lr = match Message::parse_from_bytes(&bytes) {
        Ok(m) => match m.union {
            Some(message::Union::LoginResponse(lr)) => lr,
            Some(message::Union::TerminalResponse(_)) => {
                log::debug!("Early terminal response, proceeding");
                LoginResponse::new()
            }
            _ => LoginResponse::parse_from_bytes(&bytes).unwrap_or_default(),
        },
        Err(_) => LoginResponse::parse_from_bytes(&bytes).unwrap_or_default(),
    };
    let mut remote_platform = String::new();
    match lr.union {
        Some(login_response::Union::Error(err)) if !err.is_empty() => bail!("Login failed: {}", err),
        Some(login_response::Union::PeerInfo(pi)) => {
            log::info!("Connected to {} ({} {})", pi.hostname, pi.platform, pi.version);
            remote_platform = pi.platform;
        }
        _ => log::debug!("Login accepted"),
    }

    // Phase 6: Terminal I/O
    terminal_io_loop(&mut conn, &remote_platform).await
}

// ── secure_tcp ─────────────────────────────────────────────────────

async fn attempt_secure_tcp(conn: &mut Stream, key: &str) -> Result<()> {
    let rs_pk = match get_rs_pk(key) {
        Some(pk) => pk,
        None => { log::debug!("No valid key, skipping secure_tcp"); return Ok(()); }
    };
    match hbb_common::timeout(3000, conn.next()).await {
        Ok(Some(Ok(bytes))) => {
            let rmsg = match RendezvousMessage::parse_from_bytes(&bytes) {
                Ok(m) => m, Err(_) => { log::debug!("Non-protobuf, skipping"); return Ok(()); }
            };
            let ex = match rmsg.union {
                Some(hbb_common::rendezvous_proto::rendezvous_message::Union::KeyExchange(ex)) => ex,
                _ => { log::debug!("No KeyExchange, proceeding"); return Ok(()); }
            };
            if ex.keys.len() != 1 { log::warn!("Invalid KeyExchange"); return Ok(()); }
            let their_pk_b = match sign::verify(&ex.keys[0], &rs_pk) {
                Ok(pk) => pk, Err(_) => { log::warn!("Sig verify failed"); return Ok(()); }
            };
            let their_pk = match get_pk(&their_pk_b) {
                Some(pk) => pk, None => { log::warn!("Invalid pk len"); return Ok(()); }
            };
            let (av, sv, enc) = create_symmetric_key_msg(their_pk);
            let mut mo = RendezvousMessage::new();
            mo.set_key_exchange(KeyExchange { keys: vec![av.into(), sv.into()], ..Default::default() });
            send_msg(conn, &mo, "key_exchange_response").await?;
            conn.set_key(enc);
            log::info!("Secure channel with rendezvous server");
        }
        Ok(Some(Err(e))) => { log::warn!("Stream err: {e}"); }
        Ok(None) => bail!("Rendezvous server closed connection"),
        Err(_) => { log::debug!("No KeyExchange (timeout), proceeding"); }
    }
    Ok(())
}

// ── Terminal I/O loop ──────────────────────────────────────────────

async fn terminal_io_loop(conn: &mut Stream, remote_platform: &str) -> Result<()> {
    let _guard = ConsoleGuard::enable()?;
    let (cols, rows) = crossterm::terminal::size().context("Failed to get terminal size")?;
    let terminal_id: i32 = 1;

    {
        let mut action = TerminalAction::new();
        action.set_open(OpenTerminal { terminal_id, rows: rows as u32, cols: cols as u32, ..Default::default() });
        let mut msg = Message::new();
        msg.set_terminal_action(action);
        send_msg(conn, &msg, "open_terminal").await?;
    }
    log::debug!("Terminal opened ({}x{})", cols, rows);

    let mut input_timer = time::interval(std::time::Duration::from_millis(20));
    let mut keepalive = time::interval(std::time::Duration::from_secs(15));
    let mut terminal_opened = false;
    let mut locale_injected = false;
    let mut last_cols = cols;
    let mut last_rows = rows;

    loop {
        tokio::select! {
            _ = keepalive.tick() => { conn.send(&Message::new()).await.ok(); }

            res = conn.next() => {
                let bytes = match res {
                    Some(Ok(b)) => b,
                    Some(Err(e)) => { log::error!("Stream error: {}", e); break; }
                    None => { log::info!("Connection closed by peer"); break; }
                };
                let msg_in = match Message::parse_from_bytes(&bytes) {
                    Ok(m) => m, Err(e) => { log::error!("Parse: {}", e); continue; }
                };
                match msg_in.union {
                    Some(message::Union::TerminalResponse(resp)) => {
                        use terminal_response::Union;
                        match resp.union {
                            Some(Union::Opened(o)) => {
                                terminal_opened = o.success;
                                if !o.success { bail!("Terminal open failed: {}", o.message); }
                                log::debug!("Shell started (pid: {})", o.pid);
                            }
                            Some(Union::Data(data)) => {
                                let output = if data.compressed {
                                    hbb_common::compress::decompress(&data.data)
                                } else { data.data.to_vec() };
                                write_stdout(&output);
                            }
                            Some(Union::Closed(c)) => {
                                log::info!("Terminal closed (exit code: {})", c.exit_code);
                                return Ok(());
                            }
                            Some(Union::Error(e)) => bail!("Terminal error: {}", e.message),
                            _ => {}
                        }
                    }
                    Some(message::Union::Hash(_)) => {}
                    _ => { log::trace!("Unhandled msg"); }
                }
            }

            _ = input_timer.tick() => {
                // Inject UTF-8 locale fix after shell starts.
                // The remote PTY may run in C locale, breaking CJK echo.
                // Sending these exports makes zsh/bash handle multi-byte input correctly.
                if terminal_opened && !locale_injected {
                    locale_injected = true;
                    // Pick the right UTF-8 locale setup command for the remote platform.
                    // - POSIX shells (bash/zsh on macOS/Linux): export LANG/LC_ALL
                    // - Windows (PowerShell/cmd): chcp 65001
                    let cmd: &[u8] = if remote_platform.eq_ignore_ascii_case("Windows") {
                        b"chcp 65001 >nul 2>&1\r"
                    } else {
                        b"export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 2>/dev/null; stty iutf8 2>/dev/null\r"
                    };
                    let mut a = TerminalAction::new();
                    a.set_data(TerminalData { terminal_id, data: cmd.to_vec().into(), compressed: false, ..Default::default() });
                    let mut m = Message::new(); m.set_terminal_action(a);
                    conn.send(&m).await.ok();
                    log::debug!("Injected locale fix (LANG/LC_ALL=en_US.UTF-8)");
                }

                if let Ok((nc, nr)) = crossterm::terminal::size() {
                    if (nc != last_cols || nr != last_rows) && terminal_opened {
                        log::debug!("Resize: {}x{}", nc, nr);
                        let mut a = TerminalAction::new();
                        a.set_resize(ResizeTerminal { terminal_id, rows: nr as u32, cols: nc as u32, ..Default::default() });
                        let mut m = Message::new(); m.set_terminal_action(a);
                        conn.send(&m).await.ok();
                        last_cols = nc; last_rows = nr;
                    }
                }
                while let Some(data) = poll_key_event() {
                    if data.is_empty() { continue; }
                    if data == [3] || data == [4] {
                        log::info!("Closing terminal...");
                        if terminal_opened {
                            let mut a = TerminalAction::new();
                            a.set_close(CloseTerminal { terminal_id, ..Default::default() });
                            let mut m = Message::new(); m.set_terminal_action(a);
                            conn.send(&m).await.ok();
                        }
                        return Ok(());
                    }
                    if terminal_opened {
                        let mut a = TerminalAction::new();
                        a.set_data(TerminalData { terminal_id, data: data.into(), compressed: false, ..Default::default() });
                        let mut m = Message::new(); m.set_terminal_action(a);
                        conn.send(&m).await.ok();
                    }
                }
            }
        }
    }
    Ok(())
}
