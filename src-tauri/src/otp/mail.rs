//! 邮箱验证码：用 IMAP 连着邮箱的收件箱，新邮件一到就看里面有没有验证码。Windows、macOS 是同一套。
//!
//! - 只读：收件箱用 EXAMINE 打开、正文用 BODY.PEEK 取，不改"已读"，不删不移。
//! - 只看连上之后新到的邮件（记住当时最大的 UID），不翻以前的。断线重连后补看断开期间到的，
//!   但太旧的（超过 `FRESH`）不要：验证码早过期了。
//! - 邮件内容只在内存里过一遍，认出验证码才把主题和开头一小段存进历史（和短信一样），别的不留、不写日志。
//! - 服务器支持 IDLE 就挂着等推送（几秒内到），不支持的隔一会儿问一次。
//! - 密码（授权码 / 应用专用密码）加密存在密钥表里，不进设置文件，不参与多端同步。

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use native_tls::{TlsConnector, TlsStream};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::AppHandle;

use super::{mail_text, Found};
use crate::error::{AppError, AppResult};
use crate::settings::MailCodeSettings;
use crate::state::state;
use crate::storage::secrets;

/// 密钥表里的键
pub const SECRET: &str = "mail.password";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 平常一问一答等多久
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// IDLE 最多挂这么久就重新挂一次：路由器会掐掉长时间没动静的连接，隔几分钟动一下才稳
const IDLE_ROUND: Duration = Duration::from_secs(4 * 60);
/// 服务器不支持 IDLE 时多久问一次
const POLL_EVERY: Duration = Duration::from_secs(15);
/// 到了这么久的邮件不看了
const FRESH: Duration = Duration::from_secs(10 * 60);
/// 超过这么大的邮件不取（验证码邮件都很小）
const MAX_MAIL_BYTES: u32 = 400 * 1024;
/// 断线后第一次重连等多久，之后每次翻倍，最长 `RETRY_MAX`
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);

type Session = imap::Session<Wire<TlsStream<TcpStream>>>;

/// 连接外面套的一层：发 ID 命令期间把服务器回的 `* ID (...)` 那一行滤掉。
///
/// 用的 IMAP 库（imap 2.4 / imap-proto 0.10）不认识这种回应：读到它就当出错返回，ID 命令自己的
/// 结束行（`a3 OK`）留在连接里没读走。下一条命令（打开收件箱，`a4`）读到的是这行旧的，标签对不上，
/// 库里的断言直接崩掉（用户在 Gmail 上点"测试连接"报过 `left: [97, 51] right: [97, 52]`）。
/// 那一行本来就用不上，在它进库之前扔掉，ID 命令就能正常收尾。只在发 ID 时打开，
/// 免得误伤邮件正文里恰好以 `* ID ` 开头的行。
pub struct Wire<S> {
    inner: S,
    filter: Arc<AtomicBool>,
    /// 已经从连接上读到、还没凑成整行的字节（只在过滤时用）
    pending: Vec<u8>,
    /// 过滤完、等着交给库的字节
    ready: Vec<u8>,
}

impl<S> Wire<S> {
    fn new(inner: S, filter: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            filter,
            pending: Vec::new(),
            ready: Vec::new(),
        }
    }

    fn hand_out(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.ready.len());
        out[..n].copy_from_slice(&self.ready[..n]);
        self.ready.drain(..n);
        n
    }
}

impl<S: Read> Read for Wire<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.ready.is_empty() {
                return Ok(self.hand_out(out));
            }
            if !self.filter.load(Ordering::SeqCst) {
                if self.pending.is_empty() {
                    return self.inner.read(out);
                }
                self.ready.append(&mut self.pending);
                continue;
            }
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                if !line.starts_with(b"* ID ") {
                    self.ready.extend_from_slice(&line);
                }
                continue;
            }
            let mut chunk = [0u8; 4096];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                // 连接断了：剩下的半行原样交出去，让库自己报错
                self.ready.append(&mut self.pending);
                if self.ready.is_empty() {
                    return Ok(0);
                }
                continue;
            }
            self.pending.extend_from_slice(&chunk[..n]);
        }
    }
}

