// Windows 端不做默认路由探测（见 get_primary_interface 注释），因此该平台不需要 Command
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// 公网 IP 回显源列表（均返回纯文本 IP），国内源排在前面以提升国内网络成功率。
const PUBLIC_IP_SOURCES: &[&str] = &[
    "https://ip.3322.net/",
    "https://4.ipw.cn",
    "https://api.ipify.org",
    "https://api64.ipify.org",
    "https://ifconfig.me/ip",
];

/// 单个源的请求超时时间。
const PER_SOURCE_TIMEOUT: Duration = Duration::from_secs(4);

/// 所有源并发竞速的总等待预算；超过该时间仍无有效结果则判定失败。
const OVERALL_TIMEOUT: Duration = Duration::from_secs(6);

/// 模拟浏览器的 User-Agent，避免部分回显服务（如 ifconfig.me）因默认 UA 返回 403。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 并发查询多个公网 IP 回显源，返回第一个校验通过的结果（竞速模式）。
///
/// 旧实现为同步串行请求 3 个境外源（每个 5 秒超时），网络不通时会长时间
/// 阻塞调用线程（曾导致 Tauri 主线程卡死、界面一直停留在"正在获取..."）。
/// 现改为所有源并发请求：首个合法 IP 立即返回；全部失败或超过总预算才报错，
/// 正常网络下通常 1 秒内即可拿到结果。
pub fn fetch_public_ip() -> Result<String, String> {
    let started_at = Instant::now();
    let (tx, rx) = mpsc::channel::<String>();
    let mut workers = Vec::with_capacity(PUBLIC_IP_SOURCES.len());

    for url in PUBLIC_IP_SOURCES {
        let tx = tx.clone();
        let url = (*url).to_owned();
        workers.push(thread::spawn(move || {
            if let Some(ip) = fetch_ip_from_source(&url) {
                // 接收端若已拿到更快的结果或已超时，发送会失败，忽略即可。
                let _ = tx.send(ip);
            }
        }));
    }
    // 主动关闭主线程持有的发送端，保证所有工作线程结束后接收端能收到 Disconnected。
    drop(tx);

    // 扣除前面创建线程已消耗的时间，保持总等待预算恒定。
    let remaining = OVERALL_TIMEOUT
        .checked_sub(started_at.elapsed())
        .unwrap_or(Duration::ZERO);

    let result = match rx.recv_timeout(remaining) {
        Ok(ip) => Ok(ip),
        Err(RecvTimeoutError::Timeout) => Err("公网IP获取超时（已并发尝试多个源）".to_string()),
        Err(RecvTimeoutError::Disconnected) => Err("所有公网IP源均获取失败".to_string()),
    };

    // 仍在运行的工作线程会随请求完成/超时自然结束，这里无需等待。
    drop(workers);
    result
}

/// 向单个回显源发起 HTTP 请求，并从响应文本中解析出合法 IP。
fn fetch_ip_from_source(url: &str) -> Option<String> {
    let body = ureq::get(url)
        .timeout(PER_SOURCE_TIMEOUT)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "text/plain, */*")
        .call()
        .ok()?
        .into_string()
        .ok()?;

    parse_ip_from_response(&body)
}

/// 从响应文本中提取第一个合法的 IPv4/IPv6 地址。
///
/// 正常源返回纯文本 IP；若响应被空白（或认证门户 HTML）包裹，
/// 退化为按空白分词后逐个尝试解析，避免把劫持页面误当成 IP 展示。
fn parse_ip_from_response(body: &str) -> Option<String> {
    body.split_whitespace()
        .find(|token| token.parse::<std::net::IpAddr>().is_ok())
        .map(|ip| ip.to_owned())
}

/// 本机网卡条目。`primary` 表示该地址属于"默认路由主网卡"的 IPv4 地址，
/// 即用户认知中"当前的本地 IP"（如 Wi-Fi 网卡 en0 上的 192.168.x.x）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIface {
    pub name: String,
    pub ip: String,
    pub primary: bool,
}

/// 查询承载默认路由（0.0.0.0）的网卡名。
///
/// 仅靠 getifaddrs 的枚举顺序无法识别"当前真正联网的网卡"：VPN / 代理
/// 客户端创建的虚拟网卡（feth*、utun*、bridge*）往往排在物理网卡前面，
/// 旧版因此把 VPN 内网地址（如 172.26.x.x）误当成当前 IP 显示。
/// 默认路由指向的网卡才是系统实际用来上网的网卡。
fn get_primary_interface() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("route")
            .args(["-n", "get", "default"])
            .output()
            .ok()?;
        // 输出形如："   interface: en0"
        parse_kv_line(&String::from_utf8_lossy(&output.stdout), "interface")
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("ip")
            .args(["route", "show", "default"])
            .output()
            .ok()?;
        // 输出形如："default via 192.168.10.1 dev en0 proto dhcp"
        let text = String::from_utf8_lossy(&output.stdout);
        text.split_whitespace()
            .position(|t| t == "dev")
            .map(|i| text.split_whitespace().nth(i + 1).unwrap().to_string())
    }

    // Windows 默认路由解析较繁琐，返回 None 后由调用方按网卡名启发式兜底。
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// 从 `key: value` 形式的命令输出中取出指定 key 的 value。
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn parse_kv_line(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(|l| l.trim())
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_owned())
}

