//! 局域网只读媒体服务。
//!
//! 设计边界（刻意保持简单、可审计）：
//! - 只读：数据库以 SQLITE_OPEN_READ_ONLY 打开，接口不提供写入/删除/扫描。
//! - 直接播放：不做转码、不做转封装，按 HTTP Range 发送原始文件。
//! - 不暴露磁盘路径：手机只拿到 video_id，路径只在本机根据 id 查库。
//! - 零第三方 HTTP 依赖：请求/响应格式足够窄，标准库即可稳定承载单机家用场景。
//!
//! 服务生命周期由 LanService 管理。关闭服务时只停监听和正在发送的流，
//! 不触碰 CineVault 主数据库连接。

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db;
use crate::models::Tag;

pub const DEFAULT_PORT: u16 = 42337;
const DISCOVERY_PORT: u16 = 42338;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_CONCURRENT: usize = 64;
const MAX_CONCURRENT_PER_IP: usize = 12;
const API_PAGE_SIZE_MAX: i64 = 200;
const HEADER_DEADLINE: Duration = Duration::from_secs(8);
const PAIR_CODE_TTL_SECS: u64 = 10 * 60;
const MEDIA_TICKET_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const PAIR_WINDOW: Duration = Duration::from_secs(60);
const PAIR_MAX_GLOBAL: usize = 12;
const PAIR_MAX_PER_IP: usize = 5;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// ============================================
// 对外状态与生命周期
// ============================================

#[derive(Debug, Clone, Serialize)]
pub struct LanStatus {
    /// 监听是否真的起来了（端口占用时 enabled 可能仍为 true）
    pub running: bool,
    /// 用户意图开关，服务下次启动会沿用
    pub enabled: bool,
    pub port: u16,
    pub name: String,
    pub token: String,
    pub pair_code: String,
    /// Unix 秒。到期后配对接口拒绝旧码，桌面端据此提示更换。
    pub pair_expires_at: u64,
    pub urls: Vec<String>,
    pub error: Option<String>,
}

struct LanRuntime {
    enabled: bool,
    running: bool,
    port: u16,
    token: String,
    pair_code: String,
    pair_expires_at: u64,
    error: Option<String>,
    stop: Option<Arc<AtomicBool>>,
    active_cfg: Option<Arc<ServerConfig>>,
    http_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
}

pub struct LanService {
    db_path: PathBuf,
    state: Mutex<LanRuntime>,
}