impl<S: Write> Write for Wire<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl imap::extensions::idle::SetReadTimeout for Wire<TlsStream<TcpStream>> {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.inner
            .get_ref()
            .set_read_timeout(timeout)
            .map_err(imap::Error::Io)
    }
}

/// 每次重新配置加一；后台线程发现自己的号过期了就退出
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 当前连接的套接字（另一个句柄）：要它马上停下 / 重连时把它关掉，卡在读上的后台线程立刻返回
static SOCKET: Mutex<Option<TcpStream>> = Mutex::new(None);
static STATUS: Mutex<Status> = Mutex::new(Status::Off);

/// 设置页上显示的连接状态
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// 没开
    Off,
    /// 开了但邮箱或密码还没填
    Incomplete,
    /// 离线模式下不连
    Offline,
    Connecting,
    Connected,
    /// 连不上 / 登录不了，过一会儿自动再试
    Error {
        message: String,
    },
}

pub fn status() -> Status {
    STATUS.lock().clone()
}

fn set_status(generation: u64, status: Status) {
    if alive(generation) {
        *STATUS.lock() = status;
    }
}

fn alive(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

/// 连哪台服务器
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// 先明文连上再升级加密（STARTTLS）；false = 一上来就是加密的
    pub starttls: bool,
}

impl Endpoint {
    /// 本机回环地址：Proton Mail Bridge 这类本机网关用的是自己签的证书，没法按域名验证，只对本机地址放行
    fn loopback(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
}

/// 按设置算出服务器地址。预设的几家是定死的；Proton 走本机的 Proton Mail Bridge。
pub fn endpoint(cfg: &MailCodeSettings) -> AppResult<Endpoint> {
    let tls = |host: &str| Endpoint {
        host: host.into(),
        port: 993,
        starttls: false,
    };
    Ok(match cfg.provider.as_str() {
        "qq" => tls("imap.qq.com"),
        "163" => tls("imap.163.com"),
        "gmail" => tls("imap.gmail.com"),
        "icloud" => tls("imap.mail.me.com"),
        "proton" => Endpoint {
            host: "127.0.0.1".into(),
            port: if cfg.port == 0 { 1143 } else { cfg.port },
            starttls: true,
        },
        _ => {
            if cfg.host.is_empty() {
                return Err(AppError::msg("还没填邮箱服务器地址"));
            }
            let starttls = cfg.security == "starttls";
            Endpoint {
                host: cfg.host.clone(),
                port: match cfg.port {
                    0 if starttls => 143,
                    0 => 993,
                    port => port,
                },
                starttls,
            }
        }
    })
}

fn password(app: &AppHandle) -> Option<String> {
    state(app)
        .db
        .with(|c| secrets::get(c, SECRET))
        .ok()
        .flatten()
}

/// 存 / 清掉邮箱密码，然后按新的重连。
pub fn set_password(app: &AppHandle, value: Option<&str>) -> AppResult<()> {
    state(app).db.with(|c| secrets::set(c, SECRET, value))?;
    reconfigure(app);
    Ok(())
}

pub fn has_password(app: &AppHandle) -> bool {
    password(app).is_some()
}

/// 按当前设置重新来过：停掉原来的连接，该连的话另起一条。启动时、改了设置 / 密码 / 离线模式时调用。
pub fn reconfigure(app: &AppHandle) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    drop_socket();
    let st = state(app);
    let (cfg, offline) = {
        let s = st.settings.read();
        (s.clipboard.mail_codes.clone(), s.general.offline_mode)
    };
    let start = if !cfg.enabled {
        Err(Status::Off)
    } else if offline {
        Err(Status::Offline)
    } else {
        match (password(app), endpoint(&cfg)) {
            (Some(password), Ok(endpoint)) if !cfg.address.is_empty() => Ok((endpoint, password)),
            _ => Err(Status::Incomplete),
        }
    };
    match start {
        Err(status) => *STATUS.lock() = status,
        Ok((endpoint, password)) => {
            *STATUS.lock() = Status::Connecting;
            let app = app.clone();
            let _ = std::thread::Builder::new()
                .name("otp-mail".into())
                .spawn(move || run(&app, generation, &endpoint, &cfg.address, &password));
        }
    }
}

