//! 联网用的 HTTP 客户端：按设置里的网络选项（系统代理 / 直连 / 自定义代理）构建，设置变了才重建。

use std::time::Duration;

use parking_lot::Mutex;
use tauri::AppHandle;

use crate::state::state;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// 翻译：一次请求超过 5 秒就换下一个源（规格 04 §6）
    Translate,
    /// AI 对话：流式输出可能持续几分钟，不设总超时，由用户随时停止
    Ai,
    /// WebDAV 网盘同步：传图片可能要一会儿
    Sync,
    /// 检查更新：拿一个小 JSON
    Update,
    /// 下载安装包：几十 MB，不设总超时，只要一直有数据进来就行
    Download,
}

pub fn client(app: &AppHandle, purpose: Purpose) -> reqwest::Client {
    static CACHE: Mutex<Vec<(Purpose, String, reqwest::Client)>> = Mutex::new(Vec::new());
    let net = state(app).settings.read().network.clone();
    let key = format!("{}|{}", net.proxy_mode, net.proxy_url);
    let mut cache = CACHE.lock();
    if let Some((_, _, c)) = cache.iter().find(|(p, k, _)| *p == purpose && *k == key) {
        return c.clone();
    }
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(if purpose == Purpose::Translate {
            5
        } else {
            15
        }));
    builder = match purpose {
        Purpose::Translate => builder.timeout(Duration::from_secs(5)),
        Purpose::Ai => builder.read_timeout(Duration::from_secs(120)),
        Purpose::Sync => builder.timeout(Duration::from_secs(90)),
        Purpose::Update => builder.timeout(Duration::from_secs(20)),
        Purpose::Download => builder.read_timeout(Duration::from_secs(60)),
    };
    builder = match net.proxy_mode.as_str() {
        "none" => builder.no_proxy(),
        "custom" if !net.proxy_url.is_empty() => {
            let url = if net.proxy_url.contains("://") {
                net.proxy_url.clone()
            } else {
                format!("http://{}", net.proxy_url)
            };
            match reqwest::Proxy::all(&url) {
                Ok(proxy) => builder.proxy(proxy),
                Err(err) => {
                    tracing::warn!("代理地址无效，改用系统代理：{err}");
                    builder
                }
            }
        }
        // system：reqwest 默认读 HTTP(S)_PROXY 环境变量和 Windows 的系统代理设置
        _ => builder,
    };
    let client = builder.build().unwrap_or_default();
    cache.retain(|(p, _, _)| *p != purpose);
    cache.push((purpose, key, client.clone()));
    client
}