impl LanService {
    /// 启动时恢复上次的开关状态。端口绑定失败不 panic：让 UI 显示明确错误。
    pub fn new(db_path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(&db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;

        let stored_token = db::get_setting(&conn, "lan_token").map_err(|e| e.to_string())?;
        let token = match stored_token {
            Some(v) if !v.is_empty() => v,
            _ => {
                let v = random_hex(16)?;
                db::set_setting(&conn, "lan_token", &v).map_err(|e| e.to_string())?;
                v
            }
        };

        let stored_code = db::get_setting(&conn, "lan_pair_code").map_err(|e| e.to_string())?;
        let stored_expiry = db::get_setting(&conn, "lan_pair_expires_at")
            .map_err(|e| e.to_string())?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let stored_code_valid = stored_code
            .as_deref()
            .map(|v| v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        let (pair_code, pair_expires_at) = if stored_code_valid && stored_expiry > unix_now() {
            (stored_code.unwrap_or_default(), stored_expiry)
        } else {
            let v = random_pair_code()?;
            let expires = unix_now() + PAIR_CODE_TTL_SECS;
            db::set_setting(&conn, "lan_pair_code", &v).map_err(|e| e.to_string())?;
            db::set_setting(&conn, "lan_pair_expires_at", &expires.to_string())
                .map_err(|e| e.to_string())?;
            (v, expires)
        };
        let enabled = db::get_setting(&conn, "lan_enabled")
            .map_err(|e| e.to_string())?
            .as_deref()
            == Some("1");

        let service = Self {
            db_path,
            state: Mutex::new(LanRuntime {
                enabled,
                running: false,
                port: DEFAULT_PORT,
                token,
                pair_code,
                pair_expires_at,
                error: None,
                stop: None,
                active_cfg: None,
                http_thread: None,
                discovery_thread: None,
            }),
        };
        if enabled {
            let mut rt = service.state.lock().map_err(|e| e.to_string())?;
            if let Err(e) = service.start_locked(&mut rt) {
                rt.error = Some(e);
                rt.running = false;
            }
        }
        Ok(service)
    }

    fn status_locked(rt: &LanRuntime) -> LanStatus {
        let mut urls = Vec::new();
        if let Some(ip) = local_ipv4() {
            urls.push(format!("http://{}:{}", ip, rt.port));
        }
        if let Ok(host) = std::env::var("COMPUTERNAME") {
            let host = host.trim();
            if !host.is_empty() {
                // 多网卡/VPN 下默认路由地址未必是手机所在网络；把主机名解析出的
                // 其余局域网 IPv4 一并列出，让用户能选到正确那张网卡。
                if let Ok(addrs) = (host, rt.port).to_socket_addrs() {
                    for addr in addrs {
                        if let std::net::IpAddr::V4(ip) = addr.ip() {
                            if ip.is_loopback() || ip.is_unspecified() {
                                continue;
                            }
                            let u = format!("http://{}:{}", ip, rt.port);
                            if !urls.contains(&u) {
                                urls.push(u);
                            }
                        }
                    }
                }
                let u = format!("http://{}:{}", host, rt.port);
                if !urls.contains(&u) {
                    urls.push(u);
                }
            }
        }
        LanStatus {
            running: rt.running,
            enabled: rt.enabled,
            port: rt.port,
            name: server_name(),
            token: rt.token.clone(),
            pair_code: rt.pair_code.clone(),
            pair_expires_at: rt.pair_expires_at,
            urls,
            error: rt.error.clone(),
        }
    }

    pub fn status(&self) -> Result<LanStatus, String> {
        let rt = self.state.lock().map_err(|e| e.to_string())?;
        Ok(Self::status_locked(&rt))
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<LanStatus, String> {
        self.persist("lan_enabled", if enabled { "1" } else { "0" })?;
        let mut rt = self.state.lock().map_err(|e| e.to_string())?;
        rt.enabled = enabled;
        if enabled {
            if !rt.running {
                if let Err(e) = self.start_locked(&mut rt) {
                    rt.error = Some(e);
                    rt.running = false;
                }
            }
        } else {
            self.stop_locked(&mut rt);
        }
        Ok(Self::status_locked(&rt))
    }

    pub fn regenerate_token(&self) -> Result<LanStatus, String> {
        let token = random_hex(16)?;
        self.persist("lan_token", &token)?;
        let mut rt = self.state.lock().map_err(|e| e.to_string())?;
        rt.token = token;
        if rt.running {
            self.stop_locked(&mut rt);
            if let Err(e) = self.start_locked(&mut rt) {
                rt.error = Some(e);
                rt.running = false;
            }
        }
        Ok(Self::status_locked(&rt))
    }

    pub fn rotate_pair_code(&self) -> Result<LanStatus, String> {
        let code = random_pair_code()?;
        let expires = unix_now() + PAIR_CODE_TTL_SECS;
        self.persist("lan_pair_code", &code)?;
        self.persist("lan_pair_expires_at", &expires.to_string())?;
        let mut rt = self.state.lock().map_err(|e| e.to_string())?;
        rt.pair_code = code;
        rt.pair_expires_at = expires;
        // 运行中的 ServerConfig 持有配对码副本，必须重启才能让旧码立刻失效。
        if rt.running {
            self.stop_locked(&mut rt);
            if let Err(e) = self.start_locked(&mut rt) {
                rt.error = Some(e);
                rt.running = false;
            }
        }
        Ok(Self::status_locked(&rt))
    }

    fn persist(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("打开数据库失败: {e}"))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db::set_setting(&conn, key, value).map_err(|e| e.to_string())
    }

    fn start_locked(&self, rt: &mut LanRuntime) -> Result<(), String> {
        let stop = Arc::new(AtomicBool::new(false));
        let config = Arc::new(ServerConfig {
            db_path: self.db_path.clone(),
            token: rt.token.clone(),
            pair_code: rt.pair_code.clone(),
            pair_expires_at: rt.pair_expires_at,
            port: rt.port,
            name: server_name(),
            stop: stop.clone(),
            active: Arc::new(AtomicUsize::new(0)),
            per_ip_active: Arc::new(Mutex::new(HashMap::new())),
            pair_gate: Mutex::new(PairGate::default()),
            tickets: Mutex::new(HashMap::new()),
        });

        let listener = TcpListener::bind(("0.0.0.0", rt.port))
            .map_err(|e| format!("端口 {} 无法监听: {}", rt.port, e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("监听端口设置失败: {e}"))?;

        let cfg = config.clone();
        let http_thread = thread::Builder::new()
            .name("cinevault-lan-http".into())
            .spawn(move || http_loop(listener, cfg))
            .map_err(|e| format!("启动局域网线程失败: {e}"))?;

        // 发现服务不是核心链路：被防火墙/端口占用拦下时，HTTP 仍应可用。
        let mut discovery_error = None;
        let discovery_thread = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
            Ok(sock) => {
                let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
                let cfg = config.clone();
                match thread::Builder::new()
                    .name("cinevault-lan-discovery".into())
                    .spawn(move || discovery_loop(sock, cfg))
                {
                    Ok(h) => Some(h),
                    Err(e) => {
                        discovery_error = Some(format!("自动发现线程启动失败: {e}"));
                        None
                    }
                }
            }
            Err(e) => {
                discovery_error = Some(format!("自动发现端口 {} 不可用: {}", DISCOVERY_PORT, e));
                None
            }
        };

        rt.stop = Some(stop);
        rt.active_cfg = Some(config.clone());
        rt.http_thread = Some(http_thread);
        rt.discovery_thread = discovery_thread;
        rt.running = true;
        rt.error = discovery_error;
        Ok(())
    }

    fn stop_locked(&self, rt: &mut LanRuntime) {
        if let Some(stop) = rt.stop.take() {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(h) = rt.http_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = rt.discovery_thread.take() {
            let _ = h.join();
        }
        // 已在途的连接会看到 stop 标志并尽快退出。线程是 detach 的，没有句柄可 join；
        // 这里短暂等待配额归零，确保“关闭/重置令牌”之后不会继续用旧凭据完成请求。
        if let Some(cfg) = rt.active_cfg.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while cfg.active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
        }
        rt.running = false;
    }
}

impl Drop for LanService {
    fn drop(&mut self) {
        if let Ok(mut rt) = self.state.lock() {
            self.stop_locked(&mut rt);
        }
    }
}

// ============================================
// Tauri 命令
// ============================================

// Tauri 2 的非 async 命令跑在主线程。开关服务要 join 监听线程（最长约 500ms），
// 必须 async 化，避免点开关时窗口卡顿。
#[tauri::command]
pub async fn get_lan_status(lan: State<'_, LanService>) -> Result<LanStatus, String> {
    lan.status()
}

#[tauri::command]
pub async fn set_lan_enabled(
    lan: State<'_, LanService>,
    enabled: bool,
) -> Result<LanStatus, String> {
    lan.set_enabled(enabled)
}

#[tauri::command]
pub async fn regenerate_lan_token(lan: State<'_, LanService>) -> Result<LanStatus, String> {
    lan.regenerate_token()
}

#[tauri::command]
pub async fn rotate_lan_pair_code(lan: State<'_, LanService>) -> Result<LanStatus, String> {
    lan.rotate_pair_code()
}

// ============================================
// HTTP 服务
// ============================================

struct ServerConfig {
    db_path: PathBuf,
    token: String,
    pair_code: String,
    pair_expires_at: u64,
    port: u16,
    name: String,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    per_ip_active: Arc<Mutex<HashMap<IpAddr, usize>>>,
    pair_gate: Mutex<PairGate>,
    tickets: Mutex<HashMap<String, MediaTicket>>,
}

#[derive(Default)]
struct PairGate {
    global: VecDeque<Instant>,
    per_ip: HashMap<IpAddr, VecDeque<Instant>>,
}

impl PairGate {
    fn allow(&mut self, ip: IpAddr) -> bool {
        self.prune(Instant::now());
        let ip_failures = self.per_ip.get(&ip).map(VecDeque::len).unwrap_or(0);
        self.global.len() < PAIR_MAX_GLOBAL && ip_failures < PAIR_MAX_PER_IP
    }

    fn record_failure(&mut self, ip: IpAddr) {
        let now = Instant::now();
        self.prune(now);
        self.global.push_back(now);
        self.per_ip.entry(ip).or_default().push_back(now);
    }

    fn record_success(&mut self, ip: IpAddr) {
        // 成功不累积失败预算，但本次尝试仍计入全局限额，避免成功配对被用作无成本探测。
        self.prune(Instant::now());
        self.global.push_back(Instant::now());
        self.per_ip.remove(&ip);
    }

    fn prune(&mut self, now: Instant) {
        while self
            .global
            .front()
            .map(|t| now.duration_since(*t) > PAIR_WINDOW)
            .unwrap_or(false)
        {
            self.global.pop_front();
        }
        self.per_ip.retain(|_, q| {
            while q
                .front()
                .map(|t| now.duration_since(*t) > PAIR_WINDOW)
                .unwrap_or(false)
            {
                q.pop_front();
            }
            !q.is_empty()
        });
    }
}

struct MediaTicket {
    video_id: i64,
    expires: Instant,
}

fn http_loop(listener: TcpListener, cfg: Arc<ServerConfig>) {
    while !cfg.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, addr)) => {
                let ip = addr.ip();
                let ip_count = cfg
                    .per_ip_active
                    .lock()
                    .map(|m| m.get(&ip).copied().unwrap_or(0))
                    .unwrap_or(usize::MAX);
                if cfg.active.load(Ordering::SeqCst) >= MAX_CONCURRENT
                    || ip_count >= MAX_CONCURRENT_PER_IP
                {
                    let _ = write_simple(
                        &stream,
                        "503 Service Unavailable",
                        "text/plain; charset=utf-8",
                        b"too many connections",
                        false,
                        None,
                    );
                    continue;
                }
                cfg.active.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut map) = cfg.per_ip_active.lock() {
                    *map.entry(ip).or_insert(0) += 1;
                }
                let c = cfg.clone();
                // 配额守卫必须在 spawn 之前创建：spawn 失败时闭包连同守卫一起丢弃，
                // 计数能自动归还；若放到 handle_connection 内部，spawn 失败就会永久漏一个名额。
                let guard = ActiveGuard {
                    global: cfg.active.clone(),
                    per_ip: cfg.per_ip_active.clone(),
                    ip,
                };
                let _ = thread::Builder::new()
                    .name("cinevault-lan-conn".into())
                    .spawn(move || {
                        let _guard = guard;
                        let _ = handle_connection(stream, c, ip);
                    });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40));
            }
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
}