/// 睡眠唤醒、换了网络之后：原来的连接多半已经死了，但要等到超时才知道。直接关掉，后台线程马上重连。
pub fn kick() {
    drop_socket();
}

fn drop_socket() {
    if let Some(socket) = SOCKET.lock().take() {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
}

/// 设置页的"测试连接"：用填的账号连一次、登录、打开收件箱。`password` 没给就用已经存着的。
pub fn test(
    app: &AppHandle,
    cfg: &MailCodeSettings,
    password_input: Option<&str>,
) -> AppResult<()> {
    if cfg.address.trim().is_empty() {
        return Err(AppError::msg("还没填邮箱地址"));
    }
    let password = password_input
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| password(app))
        .ok_or_else(|| AppError::msg("还没填邮箱密码"))?;
    let endpoint = endpoint(cfg)?;
    let (mut session, _socket) = connect(&endpoint, cfg.address.trim(), &password)?;
    let opened = session.examine("INBOX").map_err(|e| explain(&e, &endpoint));
    let _ = session.logout();
    opened.map(|_| ())
}

fn run(app: &AppHandle, generation: u64, endpoint: &Endpoint, user: &str, password: &str) {
    // 看过的最大 UID；None = 还没连上过，连上时从当时最新的那封之后算起
    let mut seen: Option<u32> = None;
    let mut retry = RETRY_FIRST;
    while alive(generation) {
        set_status(generation, Status::Connecting);
        let started = Instant::now();
        let err = match watch(app, generation, endpoint, user, password, &mut seen) {
            Ok(()) => return,
            Err(err) => err,
        };
        if !alive(generation) {
            return;
        }
        tracing::warn!(host = %endpoint.host, "邮箱连接断了：{err}");
        set_status(
            generation,
            Status::Error {
                message: crate::i18n::text(&err),
            },
        );
        // 连上好一阵子才断的（睡眠、换网络）：马上重连；一连就失败的越等越久，别一直去撞服务器
        if started.elapsed() > Duration::from_secs(60) {
            retry = RETRY_FIRST;
        }
        let until = Instant::now() + retry;
        while alive(generation) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(500));
        }
        retry = (retry * 2).min(RETRY_MAX);
    }
}

/// 连上、守着收件箱，直到出错（返回 Err）或者被叫停（返回 Ok）。
fn watch(
    app: &AppHandle,
    generation: u64,
    endpoint: &Endpoint,
    user: &str,
    password: &str,
    seen: &mut Option<u32>,
) -> AppResult<()> {
    let (mut session, socket) = connect(endpoint, user, password)?;
    let timeouts = socket.try_clone()?;
    *SOCKET.lock() = Some(socket);
    // 连接期间又被重新配置了：新的那次已经清过 SOCKET，这条是漏网的，自己关掉
    if !alive(generation) {
        drop_socket();
        return Ok(());
    }
    let fail = |e: imap::Error| explain(&e, endpoint);

    let mailbox = session.examine("INBOX").map_err(fail)?;
    let newest = mailbox.uid_next.map_or(0, |next| next.saturating_sub(1));
    match seen {
        // 邮箱被清空 / 重建过，UID 从头排了
        Some(last) if newest < *last => *seen = Some(newest),
        Some(_) => check_new(app, &mut session, seen).map_err(fail)?,
        None => *seen = Some(newest),
    }
    let idle = session
        .capabilities()
        .map(|caps| caps.has_str("IDLE"))
        .unwrap_or(false);
    tracing::info!(host = %endpoint.host, idle, "邮箱已连上，等新邮件");
    set_status(generation, Status::Connected);

    while alive(generation) {
        if idle {
            session
                .idle()
                .and_then(|handle| handle.wait_with_timeout(IDLE_ROUND))
                .map_err(fail)?;
            // IDLE 结束时库会把读超时清掉，设回来
            timeouts.set_read_timeout(Some(IO_TIMEOUT))?;
        } else {
            let until = Instant::now() + POLL_EVERY;
            while alive(generation) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(500));
            }
            session.noop().map_err(fail)?;
        }
        if !alive(generation) {
            break;
        }
        check_new(app, &mut session, seen).map_err(fail)?;
    }
    let _ = session.logout();
    Ok(())
}