/// 判断网卡是否为虚拟网卡（VPN / 代理隧道 / 容器桥接 / 互联网共享等）。
fn is_virtual_interface(name: &str) -> bool {
    const VIRTUAL_PREFIXES: &[&str] = &[
        // macOS：VPN/代理 TUN、以太网对、桥接、共享、回环/隧道辅助
        "utun", "feth", "bridge", "tap", "gif", "stf", "vlan", "ap1", "anpi",
        "awdl", "llw", "vmenet", "pdp_ip",
        // Linux：容器、虚拟化、隧道
        "docker", "veth", "br-", "virbr", "tun", "tap", "cni", "flannel",
        "cali", "kube",
    ];
    let lower = name.to_ascii_lowercase();
    VIRTUAL_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// 返回本机网卡 IP 列表（已排除环回/链路本地/未指定地址）。
///
/// 排序规则（把"当前 IP"稳定地排在第一位，避免虚拟网卡抢占）：
///   1. 默认路由主网卡的 IPv4（primary = true）
///   2. 其他物理网卡的 IPv4
///   3. 虚拟网卡（utun/feth/bridge 等）的 IPv4
///   4. 主网卡的全局 IPv6
///   5. 其他网卡的全局 IPv6
pub fn get_local_ips() -> Vec<LocalIface> {
    let primary_name = get_primary_interface();

    // 收集阶段：(name, ip, is_ipv6)
    let mut entries: Vec<(String, String, bool)> = Vec::new();

    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }
            let (ip, is_ipv6) = match iface.ip() {
                std::net::IpAddr::V4(v4) => (v4.to_string(), false),
                std::net::IpAddr::V6(v6) => (v6.to_string(), true),
            };
            // 跳过未指定地址、环回、链路本地，以及 IPv4 APIPA 自分配地址（169.254/16）
            if ip == "0.0.0.0"
                || ip == "::"
                || ip == "::1"
                || ip.starts_with("fe80:")
                || ip.starts_with("169.254.")
            {
                continue;
            }
            entries.push((iface.name.clone(), ip, is_ipv6));
        }
    }

    // 按 IP 去重，保留首次出现
    let mut seen = std::collections::HashSet::new();
    entries.retain(|(_, ip, _)| seen.insert(ip.clone()));

    // 默认路由查询失败时的启发式兜底：第一个物理网卡上的 IPv4 视为主网卡
    let fallback_name = primary_name.clone().or_else(|| {
        entries
            .iter()
            .find(|(name, _, is_ipv6)| !is_ipv6 && !is_virtual_interface(name))
            .map(|(name, _, _)| name.clone())
    });

    // 分桶排序，桶内保持 getifaddrs 的原始顺序
    let mut bucket_primary_v4: Vec<(String, String)> = Vec::new();
    let mut bucket_physical_v4: Vec<(String, String)> = Vec::new();
    let mut bucket_virtual_v4: Vec<(String, String)> = Vec::new();
    let mut bucket_primary_v6: Vec<(String, String)> = Vec::new();
    let mut bucket_other_v6: Vec<(String, String)> = Vec::new();

    for (name, ip, is_ipv6) in entries {
        let is_primary = fallback_name.as_deref() == Some(name.as_str());
        match (is_ipv6, is_primary) {
            (false, true) => bucket_primary_v4.push((name, ip)),
            (false, false) if is_virtual_interface(&name) => {
                bucket_virtual_v4.push((name, ip))
            }
            (false, false) => bucket_physical_v4.push((name, ip)),
            (true, true) => bucket_primary_v6.push((name, ip)),
            (true, false) => bucket_other_v6.push((name, ip)),
        }
    }

    let mut results: Vec<LocalIface> = Vec::new();
    let push = |list: Vec<(String, String)>, primary: bool, results: &mut Vec<LocalIface>| {
        for (name, ip) in list {
            results.push(LocalIface { name, ip, primary });
        }
    };
    // 仅主网卡 IPv4 的第一条标记 primary（通常也只有一条）
    let mut first_primary = true;
    for (name, ip) in bucket_primary_v4 {
        results.push(LocalIface {
            name,
            ip,
            primary: first_primary,
        });
        first_primary = false;
    }
    push(bucket_physical_v4, false, &mut results);
    push(bucket_virtual_v4, false, &mut results);
    push(bucket_primary_v6, false, &mut results);
    push(bucket_other_v6, false, &mut results);

    results
}