// 连接线程的 RAII 配额。spawn 失败时守卫随闭包丢弃，计数自动归还。
struct ActiveGuard {
    global: Arc<AtomicUsize>,
    per_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.global.fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut map) = self.per_ip.lock() {
            if let Some(n) = map.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    map.remove(&self.ip);
                }
            }
        }
    }
}

fn handle_connection(stream: TcpStream, cfg: Arc<ServerConfig>, peer_ip: IpAddr) -> io::Result<()> {
    // Windows 上 accept() 返回的 socket 会继承 listener 的非阻塞状态（Unix 不继承）。
    // http_loop 为了让 accept 可轮询把 listener 设成了非阻塞，这里必须显式改回阻塞：
    // 否则发送缓冲写满时 write_all 会返回 WouldBlock，被当成致命错误提前断开，
    // 表现为大文件播到几秒（约几百 KB）就断流。小响应因不填满缓冲而看不出问题。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(HEADER_DEADLINE));
    // 播放器暂停时对端会停止读取，给足 2 分钟再判定为死连接，避免暂停一会儿就断流。
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_nodelay(true);

    let mut reader = BufReader::with_capacity(4 * 1024, stream.try_clone()?);
    let req = match read_request(&mut reader, Instant::now() + HEADER_DEADLINE) {
        Ok(r) => r,
        Err(status) => {
            return write_simple(
                &stream,
                status,
                "text/plain; charset=utf-8",
                status.as_bytes(),
                false,
                None,
            );
        }
    };

    if cfg.stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    match req.method.as_str() {
        "GET" | "HEAD" => {}
        "OPTIONS" => {
            return write_simple(
                &stream,
                "204 No Content",
                "text/plain; charset=utf-8",
                b"",
                false,
                Some("Allow: GET, HEAD, OPTIONS\r\n"),
            )
        }
        _ => {
            return write_simple(
                &stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"method not allowed",
                false,
                Some("Allow: GET, HEAD, OPTIONS\r\n"),
            )
        }
    }

    let (path, query) = split_target(&req.target);
    let is_head = req.method == "HEAD";
    // 长期令牌只允许走 Authorization 头，不接受 URL 查询参数，避免进入日志/历史。
    let provided = req
        .headers
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());

    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    if path == "/" {
        return write_simple(
            &stream,
            "200 OK",
            "text/html; charset=utf-8",
            ROOT_HTML.as_bytes(),
            is_head,
            None,
        );
    }

    if path == "/api/v1/health" {
        let body = serde_json::to_vec(&serde_json::json!({
            "app": "CineVault",
            "version": APP_VERSION,
            "name": cfg.name,
        }))
        .unwrap_or_default();
        return write_json(&stream, "200 OK", &body, is_head);
    }

    if path == "/api/v1/pair" {
        let allowed = cfg
            .pair_gate
            .lock()
            .map(|mut g| g.allow(peer_ip))
            .unwrap_or(false);
        if !allowed {
            return write_error(
                &stream,
                "429 Too Many Requests",
                "配对尝试过于频繁，请稍后再试",
                is_head,
            );
        }
        // 配对码走请求头，不放进 URL，避免被浏览器历史/代理日志记录。
        let code = req
            .headers
            .get("x-cinevault-pair")
            .map(String::as_str)
            .unwrap_or("");
        let code_ok = !code.is_empty() && constant_eq(code, &cfg.pair_code);
        let fresh = unix_now() < cfg.pair_expires_at;
        if let Ok(mut gate) = cfg.pair_gate.lock() {
            if code_ok && fresh {
                gate.record_success(peer_ip);
            } else {
                gate.record_failure(peer_ip);
            }
        }
        if !fresh {
            return write_error(
                &stream,
                "410 Gone",
                "配对码已过期，请在电脑端更换配对码",
                is_head,
            );
        }
        if !code_ok {
            return write_error(&stream, "403 Forbidden", "配对码不正确", is_head);
        }
        if cfg.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let body = serde_json::to_vec(&serde_json::json!({
            "token": cfg.token,
            "port": cfg.port,
            "name": cfg.name,
        }))
        .unwrap_or_default();
        return write_json(&stream, "200 OK", &body, is_head);
    }

    // 媒体只认单视频、限时播放票据；长期令牌永远不进播放器 URL。
    if let ["api", "v1", "media", id] = segments.as_slice() {
        return match parse_id(id) {
            Some(vid) => serve_media(&stream, &cfg, vid, &query, &req, is_head),
            None => write_error(&stream, "400 Bad Request", "视频 id 不合法", is_head),
        };
    }

    if provided
        .as_deref()
        .map(|t| !constant_eq(t, &cfg.token))
        .unwrap_or(true)
    {
        return write_error(
            &stream,
            "401 Unauthorized",
            "未授权：请在 App 中配对",
            is_head,
        );
    }

    if cfg.stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    let result = match (req.method.as_str(), segments.as_slice()) {
        (_, ["api", "v1", "overview"]) => api_overview(&cfg, is_head, &stream),
        (_, ["api", "v1", "series"]) => api_series(&cfg, &query, is_head, &stream),
        (_, ["api", "v1", "series", id]) => match parse_id(id) {
            Some(sid) => api_series_detail(&cfg, sid, is_head, &stream),
            None => write_error(&stream, "400 Bad Request", "剧集 id 不合法", is_head),
        },
        (_, ["api", "v1", "videos"]) | (_, ["api", "v1", "movies"]) => {
            api_videos(&cfg, &query, is_head, &stream)
        }
        (_, ["api", "v1", "history"]) => api_history(&cfg, &query, is_head, &stream),
        (_, ["api", "v1", "media-ticket"]) => api_media_ticket(&cfg, &query, is_head, &stream),
        _ => write_error(&stream, "404 Not Found", "接口不存在", is_head),
    };
    result
}

fn read_request(
    reader: &mut BufReader<TcpStream>,
    deadline: Instant,
) -> Result<Request, &'static str> {
    let first = read_header_line(reader, deadline, MAX_HEADER_BYTES)?;
    let line = String::from_utf8(first).map_err(|_| "400 Bad Request")?;
    let mut parts = line.trim_end_matches(['\r', '\n']).split_whitespace();
    let method = parts.next().ok_or("400 Bad Request")?.to_string();
    let target = parts.next().ok_or("400 Bad Request")?.to_string();
    let version = parts.next().ok_or("400 Bad Request")?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err("400 Bad Request");
    }

    let mut headers = HashMap::new();
    let mut total = line.len();
    loop {
        let remaining = MAX_HEADER_BYTES.saturating_sub(total);
        if remaining == 0 {
            return Err("431 Request Header Fields Too Large");
        }
        let raw = read_header_line(reader, deadline, remaining)?;
        total += raw.len();
        let raw = String::from_utf8(raw).map_err(|_| "400 Bad Request")?;
        let trimmed = raw.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Ok(Request {
        method,
        target,
        headers,
    })
}