fn connect(endpoint: &Endpoint, user: &str, password: &str) -> AppResult<(Session, TcpStream)> {
    let unreachable = || {
        AppError::msg(if endpoint.loopback() {
            format!(
                "连不上本机的 {}:{}，请确认 Proton Mail Bridge 开着",
                endpoint.host, endpoint.port
            )
        } else {
            format!("连不上邮箱服务器 {}:{}", endpoint.host, endpoint.port)
        })
    };
    let addrs: Vec<_> = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|_| unreachable())?
        .collect();
    let tcp = addrs
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, CONNECT_TIMEOUT).ok())
        .ok_or_else(unreachable)?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
    let socket = tcp.try_clone()?;

    let insecure = endpoint.loopback();
    let tls = TlsConnector::builder()
        .danger_accept_invalid_certs(insecure)
        .danger_accept_invalid_hostnames(insecure)
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;
    let fail = |e: imap::Error| explain(&e, endpoint);
    let handshake = |tcp: TcpStream| {
        tls.connect(&endpoint.host, tcp)
            .map_err(|e| AppError::Network(format!("加密连接建立失败：{e}")))
    };
    let filter = Arc::new(AtomicBool::new(false));
    let client = if endpoint.starttls {
        // 明文连上、说 STARTTLS，再在同一个连接上握手。不用库的 `secure()`：那样拿不到底下的连接，套不上 `Wire`
        let mut tcp = tcp;
        starttls(&mut tcp)?;
        imap::Client::new(Wire::new(handshake(tcp)?, filter.clone()))
    } else {
        let mut client = imap::Client::new(Wire::new(handshake(tcp)?, filter.clone()));
        client.read_greeting().map_err(fail)?;
        client
    };
    let mut session = client.login(user, password).map_err(|(e, _)| match e {
        imap::Error::No(_) | imap::Error::Bad(_) => {
            AppError::msg("邮箱登录失败：请检查邮箱地址和授权码（不是网页登录的密码）")
        }
        other => explain(&other, endpoint),
    })?;
    // 网易的邮箱要求客户端先报上名字（ID 命令），不报的话打开收件箱时会被当成"不安全的登录"拒绝
    let wants_id = session
        .capabilities()
        .map(|caps| caps.has_str("ID"))
        .unwrap_or(false);
    if wants_id {
        let id = format!(
            "ID (\"name\" \"DATO OCR\" \"version\" \"{}\")",
            env!("CARGO_PKG_VERSION")
        );
        filter.store(true, Ordering::SeqCst);
        let sent = session.run_command_and_read_response(id);
        filter.store(false, Ordering::SeqCst);
        if let Err(err) = sent {
            tracing::debug!("邮箱 ID 命令没成功（不影响收信）：{err}");
        }
    }
    Ok((session, socket))
}

