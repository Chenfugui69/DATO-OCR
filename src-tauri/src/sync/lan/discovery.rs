//! 局域网里互相发现（mDNS / Bonjour，`_datocor._tcp.local.`，规格 09 §4.1）。
//!
//! 主机广播自己的设备 ID、名字、设备码和端口；加入方输入设备码后在局域网里找码对得上的那台。
//! 设备码不是秘密（真正的门槛是主机上点"同意"），所以放在广播里没问题。
//! 有的路由器 / 公司网络会挡组播，找不到时界面上可以手填地址。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::{AppError, AppResult};

pub const SERVICE: &str = "_datocor._tcp.local.";

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    pub fn start(device_id: &str, name: &str, code: &str, port: u16) -> AppResult<Self> {
        let daemon =
            ServiceDaemon::new().map_err(|e| AppError::msg(format!("mDNS 启动失败：{e}")))?;
        let host = format!("dato-{}.local.", &device_id[..device_id.len().min(12)]);
        let props = [
            ("id", device_id),
            ("name", name),
            ("code", code),
            ("v", "1"),
        ];
        let info = ServiceInfo::new(SERVICE, device_id, &host, "", port, &props[..])
            .map_err(|e| AppError::msg(format!("mDNS 服务信息无效：{e}")))?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon
            .register(info)
            .map_err(|e| AppError::msg(format!("mDNS 广播失败：{e}")))?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[derive(Clone, Debug)]
pub struct Found {
    pub device_id: String,
    pub code: String,
    pub addrs: Vec<SocketAddr>,
}

/// 在局域网里找 `timeout` 这么久；`stop` 返回 true 就提前结束（比如已经找到要的那台）。
pub fn browse(timeout: Duration, stop: impl Fn(&Found) -> bool) -> AppResult<Vec<Found>> {
    let daemon = ServiceDaemon::new().map_err(|e| AppError::msg(format!("mDNS 启动失败：{e}")))?;
    let rx = daemon
        .browse(SERVICE)
        .map_err(|e| AppError::msg(format!("mDNS 搜索失败：{e}")))?;
    let deadline = Instant::now() + timeout;
    let mut found: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else {
            break;
        };
        let ServiceEvent::ServiceResolved(info) = event else {
            continue;
        };
        let prop = |k: &str| info.get_property_val_str(k).unwrap_or_default().to_string();
        let mut addrs: Vec<SocketAddr> = info
            .get_addresses_v4()
            .into_iter()
            .map(|ip| SocketAddr::new(IpAddr::V4(ip), info.port))
            .collect();
        addrs.sort_by_key(|a| rank(a.ip()));
        let item = Found {
            device_id: prop("id"),
            code: prop("code"),
            addrs,
        };
        if item.device_id.is_empty() || item.addrs.is_empty() {
            continue;
        }
        let done = stop(&item);
        found.retain(|f| f.device_id != item.device_id);
        found.push(item);
        if done {
            break;
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    Ok(found)
}

/// 家用网段排前面，虚拟网卡、代理软件的虚拟网段排后面。
pub fn rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            match o {
                [192, 168, ..] => 0,
                [10, ..] => 1,
                [172, b, ..] if (16..32).contains(&b) => 2,
                // 100.64/10：CGNAT、Tailscale
                [100, b, ..] if (64..128).contains(&b) => 3,
                // 198.18/15：Clash 之类 TUN 模式的虚拟网段，手机连不上
                [198, 18 | 19, ..] => 9,
                _ => 5,
            }
        }
        IpAddr::V6(_) => 8,
    }
}

/// 只接受局域网里来的连接：私有网段、链路本地、回环、CGNAT（Tailscale）。
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xffc0) == 0xfe80 || (seg & 0xfe00) == 0xfc00
        }
    }
}

/// 本机的局域网地址（给手机扫码用），好用的排前面。
pub fn local_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4() && is_local(*ip))
        .collect();
    ips.sort_by_key(|ip| rank(*ip));
    ips.dedup();
    ips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ranges() {
        for ok in [
            "192.168.1.5",
            "10.0.0.2",
            "172.20.1.1",
            "127.0.0.1",
            "169.254.3.4",
            "100.100.1.1",
            "::1",
            "fe80::1",
            "::ffff:192.168.1.5",
        ] {
            assert!(is_local(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "100.128.0.1", "2001:db8::1"] {
            assert!(!is_local(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn home_network_ranks_first() {
        let mut ips: Vec<IpAddr> = ["198.18.0.1", "10.1.1.1", "192.168.31.8"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        ips.sort_by_key(|ip| rank(*ip));
        assert_eq!(ips[0].to_string(), "192.168.31.8");
        assert_eq!(ips[2].to_string(), "198.18.0.1");
    }
}