/// 读取一行，但使用整个请求头的绝对截止时间，而不是每次 read 重新计时。
/// 这样逐字节慢速发送无法把单次 read 超时无限续期。
fn read_header_line(
    reader: &mut BufReader<TcpStream>,
    deadline: Instant,
    max_bytes: usize,
) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("408 Request Timeout");
        }
        if reader.buffer().is_empty() {
            let remaining = deadline.saturating_duration_since(now);
            let _ = reader
                .get_ref()
                .set_read_timeout(Some(remaining.max(Duration::from_millis(1))));
        }
        let available = reader.fill_buf().map_err(|_| "408 Request Timeout")?;
        if available.is_empty() {
            return if out.is_empty() {
                Err("400 Bad Request")
            } else {
                Ok(out)
            };
        }
        let take = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(available.len());
        if out.len() + take > max_bytes {
            return Err("431 Request Header Fields Too Large");
        }
        out.extend_from_slice(&available[..take]);
        reader.consume(take);
        if out.last() == Some(&b'\n') {
            return Ok(out);
        }
    }
}

struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
}

// ============================================
// API 实现
// ============================================

#[derive(Serialize)]
struct MobileVideo {
    id: i64,
    title: String,
    file_name: String,
    file_size: i64,
    duration: Option<f64>,
    rating: f64,
    kind: String,
    series_id: Option<i64>,
    series_name: Option<String>,
    season: Option<i32>,
    episode: Option<i32>,
    episode_end: Option<i32>,
    last_watched_at: Option<String>,
    watch_progress: f64,
    tags: Vec<Tag>,
    available: bool,
}

#[derive(Serialize)]
struct MobileSeries {
    id: i64,
    name: String,
    total_episodes: i64,
    watched_episodes: i64,
    progress: f64,
    total_size: i64,
    total_duration: f64,
    rating: f64,
    last_watched_at: Option<String>,
    updated_at: String,
    tags: Vec<Tag>,
}

fn mobile_video(v: crate::models::VideoWithTags) -> MobileVideo {
    let available = Path::new(&v.video.file_path).is_file();
    MobileVideo {
        id: v.video.id,
        title: v.video.title,
        file_name: v.video.file_name,
        file_size: v.video.file_size,
        duration: v.video.duration,
        rating: v.video.rating,
        kind: v.video.kind,
        series_id: v.video.series_id,
        series_name: v.video.series_name,
        season: v.video.season,
        episode: v.video.episode,
        episode_end: v.video.episode_end,
        last_watched_at: v.video.last_watched_at,
        watch_progress: v.video.watch_progress,
        tags: v.tags,
        available,
    }
}

fn mobile_series(s: crate::models::SeriesOverview) -> MobileSeries {
    MobileSeries {
        id: s.id,
        name: s.name,
        total_episodes: s.total_episodes,
        watched_episodes: s.watched_episodes,
        progress: s.progress,
        total_size: s.total_size,
        total_duration: s.total_duration,
        rating: s.rating,
        last_watched_at: s.last_watched_at,
        updated_at: s.updated_at,
        tags: s.tags,
    }
}

fn open_readonly(cfg: &ServerConfig) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        &cfg.db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开数据库失败: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn api_overview(cfg: &ServerConfig, is_head: bool, stream: &TcpStream) -> io::Result<()> {
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    let stats = match db::get_stats(&tx) {
        Ok(s) => s,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    if let Err(e) = tx.commit() {
        return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head);
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "app": "CineVault",
        "version": APP_VERSION,
        "name": cfg.name,
        "stats": stats,
    }))
    .unwrap_or_default();
    write_json(stream, "200 OK", &body, is_head)
}

fn api_series(
    cfg: &ServerConfig,
    query: &HashMap<String, String>,
    is_head: bool,
    stream: &TcpStream,
) -> io::Result<()> {
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    let all = match db::get_series_overview(&tx) {
        Ok(v) => v,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    if let Err(e) = tx.commit() {
        return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head);
    }

    let terms: Vec<String> = query
        .get("q")
        .unwrap_or(&String::new())
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .collect();
    let mut items: Vec<MobileSeries> = all
        .into_iter()
        .filter(|s| {
            if terms.is_empty() {
                return true;
            }
            let hay = format!(
                "{} {}",
                s.name.to_lowercase(),
                s.tags
                    .iter()
                    .map(|t| t.name.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            terms.iter().all(|t| hay.contains(t))
        })
        .map(mobile_series)
        .collect();

    match query.get("sort").map(String::as_str).unwrap_or("recent") {
        "name" => items.sort_by_key(|s| s.name.to_lowercase()),
        "rating" => items.sort_by(|a, b| {
            b.rating
                .partial_cmp(&a.rating)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "progress" => items.sort_by(|a, b| {
            b.progress
                .partial_cmp(&a.progress)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => items.sort_by(|a, b| {
            b.last_watched_at
                .as_deref()
                .unwrap_or("")
                .cmp(a.last_watched_at.as_deref().unwrap_or(""))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
    }

    let page = query
        .get("page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(40)
        .clamp(1, API_PAGE_SIZE_MAX);
    let total = items.len() as i64;
    let offset = page.saturating_mul(limit) as usize;
    let paged: Vec<MobileSeries> = items
        .into_iter()
        .skip(offset)
        .take(limit as usize)
        .collect();
    let next_offset = page.saturating_add(1).saturating_mul(limit);
    let body = serde_json::to_vec(&serde_json::json!({
        "items": paged,
        "total": total,
        "page": page,
        "page_size": limit,
        "has_more": next_offset < total,
    }))
    .unwrap_or_default();
    write_json(stream, "200 OK", &body, is_head)
}

fn api_series_detail(
    cfg: &ServerConfig,
    series_id: i64,
    is_head: bool,
    stream: &TcpStream,
) -> io::Result<()> {
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    // 概要与单集必须来自同一读快照，避免桌面端并发扫描时返回“标题是新版、集数是旧版”的拼接。
    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    let series = match db::get_series_overview(&tx) {
        Ok(v) => v.into_iter().find(|s| s.id == series_id),
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    let Some(series) = series else {
        return write_error(stream, "404 Not Found", "剧集不存在", is_head);
    };
    let episodes = match db::get_series_episodes(&tx, series_id) {
        Ok(v) => v.into_iter().map(mobile_video).collect::<Vec<_>>(),
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    if let Err(e) = tx.commit() {
        return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head);
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "series": mobile_series(series),
        "episodes": episodes,
    }))
    .unwrap_or_default();
    write_json(stream, "200 OK", &body, is_head)
}

fn api_videos(
    cfg: &ServerConfig,
    query: &HashMap<String, String>,
    is_head: bool,
    stream: &TcpStream,
) -> io::Result<()> {
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };

    let page = query
        .get("page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(60)
        .clamp(1, API_PAGE_SIZE_MAX);
    let q = query.get("q").map(|s| s.trim()).unwrap_or("");
    let kind = query.get("kind").map(String::as_str).unwrap_or("all");
    let sort = query.get("sort").map(String::as_str).unwrap_or("recent");
    let order = query.get("order").map(String::as_str).unwrap_or("desc");

    let mut conditions = vec!["v.kind != 'series'".to_string()];
    if kind == "movie" {
        conditions.push("v.kind = 'movie'".to_string());
    }
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !q.is_empty() {
        let like = format!("%{}%", db::escape_like(q));
        let idx = params_vec.len() + 1;
        conditions.push(format!(
            "(v.title LIKE ?{0} ESCAPE '\\' OR v.file_name LIKE ?{0} ESCAPE '\\' \
             OR s.name LIKE ?{0} ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM video_tags vt_ JOIN tags t_ ON t_.id = vt_.tag_id \
                        WHERE vt_.video_id = v.id AND t_.name LIKE ?{0} ESCAPE '\\'))",
            idx
        ));
        params_vec.push(Box::new(like));
    }
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|v| v.as_ref()).collect();

    let sort_key = match sort {
        "name" | "title" => "name",
        "rating" => "rating",
        "size" => "size",
        "duration" => "duration",
        "updated" => "updated",
        _ => "last_watched",
    };
    let order = if order == "asc" { "asc" } else { "desc" };

    match db::query_videos_paginated(
        &conn,
        &conditions,
        &params_ref,
        page,
        limit,
        0.0,
        10.0,
        sort_key,
        order,
    ) {
        Ok(res) => {
            let items: Vec<MobileVideo> = res.items.into_iter().map(mobile_video).collect();
            let body = serde_json::to_vec(&serde_json::json!({
                "items": items,
                "total": res.total,
                "page": res.page,
                "page_size": res.page_size,
                "has_more": res.has_more,
            }))
            .unwrap_or_default();
            write_json(stream, "200 OK", &body, is_head)
        }
        Err(e) => write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    }
}

fn api_history(
    cfg: &ServerConfig,
    query: &HashMap<String, String>,
    is_head: bool,
    stream: &TcpStream,
) -> io::Result<()> {
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(60)
        .clamp(1, API_PAGE_SIZE_MAX);
    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    match db::get_history_entries(&tx, limit) {
        Ok(entries) => {
            if let Err(e) = tx.commit() {
                return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head);
            }
            let items: Vec<_> = entries
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "watched_at": e.watched_at,
                        "progress": e.progress,
                        "video": mobile_video(e.video),
                    })
                })
                .collect();
            let body =
                serde_json::to_vec(&serde_json::json!({ "items": items })).unwrap_or_default();
            write_json(stream, "200 OK", &body, is_head)
        }
        Err(e) => write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    }
}

fn api_media_ticket(
    cfg: &ServerConfig,
    query: &HashMap<String, String>,
    is_head: bool,
    stream: &TcpStream,
) -> io::Result<()> {
    let Some(video_id) = query.get("id").and_then(|v| parse_id(v)) else {
        return write_error(stream, "400 Bad Request", "视频 id 不合法", is_head);
    };
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    let exists: bool = match conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM videos WHERE id=?1)",
        params![video_id],
        |r| r.get(0),
    ) {
        Ok(v) => v,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    if !exists {
        return write_error(stream, "404 Not Found", "视频记录不存在", is_head);
    }

    let ticket = match random_hex(32) {
        Ok(t) => t,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    if let Ok(mut tickets) = cfg.tickets.lock() {
        let now = Instant::now();
        tickets.retain(|_, t| t.expires > now);
        // 已配对客户端也不能无限制签发票据把服务端内存吃满。
        if tickets.len() >= 4096 {
            tickets.clear();
        }
        tickets.insert(
            ticket.clone(),
            MediaTicket {
                video_id,
                expires: now + MEDIA_TICKET_TTL,
            },
        );
    } else {
        return write_error(
            stream,
            "500 Internal Server Error",
            "无法签发播放票据",
            is_head,
        );
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "ticket": ticket,
        "expires_in": MEDIA_TICKET_TTL.as_secs(),
    }))
    .unwrap_or_default();
    write_json(stream, "200 OK", &body, is_head)
}