/// STARTTLS 明文的那一小段自己说：读问候、发 STARTTLS、等它回 OK。一个字节一个字节地读，
/// 握手之前不能从连接上多读走任何东西。
fn starttls(tcp: &mut TcpStream) -> AppResult<()> {
    fn line(tcp: &mut TcpStream) -> io::Result<String> {
        let mut bytes = Vec::new();
        let mut b = [0u8];
        while bytes.last() != Some(&b'\n') {
            if tcp.read(&mut b)? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            bytes.push(b[0]);
        }
        Ok(String::from_utf8_lossy(&bytes).trim_end().to_string())
    }
    let greeting = line(tcp)?;
    if !greeting.starts_with("* OK") {
        return Err(AppError::Network(format!("邮箱服务器没有正常应答：{greeting}")));
    }
    tcp.write_all(b"a0 STARTTLS\r\n")?;
    loop {
        let reply = line(tcp)?;
        if let Some(rest) = reply.strip_prefix("a0 ") {
            return if rest.starts_with("OK") {
                Ok(())
            } else {
                Err(AppError::Network(format!("邮箱服务器不肯切换到加密连接：{rest}")))
            };
        }
    }
}

/// 把库的报错换成用户看得懂的一句。
fn explain(err: &imap::Error, endpoint: &Endpoint) -> AppError {
    match err {
        imap::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            AppError::Network(format!("邮箱服务器 {} 没有回应", endpoint.host))
        }
        imap::Error::Io(_) | imap::Error::ConnectionLost => {
            AppError::Network("和邮箱服务器的连接断了".into())
        }
        imap::Error::No(text) | imap::Error::Bad(text) => {
            AppError::msg(format!("邮箱服务器拒绝了请求：{text}"))
        }
        other => AppError::Network(other.to_string()),
    }
}

