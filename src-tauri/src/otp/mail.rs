//! 邮箱验证码：用 IMAP 连着邮箱的收件箱，新邮件一到就看里面有没有验证码。Windows、macOS 是同一套。
//!
//! - 只读：收件箱用 EXAMINE 打开、正文用 BODY.PEEK 取，不改"已读"，不删不移。
//! - 只看连上之后新到的邮件（记住当时最大的 UID），不翻以前的。断线重连后补看断开期间到的，
//!   但太旧的（超过 `FRESH`）不要：验证码早过期了。
//! - 邮件内容只在内存里过一遍，认出验证码才把主题和开头一小段存进历史（和短信一样），别的不留、不写日志。
//! - 服务器支持 IDLE 就挂着等推送（几秒内到），不支持的隔一会儿问一次。
//! - 密码（授权码 / 应用专用密码）加密存在密钥表里，不进设置文件，不参与多端同步。

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
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

type Session = imap::Session<TlsStream<TcpStream>>;

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
    let client = if endpoint.starttls {
        let mut client = imap::Client::new(tcp);
        client.read_greeting().map_err(fail)?;
        client.secure(&endpoint.host, &tls).map_err(fail)?
    } else {
        let stream = tls
            .connect(&endpoint.host, tcp)
            .map_err(|e| AppError::Network(format!("加密连接建立失败：{e}")))?;
        let mut client = imap::Client::new(stream);
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
        let _ = session.run_command_and_read_response(id);
    }
    Ok((session, socket))
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