fn serve_media(
    stream: &TcpStream,
    cfg: &ServerConfig,
    video_id: i64,
    query: &HashMap<String, String>,
    req: &Request,
    is_head: bool,
) -> io::Result<()> {
    let ticket_ok = query
        .get("ticket")
        .map(|t| {
            cfg.tickets
                .lock()
                .map(|tickets| {
                    tickets
                        .get(t)
                        .map(|v| v.video_id == video_id && v.expires > Instant::now())
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if !ticket_ok {
        return write_error(
            stream,
            "403 Forbidden",
            "播放票据无效或已过期，请回到 App 重新播放",
            is_head,
        );
    }
    let conn = match open_readonly(cfg) {
        Ok(c) => c,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e, is_head),
    };
    let row: Option<(String, String)> = match conn
        .query_row(
            "SELECT file_path, title FROM videos WHERE id = ?1",
            params![video_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    {
        Ok(v) => v,
        Err(e) => return write_error(stream, "500 Internal Server Error", &e.to_string(), is_head),
    };
    let Some((path_str, title)) = row else {
        return write_error(stream, "404 Not Found", "视频记录不存在", is_head);
    };
    let path = PathBuf::from(&path_str);
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            return write_error(
                stream,
                "404 Not Found",
                "视频文件当前不可用（外接硬盘未连接或文件已移动）",
                is_head,
            )
        }
    };
    let size = match file.metadata() {
        Ok(m) if m.is_file() => m.len(),
        _ => return write_error(stream, "404 Not Found", "视频文件不可用", is_head),
    };
    if size == 0 {
        return write_error(stream, "416 Range Not Satisfiable", "视频文件为空", is_head);
    }

    // 多段 Range 合法但服务端允许忽略；不支持的格式回 200，比 416 更兼容。
    let range = req
        .headers
        .get("range")
        .filter(|v| !v.contains(','))
        .map(|v| parse_range(v, size))
        .transpose();
    let (status, start, end) = match range {
        Ok(Some((a, b))) => ("206 Partial Content", a, b),
        Ok(None) => ("200 OK", 0, size - 1),
        Err(_) => {
            let extra = format!("Content-Range: bytes */{}\r\n", size);
            return write_simple(
                stream,
                "416 Range Not Satisfiable",
                "text/plain; charset=utf-8",
                b"range not satisfiable",
                is_head,
                Some(&extra),
            );
        }
    };
    let length = end - start + 1;
    let mime = mime_for(&path);
    let quoted = percent_encode_filename(&title);
    let extra = if status.starts_with("206") {
        format!(
            "Accept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\nContent-Disposition: inline; filename*=UTF-8''{}\r\n",
            start, end, size, quoted
        )
    } else {
        // 200 也带 Accept-Ranges，VLC 才会主动发 range。
        format!(
            "Accept-Ranges: bytes\r\nContent-Disposition: inline; filename*=UTF-8''{}\r\n",
            quoted
        )
    };

    write_simple_with_len(stream, status, mime, length, Some(&extra))?;

    if is_head {
        return Ok(());
    }
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut remaining = length;
    let mut buf = vec![0u8; 256 * 1024];
    let mut out = stream.try_clone()?;
    while remaining > 0 && !cfg.stop.load(Ordering::SeqCst) {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        // 逐段写而不是 write_all：发送缓冲暂时满（WouldBlock）只说明对端读得慢，
        // 必须保留已读数据、稍候续写，绝不能当成致命错误断开，否则长片会在中途截断。
        let mut off = 0;
        while off < n {
            match out.write(&buf[off..n]) {
                Ok(0) => return Ok(()),
                Ok(w) => off += w,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if cfg.stop.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Ok(()),
            }
        }
        remaining -= n as u64;
    }
    let _ = out.flush();
    Ok(())
}

// ============================================
// 发现服务
// ============================================

fn discovery_loop(sock: UdpSocket, cfg: Arc<ServerConfig>) {
    let mut buf = [0u8; 256];
    while !cfg.stop.load(Ordering::SeqCst) {
        match sock.recv_from(&mut buf) {
            Ok((n, addr)) => {
                let request = match std::str::from_utf8(&buf[..n]) {
                    Ok(v) => v.trim(),
                    Err(_) => continue,
                };
                let Some(nonce) = request.strip_prefix("CINEVAULT_DISCOVER_V2:") else {
                    continue;
                };
                if nonce.len() != 64 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
                    continue;
                }
                // 用配对码对手机随机数做 HMAC：不知道配对码的伪造设备无法抢先应答。
                let proof = hmac_sha256_hex(cfg.pair_code.as_bytes(), nonce.as_bytes());
                let body = serde_json::to_vec(&serde_json::json!({
                    "app": "CineVault",
                    "name": cfg.name,
                    "port": cfg.port,
                    "version": APP_VERSION,
                    "proof": proof,
                }))
                .unwrap_or_default();
                let _ = sock.send_to(&body, addr);
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue
            }
            Err(_) => thread::sleep(Duration::from_millis(300)),
        }
    }
}

// ============================================
// 工具
// ============================================

fn write_json(stream: &TcpStream, status: &str, body: &[u8], is_head: bool) -> io::Result<()> {
    write_simple(
        stream,
        status,
        "application/json; charset=utf-8",
        body,
        is_head,
        Some("Cache-Control: no-store\r\n"),
    )
}

fn write_error(stream: &TcpStream, status: &str, message: &str, is_head: bool) -> io::Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message })).unwrap_or_default();
    write_json(stream, status, &body, is_head)
}