/// 看 `seen` 之后有没有新邮件，有验证码就送出去。
fn check_new(
    app: &AppHandle,
    session: &mut Session,
    seen: &mut Option<u32>,
) -> imap::error::Result<()> {
    let last = seen.unwrap_or(0);
    // 先只问大小和到达时间，挑出要看的再取内容。"n:*" 在没有更新的邮件时会把最后一封也带回来，按 UID 再筛一遍
    let listed = session.uid_fetch(format!("{}:*", last + 1), "(UID INTERNALDATE RFC822.SIZE)")?;
    let oldest = chrono::Utc::now() - chrono::Duration::from_std(FRESH).unwrap_or_default();
    let mut wanted = Vec::new();
    let mut newest = last;
    for item in listed.iter() {
        let Some(uid) = item.uid.filter(|uid| *uid > last) else {
            continue;
        };
        newest = newest.max(uid);
        let fresh = item.internal_date().is_none_or(|at| at >= oldest);
        let small = item.size.is_none_or(|size| size <= MAX_MAIL_BYTES);
        if fresh && small {
            wanted.push(uid);
        }
    }
    drop(listed);
    *seen = Some(newest);
    // 一下子来了一大堆（断开了很久）：只看最新的几封
    wanted.sort_unstable();
    for uid in wanted.into_iter().rev().take(5).rev() {
        let fetched = session.uid_fetch(uid.to_string(), "BODY.PEEK[]")?;
        let Some(raw) = fetched.iter().find_map(imap::types::Fetch::body) else {
            continue;
        };
        if let Some(found) = mail_text::extract(raw) {
            tracing::info!("邮件里认出验证码");
            super::deliver(
                app,
                &found.code,
                &Found {
                    origin: &found.origin,
                    source: &found.sender,
                    app_path: None,
                    device_kind: Some(crate::platform::device_kind()),
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一问一答的假服务器：收到一条完整的命令才放出对应的那段回应（和真服务器一样，不会提前到）
    struct Script {
        replies: Vec<&'static str>,
        out: Vec<u8>,
    }
    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // 一次只给几个字节，测半行的情况
            let n = buf.len().min(5).min(self.out.len());
            buf[..n].copy_from_slice(&self.out[..n]);
            self.out.drain(..n);
            Ok(n)
        }
    }
    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.ends_with(b"\r\n") && !self.replies.is_empty() {
                self.out.extend_from_slice(self.replies.remove(0).as_bytes());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn id_reply_is_dropped_so_the_next_command_stays_in_step() {
        let script = Script {
            replies: vec![
                "a1 OK LOGIN done\r\n",
                "* ID (\"name\" \"GImap\" \"vendor\" \"Google, Inc.\")\r\na2 OK Success\r\n",
                "* 3 EXISTS\r\n* OK [UIDNEXT 9] next\r\na3 OK [READ-ONLY] INBOX selected\r\n",
            ],
            out: Vec::new(),
        };
        let filter = Arc::new(AtomicBool::new(false));
        let wire = Wire::new(script, filter.clone());
        let mut session = imap::Client::new(wire).login("u", "p").map_err(|(e, _)| e).unwrap();
        filter.store(true, Ordering::SeqCst);
        session.run_command_and_read_response("ID NIL").unwrap();
        filter.store(false, Ordering::SeqCst);
        let mailbox = session.examine("INBOX").unwrap();
        assert_eq!(mailbox.exists, 3);
        assert_eq!(mailbox.uid_next, Some(9));
    }

    fn cfg(provider: &str) -> MailCodeSettings {
        MailCodeSettings {
            provider: provider.into(),
            ..Default::default()
        }
    }

    #[test]
    fn preset_endpoints() {
        let qq = endpoint(&cfg("qq")).unwrap();
        assert_eq!(
            (qq.host.as_str(), qq.port, qq.starttls),
            ("imap.qq.com", 993, false)
        );
        assert!(!qq.loopback());
        let proton = endpoint(&cfg("proton")).unwrap();
        assert_eq!(
            (proton.host.as_str(), proton.port, proton.starttls),
            ("127.0.0.1", 1143, true)
        );
        assert!(proton.loopback());
        let moved = endpoint(&MailCodeSettings {
            port: 2143,
            ..cfg("proton")
        })
        .unwrap();
        assert_eq!(moved.port, 2143);
    }

    /// 真连一次各家的服务器（要联网，默认忽略）：`cargo test -- --ignored smoke_connect --nocapture`。
    /// 不带环境变量时用假账号，只看能不能连上、握手、得到"登录失败"；
    /// 带 `CHENOCR_MAIL=服务商,邮箱,密码` 时用真账号登录并打开收件箱。
    #[test]
    #[ignore]
    fn smoke_connect() {
        if let Ok(spec) = std::env::var("CHENOCR_MAIL") {
            let mut parts = spec.splitn(3, ',');
            let (provider, user, pass) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let (mut session, _socket) =
                connect(&endpoint(&cfg(provider)).unwrap(), user, pass).unwrap();
            let mailbox = session.examine("INBOX").unwrap();
            let idle = session.capabilities().unwrap().has_str("IDLE");
            println!(
                "{provider}: 已登录，收件箱 {} 封，下一个 UID {:?}，IDLE {idle}",
                mailbox.exists, mailbox.uid_next
            );
            return;
        }
        for provider in ["qq", "163", "gmail", "icloud"] {
            let started = Instant::now();
            let result = connect(
                &endpoint(&cfg(provider)).unwrap(),
                "nobody@example.invalid",
                "wrong-password",
            );
            println!(
                "{provider}: {:?} -> {}",
                started.elapsed(),
                result
                    .err()
                    .map_or("居然登录上了".into(), |e| e.to_string())
            );
        }
    }

    #[test]
    fn custom_endpoint() {
        assert!(endpoint(&cfg("custom")).is_err());
        let base = MailCodeSettings {
            host: "mail.example.com".into(),
            ..cfg("custom")
        };
        assert_eq!(endpoint(&base).unwrap().port, 993);
        let starttls = MailCodeSettings {
            security: "starttls".into(),
            ..base.clone()
        };
        let e = endpoint(&starttls).unwrap();
        assert_eq!((e.port, e.starttls), (143, true));
        assert_eq!(
            endpoint(&MailCodeSettings { port: 1993, ..base })
                .unwrap()
                .port,
            1993
        );
    }
}