fn write_simple(
    stream: &TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    is_head: bool,
    extra_headers: Option<&str>,
) -> io::Result<()> {
    write_simple_with_len(
        stream,
        status,
        content_type,
        body.len() as u64,
        extra_headers,
    )?;
    if !is_head {
        let mut out = stream.try_clone()?;
        out.write_all(body)?;
        out.flush()?;
    }
    Ok(())
}

/// 只写响应头。HEAD 与 GET 的头部逐字节相同（Content-Length 是资源真实长度，
/// 客户端据此探测文件大小），body 是否发送由调用方决定，故本函数不接收 is_head。
fn write_simple_with_len(
    stream: &TcpStream,
    status: &str,
    content_type: &str,
    content_length: u64,
    extra_headers: Option<&str>,
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
         X-Content-Type-Options: nosniff\r\n{}\r\n",
        status,
        content_type,
        content_length,
        extra_headers.unwrap_or("")
    );
    let mut out = stream.try_clone()?;
    out.write_all(head.as_bytes())?;
    out.flush()?;
    Ok(())
}

fn parse_range(value: &str, size: u64) -> Result<(u64, u64), ()> {
    let spec = value.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') {
        return Err(());
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    if a.is_empty() {
        let suffix: u64 = b.trim().parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let suffix = suffix.min(size);
        return Ok((size - suffix, size - 1));
    }
    let start: u64 = a.trim().parse().map_err(|_| ())?;
    if start >= size {
        return Err(());
    }
    let end: u64 = if b.trim().is_empty() {
        size - 1
    } else {
        b.trim().parse().map_err(|_| ())?
    };
    if end < start {
        return Err(());
    }
    Ok((start, end.min(size - 1)))
}

fn split_target(target: &str) -> (String, HashMap<String, String>) {
    match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (target.to_string(), HashMap::new()),
    }
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        map.insert(url_decode(k), url_decode(v));
    }
    map
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                // 必须按字节解十六进制，不能切 &s[i+1..i+3]：
                // 非法百分号后跟多字节 UTF-8 时，那个切片位置可能不在字符边界，会直接 panic。
                match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi << 4) | lo);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn parse_id(s: &str) -> Option<i64> {
    let id: i64 = s.parse().ok()?;
    (id > 0).then_some(id)
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" | "m2ts" | "mts" => "video/mp2t",
        "mpg" | "mpeg" => "video/mpeg",
        "rmvb" | "rm" => "application/vnd.rn-realmedia-vbr",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

fn percent_encode_filename(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn constant_eq(a: &str, b: &str) -> bool {
    let aa = a.as_bytes();
    let bb = b.as_bytes();
    let mut diff = aa.len() ^ bb.len();
    let n = aa.len().max(bb.len());
    for i in 0..n {
        diff |= aa.get(i).copied().unwrap_or(0) as usize ^ bb.get(i).copied().unwrap_or(0) as usize;
    }
    diff == 0
}

fn local_ipv4() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // UDP connect 不改包、不要求网络连通，只让路由表选一个出站地址。
    sock.connect(("8.8.8.8", 80)).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

fn server_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "CineVault".to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 发现协议使用 HMAC-SHA256。项目不引入新依赖，因此这里实现固定、可测试的 SHA-256。
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&sha256(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend(key_block.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);
    let mut outer = Vec::with_capacity(96);
    outer.extend(key_block.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    hmac_sha256(key, message)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn random_pair_code() -> Result<String, String> {
    let bytes = random_bytes(4)?;
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 100_000_000;
    Ok(format!("{:08}", n))
}

fn random_hex(bytes: usize) -> Result<String, String> {
    Ok(random_bytes(bytes)?
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

fn random_bytes(len: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; len];
    #[cfg(windows)]
    {
        // 直接调用 BCryptGenRandom，免去 rand 依赖。失败必须上抛，
        // 绝不能用时间/进程号拼一个“看起来随机”的弱凭据。
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG = 0x00000002
        #[link(name = "bcrypt")]
        extern "system" {
            fn BCryptGenRandom(
                h_algorithm: *mut core::ffi::c_void,
                pb_buffer: *mut u8,
                cb_buffer: u32,
                dw_flags: u32,
            ) -> i32;
        }
        let ok = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                0x0000_0002,
            ) == 0
        };
        if ok {
            return Ok(buf);
        }
        return Err("Windows 安全随机数生成失败".into());
    }
    #[cfg(not(windows))]
    {
        let mut source =
            File::open("/dev/urandom").map_err(|e| format!("安全随机源不可用: {e}"))?;
        source
            .read_exact(&mut buf)
            .map_err(|e| format!("读取安全随机数失败: {e}"))?;
        Ok(buf)
    }
}

const ROOT_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>CineVault 局域网服务</title>
<style>
body{margin:0;background:#0a0f1a;color:#e6ecf7;font-family:system-ui,sans-serif;
display:flex;min-height:100vh;align-items:center;justify-content:center}
main{max-width:420px;padding:32px;background:#111a2b;border:1px solid rgba(255,255,255,.09);
border-radius:16px}h1{font-size:20px;margin:0 0 10px;color:#f0b429}
p{color:#a3b3cd;font-size:14px;line-height:1.7;margin:8px 0}
code{background:#18233a;padding:2px 6px;border-radius:5px;color:#f0b429}
</style></head><body><main>
<h1>CineVault 局域网服务已开启</h1>
<p>这是只读接口，不提供网页媒体库。请安装 <b>CineVault 手机端</b>，在 App 中输入电脑地址与配对码。</p>
<p>电脑端「手机观看」面板会显示配对码。观看由 VLC 直接读取原始文件，不转码。</p>
<p>接口状态：<code>/api/v1/health</code></p>
</main></body></html>"#;

// ============================================
// 单元测试
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::VideoUpsertParams;
    use rusqlite::Connection;

    fn test_cfg() -> Arc<ServerConfig> {
        let path = std::env::temp_dir().join(format!(
            "cinevault-lan-test-{}-{}.db",
            std::process::id(),
            random_hex(4).expect("test random")
        ));
        let conn = Connection::open(&path).unwrap();
        db::initialize_database(&conn).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let sid = db::upsert_series(&tx, "测试剧集", None).unwrap();
        db::upsert_video(
            &tx,
            &VideoUpsertParams {
                title: "测试第一集",
                file_path: "Z:\\不存在\\测试第一集.mkv",
                file_name: "测试第一集.mkv",
                file_size: 1024,
                duration: Some(120.0),
                kind: "series",
                confidence: 90,
                user_kind: None,
                series_id: Some(sid),
                season: Some(1),
                episode: Some(1),
                episode_end: None,
                default_watched: false,
            },
            false,
        )
        .unwrap();
        tx.commit().unwrap();
        drop(conn);
        Arc::new(ServerConfig {
            db_path: path,
            token: "aabbccdd".into(),
            pair_code: "12345678".into(),
            pair_expires_at: u64::MAX,
            port: 1,
            name: "TEST".into(),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            per_ip_active: Arc::new(Mutex::new(HashMap::new())),
            pair_gate: Mutex::new(PairGate::default()),
            tickets: Mutex::new(HashMap::new()),
        })
    }

    fn request(addr: std::net::SocketAddr, raw: &str) -> String {
        String::from_utf8_lossy(&request_bytes(addr, raw)).into_owned()
    }

    fn request_bytes(addr: std::net::SocketAddr, raw: &str) -> Vec<u8> {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        out
    }

    /// 测试库使用 WAL，删主文件不够：-wal/-shm 也要一起清，避免临时目录留垃圾。
    fn cleanup_db(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{}", path.display(), suffix));
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn parses_single_and_suffix_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Ok((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Ok((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Ok((900, 999)));
        assert!(parse_range("bytes=1000-", 1000).is_err());
        assert!(parse_range("bytes=0-1,2-3", 1000).is_err());
    }

    #[test]
    fn request_header_has_absolute_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let _client = TcpStream::connect(addr).unwrap();
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let started = Instant::now();
        let err = read_request(&mut reader, Instant::now() + Duration::from_millis(80));
        assert!(matches!(err, Err("408 Request Timeout")));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn query_decoding_handles_chinese() {
        let q = parse_query("q=%E5%A4%A7%E6%98%8E&page=1");
        assert_eq!(q.get("q").map(String::as_str), Some("大明"));
        assert_eq!(q.get("page").map(String::as_str), Some("1"));
        // 非法百分号后跟多字节 UTF-8 不能 panic（旧实现按字符串下标切片会崩）。
        assert_eq!(url_decode("%你"), "%你");
    }

    #[test]
    fn api_requires_token_and_returns_series() {
        let cfg = test_cfg();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let c = cfg.clone();
        let h = thread::spawn(move || http_loop(listener, c));

        let unauthorized = request(
            addr,
            "GET /api/v1/series HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
        );
        assert!(unauthorized.starts_with("HTTP/1.1 401"), "{unauthorized}");

        let query_token = request(
            addr,
            "GET /api/v1/series?token=aabbccdd HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
        );
        assert!(query_token.starts_with("HTTP/1.1 401"), "{query_token}");

        let ok = request(
            addr,
            "GET /api/v1/series HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer aabbccdd\r\nConnection: close\r\n\r\n",
        );
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("测试剧集"), "{ok}");
        assert!(!ok.contains("Access-Control-Allow-Origin"), "{ok}");

        let huge_page = request(
            addr,
            "GET /api/v1/series?page=9223372036854775807&limit=200 HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer aabbccdd\r\nConnection: close\r\n\r\n",
        );
        assert!(huge_page.starts_with("HTTP/1.1 200"), "{huge_page}");
        let huge_page2 = request(
            addr,
            "GET /api/v1/videos?page=9223372036854775807&limit=200 HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer aabbccdd\r\nConnection: close\r\n\r\n",
        );
        assert!(huge_page2.starts_with("HTTP/1.1 200"), "{huge_page2}");

        let pair = request(
            addr,
            "GET /api/v1/pair HTTP/1.1\r\nHost: test\r\nX-CineVault-Pair: 12345678\r\nConnection: close\r\n\r\n",
        );
        assert!(pair.starts_with("HTTP/1.1 200"), "{pair}");
        assert!(pair.contains("\"token\":\"aabbccdd\""), "{pair}");

        cfg.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
        cleanup_db(&cfg.db_path);
    }

    #[test]
    fn hmac_sha256_matches_known_vector() {
        // RFC 4231 test case 2
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn discovery_proves_pair_code_without_echoing_it() {
        let cfg = test_cfg();
        let nonce = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let addr = server.local_addr().unwrap();
        let c = cfg.clone();
        let h = thread::spawn(move || discovery_loop(server, c));

        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .send_to(format!("CINEVAULT_DISCOVER_V2:{}", nonce).as_bytes(), addr)
            .unwrap();
        let mut buf = [0u8; 2048];
        let (n, _) = client.recv_from(&mut buf).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
        assert_eq!(body["app"], "CineVault");
        assert_eq!(body["port"], 1);
        assert_eq!(
            body["proof"].as_str().unwrap(),
            hmac_sha256_hex(b"12345678", nonce.as_bytes())
        );
        assert!(!String::from_utf8_lossy(&buf[..n]).contains("12345678"));

        cfg.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
        cleanup_db(&cfg.db_path);
    }

    #[test]
    fn expired_pair_code_is_rejected_and_rate_limit_closes_the_gate() {
        let cfg = test_cfg();
        // 单独构造配置，避免修改共享 Arc 的不可变字段。
        let expired = Arc::new(ServerConfig {
            db_path: cfg.db_path.clone(),
            token: cfg.token.clone(),
            pair_code: cfg.pair_code.clone(),
            pair_expires_at: 0,
            port: 1,
            name: "TEST".into(),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            per_ip_active: Arc::new(Mutex::new(HashMap::new())),
            pair_gate: Mutex::new(PairGate::default()),
            tickets: Mutex::new(HashMap::new()),
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let c = expired.clone();
        let h = thread::spawn(move || http_loop(listener, c));
        let res = request(
            addr,
            "GET /api/v1/pair HTTP/1.1\r\nHost: test\r\nX-CineVault-Pair: 12345678\r\nConnection: close\r\n\r\n",
        );
        assert!(res.starts_with("HTTP/1.1 410"), "{res}");
        expired.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();

        let limited = test_cfg();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let c = limited.clone();
        let h = thread::spawn(move || http_loop(listener, c));
        for _ in 0..PAIR_MAX_PER_IP {
            let res = request(
                addr,
                "GET /api/v1/pair HTTP/1.1\r\nHost: test\r\nX-CineVault-Pair: 00000000\r\nConnection: close\r\n\r\n",
            );
            assert!(res.starts_with("HTTP/1.1 403"), "{res}");
        }
        let res = request(
            addr,
            "GET /api/v1/pair HTTP/1.1\r\nHost: test\r\nX-CineVault-Pair: 00000000\r\nConnection: close\r\n\r\n",
        );
        assert!(res.starts_with("HTTP/1.1 429"), "{res}");
        limited.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
        cleanup_db(&cfg.db_path);
    }

    /// 真实生产库只读回放（默认忽略，避免 CI 依赖本机数据）。
    /// 运行：CINEVAULT_TEST_DB="<path>" cargo test real_database -- --ignored --nocapture
    /// 全程 READ_ONLY，不启动 Tauri 窗口、不写库。
    #[test]
    #[ignore]
    fn real_database_read_only_smoke() {
        let db_path = std::env::var("CINEVAULT_TEST_DB").expect("CINEVAULT_TEST_DB 未设置");
        assert!(Path::new(&db_path).is_file());
        let cfg = Arc::new(ServerConfig {
            db_path: PathBuf::from(&db_path),
            token: "real-smoke-token".into(),
            pair_code: "87654321".into(),
            pair_expires_at: u64::MAX,
            port: 1,
            name: "REAL".into(),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            per_ip_active: Arc::new(Mutex::new(HashMap::new())),
            pair_gate: Mutex::new(PairGate::default()),
            tickets: Mutex::new(HashMap::new()),
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let c = cfg.clone();
        let h = thread::spawn(move || http_loop(listener, c));

        let overview = request(
            addr,
            "GET /api/v1/overview HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer real-smoke-token\r\nConnection: close\r\n\r\n",
        );
        assert!(overview.starts_with("HTTP/1.1 200"), "{overview}");
        let ov_body = overview.split("\r\n\r\n").nth(1).unwrap_or("");
        let ov: serde_json::Value = serde_json::from_str(ov_body).expect("overview JSON");
        let total_videos = ov["stats"]["total_videos"].as_i64().unwrap_or(0);
        let total_series = ov["stats"]["total_series"].as_i64().unwrap_or(0);
        assert!(total_videos > 0 && total_series > 0, "{ov}");
        println!("真实库概览: {} 视频 / {} 剧集", total_videos, total_series);

        let series = request(
            addr,
            "GET /api/v1/series HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer real-smoke-token\r\nConnection: close\r\n\r\n",
        );
        assert!(series.starts_with("HTTP/1.1 200"), "{series}");
        let sj: serde_json::Value =
            serde_json::from_str(series.split("\r\n\r\n").nth(1).unwrap_or(""))
                .expect("series JSON");
        let items = sj["items"].as_array().expect("series items");
        assert!(!items.is_empty());
        let first_id = items[0]["id"].as_i64().expect("series id");
        println!("真实库剧集数: {}，首项: {}", items.len(), items[0]["name"]);

        let detail = request(
            addr,
            &format!(
                "GET /api/v1/series/{} HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer real-smoke-token\r\nConnection: close\r\n\r\n",
                first_id
            ),
        );
        assert!(detail.starts_with("HTTP/1.1 200"), "{detail}");
        let dj: serde_json::Value =
            serde_json::from_str(detail.split("\r\n\r\n").nth(1).unwrap_or(""))
                .expect("detail JSON");
        assert!(dj["episodes"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false));
        println!(
            "真实库剧集详情: {}，单集 {} 集",
            dj["series"]["name"].as_str().unwrap_or(""),
            dj["episodes"].as_array().map(|a| a.len()).unwrap_or(0)
        );

        let movies = request(
            addr,
            "GET /api/v1/videos?kind=movie&limit=50&page=0 HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer real-smoke-token\r\nConnection: close\r\n\r\n",
        );
        assert!(movies.starts_with("HTTP/1.1 200"), "{movies}");
        let mj: serde_json::Value =
            serde_json::from_str(movies.split("\r\n\r\n").nth(1).unwrap_or(""))
                .expect("videos JSON");
        assert!(mj["items"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false));
        println!(
            "真实库电影分页: total={} 返回={} has_more={}",
            mj["total"],
            mj["items"].as_array().map(|a| a.len()).unwrap_or(0),
            mj["has_more"]
        );

        // 从可用记录里挑一条，只取前 1KB，验证真实文件路径 + Range 响应，不读整部片。
        let sample_id = mj["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["available"].as_bool().unwrap_or(false))
            .and_then(|v| v["id"].as_i64());
        if let Some(vid) = sample_id {
            let ticket_res = request(
                addr,
                &format!(
                    "GET /api/v1/media-ticket?id={} HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer real-smoke-token\r\nConnection: close\r\n\r\n",
                    vid
                ),
            );
            assert!(ticket_res.starts_with("HTTP/1.1 200"), "{ticket_res}");
            let tj: serde_json::Value =
                serde_json::from_str(ticket_res.split("\r\n\r\n").nth(1).unwrap_or(""))
                    .expect("ticket JSON");
            let ticket = tj["ticket"].as_str().expect("ticket");
            let raw = format!(
                "GET /api/v1/media/{}?ticket={} HTTP/1.1\r\nHost: test\r\nRange: bytes=0-1023\r\nConnection: close\r\n\r\n",
                vid, ticket
            );
            let media = request_bytes(addr, &raw);
            let split = media
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .expect("HTTP header separator");
            let head = String::from_utf8_lossy(&media[..split]);
            assert!(
                head.starts_with("HTTP/1.1 206"),
                "{}",
                &head[..head.len().min(400)]
            );
            assert!(head.contains("Content-Range: bytes 0-1023/"));
            assert_eq!(media.len() - split, 1024);
            println!("真实文件串流: video_id={} 首段 1024 字节返回正确", vid);
        } else {
            println!("真实文件串流: 前 50 条电影均不可用，跳过（外接盘未连接时不伪造成功）");
        }

        cfg.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
    }

    #[test]
    fn media_range_is_byte_exact() {
        let cfg = test_cfg();
        // 真实文件：用测试数据库里的假路径会 404，所以单独建一个临时视频文件。
        let video = std::env::temp_dir().join(format!(
            "cinevault-lan-media-{}.mp4",
            random_hex(4).expect("test random")
        ));
        std::fs::write(&video, b"0123456789abcdef").unwrap();
        let conn = Connection::open(&cfg.db_path).unwrap();
        let vid: i64 = conn
            .query_row("SELECT id FROM videos LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "UPDATE videos SET file_path=?1, file_name='x.mp4' WHERE id=?2",
            params![video.to_string_lossy().to_string(), vid],
        )
        .unwrap();
        drop(conn);

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let c = cfg.clone();
        let h = thread::spawn(move || http_loop(listener, c));

        let denied = request(
            addr,
            &format!(
                "GET /api/v1/media/{}?ticket=forged HTTP/1.1\r\nHost: test\r\nRange: bytes=0-1\r\nConnection: close\r\n\r\n",
                vid
            ),
        );
        assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");

        let ticket_res = request(
            addr,
            &format!(
                "GET /api/v1/media-ticket?id={} HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer aabbccdd\r\nConnection: close\r\n\r\n",
                vid
            ),
        );
        assert!(ticket_res.starts_with("HTTP/1.1 200"), "{ticket_res}");
        let tj: serde_json::Value =
            serde_json::from_str(ticket_res.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap();
        let ticket = tj["ticket"].as_str().unwrap();
        let raw = format!(
            "GET /api/v1/media/{}?ticket={} HTTP/1.1\r\nHost: test\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n",
            vid, ticket
        );
        let res = request(addr, &raw);
        assert!(res.starts_with("HTTP/1.1 206"), "{res}");
        assert!(res.contains("Content-Range: bytes 4-7/16"), "{res}");
        assert!(res.ends_with("4567"), "{res}");

        let multi = request(
            addr,
            &format!(
                "GET /api/v1/media/{}?ticket={} HTTP/1.1\r\nHost: test\r\nRange: bytes=0-1,4-5\r\nConnection: close\r\n\r\n",
                vid, ticket
            ),
        );
        assert!(multi.starts_with("HTTP/1.1 200"), "{multi}");
        assert!(multi.ends_with("0123456789abcdef"), "{multi}");

        cfg.stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
        let _ = std::fs::remove_file(&video);
        cleanup_db(&cfg.db_path);
    }
}
