use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, image::Image,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::ip;

/// 后台网卡轮询间隔。
///
/// getifaddrs + 默认路由查询均为毫秒级本地调用；网络状态一旦变化
/// （开机后拿到 DHCP、切换 Wi-Fi、VPN 拨号/断开），最迟在该间隔内自动刷新。
const NETWORK_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Shared state holding the currently displayed IP text.
#[derive(Clone)]
pub struct AppState {
    pub current_ip: Arc<Mutex<String>>,
    pub launch_at_login: Arc<Mutex<bool>>,
    /// 托盘菜单当前展示的本地网卡快照。菜单 ID 中的下标始终对应此快照，
    /// 避免菜单重建后下标错位、点击 A 网卡却复制了 B 网卡的 IP。
    pub local_ifaces: Arc<Mutex<Vec<(String, String)>>>,
    /// 最近一次生效的主 IP（默认路由网卡 IPv4），用于检测网络变化。
    pub last_primary_ip: Arc<Mutex<String>>,
}

/// Creates the application system tray with menu.
pub fn create_tray(app: &AppHandle) -> Result<TrayIcon, Box<dyn std::error::Error>> {
    // Determine initial launch-at-login state
    let initial_autostart = app.autolaunch().is_enabled().unwrap_or(false);

    // 首次扫描网卡。开机自启动时 Wi-Fi 往往尚未完成 DHCP，此处可能暂时
    // 拿不到 IPv4 —— 后台轮询线程会在拿到租约后自动刷新菜单与标题。
    let local_ifaces = ip::get_local_ips();
    let primary = local_ifaces.iter().find(|i| i.primary).cloned();
    let initial_ip = primary.as_ref().map(|p| p.ip.clone()).unwrap_or_default();
    let initial_title: String = if initial_ip.is_empty() {
        "无连接".to_string()
    } else {
        initial_ip.clone()
    };

    let snapshot: Vec<(String, String)> = local_ifaces
        .iter()
        .map(|i| (i.name.clone(), i.ip.clone()))
        .collect();

    // Initialize shared state
    app.manage(AppState {
        current_ip: Arc::new(Mutex::new(initial_ip.clone())),
        launch_at_login: Arc::new(Mutex::new(initial_autostart)),
        local_ifaces: Arc::new(Mutex::new(snapshot)),
        last_primary_ip: Arc::new(Mutex::new(initial_ip)),
    });

    let menu = build_tray_menu(app, &local_ifaces)?;

    // Load tray icon
    let icon_bytes = include_bytes!("../icons/tray-icon.png");
    let icon = Image::from_bytes(icon_bytes)?;

    // Create tray
    #[allow(unused_mut)]
    let mut tray_builder = TrayIconBuilder::with_id("main-tray")
        .tooltip(match &primary {
            Some(p) => format!("本地 IP ({}): {}", p.name, p.ip),
            None => "ip地址（无网络连接）".to_string(),
        })
        .icon(icon)
        .menu(&menu)
        .title(initial_title)
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id.as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app_handle = tray.app_handle().clone();
                let worker = app_handle.clone();
                std::thread::spawn(move || {
                    update_public_ip(&worker);
                });
                // Windows：左键点击同时唤出 IP 浮窗（浮窗被收起后的唯一唤出入口）
                crate::pill::show(&app_handle);
            }
        });

    // icon_as_template is macOS-only
    #[cfg(target_os = "macos")]
    {
        tray_builder = tray_builder.icon_as_template(true);
    }

    let tray = tray_builder.build(app)?;

    // 后台轮询网卡与默认路由变化，自动刷新菜单并纠正托盘显示的主 IP
    spawn_network_monitor(app.clone());

    Ok(tray)
}

/// 根据网卡快照构建托盘菜单（首次创建与后续重建共用此逻辑）。
fn build_tray_menu(
    app: &AppHandle,
    ifaces: &[ip::LocalIface],
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    // Build the local IP submenu items。主 IP 条目前加 ✓ 标识
    let local_ip_items: Vec<MenuItem<tauri::Wry>> = ifaces
        .iter()
        .enumerate()
        .map(|(idx, iface)| {
            let label = if iface.primary {
                format!("✓ {} ({})", iface.name, iface.ip)
            } else {
                format!("{} ({})", iface.name, iface.ip)
            };
            MenuItem::with_id(app, &format!("local_ip_{}", idx), &label, true, None::<&str>)
                .expect("failed to create menu item")
        })
        .collect();

    let local_ip_items_ref: Vec<&dyn IsMenuItem<tauri::Wry>> = local_ip_items
        .iter()
        .map(|i| i as &dyn IsMenuItem<tauri::Wry>)
        .collect();

    let local_ip_submenu = Submenu::with_items(app, "本地 IP", true, &local_ip_items_ref)?;

    // Build menu items
    let public_ip_item = MenuItem::with_id(app, "public_ip", "公网 IP", true, None::<&str>)?;
    let show_window_item = MenuItem::with_id(app, "show_ip", "显示 IP 窗口", true, None::<&str>)?;
    let about_item = MenuItem::with_id(
        app,
        "about",
        format!("关于 ip地址 v{}", env!("CARGO_PKG_VERSION")),
        true,
        None::<&str>,
    )?;
    let copy_item = MenuItem::with_id(app, "copy", "复制当前 IP", true, None::<&str>)?;

    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart_item = CheckMenuItem::with_id(
        app,
        "launch_at_login",
        "开机启动",
        true,
        autostart_enabled,
        None::<&str>,
    )?;

    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;

    let menu_items: &[&dyn IsMenuItem<tauri::Wry>] = &[
        &public_ip_item,
        &local_ip_submenu,
        &sep1,
        &copy_item,
        &sep2,
        &show_window_item,
        &about_item,
        &sep3,
        &autostart_item,
        &sep4,
        &quit_item,
    ];
    let menu = Menu::with_items(app, menu_items)?;
    Ok(menu)
}

/// 在主线程上用最新网卡快照重建托盘菜单并同步状态。
fn rebuild_menu_and_state(app: &AppHandle, ifaces: &[ip::LocalIface]) {
    let menu = match build_tray_menu(app, ifaces) {
        Ok(menu) => menu,
        Err(e) => {
            eprintln!("rebuild tray menu failed: {e}");
            return;
        }
    };

    // 先更新快照再挂载菜单，确保点击事件到达时下标映射已一致
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut snapshot) = state.local_ifaces.lock() {
            *snapshot = ifaces
                .iter()
                .map(|i| (i.name.clone(), i.ip.clone()))
                .collect();
        }
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_menu(Some(menu));
    }
}

/// 后台轮询：网络变化时刷新本地 IP 菜单；主 IP 变化时无条件纠正托盘标题。
fn spawn_network_monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(NETWORK_POLL_INTERVAL);

        let fresh = ip::get_local_ips();
        let new_snapshot: Vec<(String, String)> = fresh
            .iter()
            .map(|i| (i.name.clone(), i.ip.clone()))
            .collect();
        let new_primary = fresh
            .iter()
            .find(|i| i.primary)
            .map(|i| (i.name.clone(), i.ip.clone()));
        let new_primary_ip = new_primary
            .as_ref()
            .map(|(_, ip)| ip.clone())
            .unwrap_or_default();

        // 与当前状态比较（仅读，UI 操作统一切到主线程执行）
        let (snapshot_changed, old_primary_ip) = {
            match app.try_state::<AppState>() {
                Some(state) => {
                    let snapshot = state.local_ifaces.lock().map(|s| s.clone()).unwrap_or_default();
                    let old = state
                        .last_primary_ip
                        .lock()
                        .map(|s| s.clone())
                        .unwrap_or_default();
                    (snapshot != new_snapshot, old)
                }
                None => continue,
            }
        };
        let primary_changed = old_primary_ip != new_primary_ip;

        if !snapshot_changed && !primary_changed {
            continue;
        }

        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if primary_changed {
                // 核心修复点：开机后 DHCP 完成、切换 Wi-Fi、VPN 拨号/断开等导致
                // 默认路由主 IP 变化时，无论用户此前是否手动选过其他地址，
                // 都把托盘标题纠正为最新的主 IP。
                if let Some(tray) = app2.tray_by_id("main-tray") {
                    match &new_primary {
                        Some((name, ip)) => {
                            let _ = tray.set_title(Some(ip));
                            let _ = tray.set_tooltip(Some(&format!("本地 IP ({}): {}", name, ip)));
                        }
                        None => {
                            let _ = tray.set_title(Some("无连接"));
                            let _ = tray.set_tooltip(Some("ip地址（无网络连接）"));
                        }
                    }
                }
                if let Some(state) = app2.try_state::<AppState>() {
                    if let Ok(mut current) = state.current_ip.lock() {
                        *current = new_primary_ip.clone();
                    }
                    if let Ok(mut last) = state.last_primary_ip.lock() {
                        *last = new_primary_ip.clone();
                    }
                }
                // 网络切换后同步 Windows 浮窗显示，避免与托盘标题不一致
                crate::pill::broadcast(&app2, &new_primary_ip);
            }
            if snapshot_changed {
                rebuild_menu_and_state(&app2, &fresh);
            }
        });
    });
}

fn handle_menu_event(app: &AppHandle, event_id: &str) {
    match event_id {
        "public_ip" => {
            let app_handle = app.clone();
            std::thread::spawn(move || {
                update_public_ip(&app_handle);
            });
            // Also show the window
            show_ip_window(app);
        }
        "show_ip" => {
            show_ip_window(app);
        }
        "about" => {
            // 先把主窗口唤到前台，再通知前端弹出"关于"对话框
            show_ip_window(app);
            let _ = app.emit("show-about", ());
        }
        "copy" => {
            let app_handle = app.clone();
            std::thread::spawn(move || {
                copy_current_ip(&app_handle);
            });
        }
        "launch_at_login" => {
            toggle_launch_at_login(app);
        }
        "quit" => {
            app.exit(0);
        }
        _ if event_id.starts_with("local_ip_") => {
            if let Some(idx_str) = event_id.strip_prefix("local_ip_") {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let app_handle = app.clone();
                    std::thread::spawn(move || {
                        select_local_ip(&app_handle, idx);
                    });
                }
            }
        }
        _ => {}
    }
}

fn show_ip_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_launch_at_login(app: &AppHandle) {
    let manager = app.autolaunch();
    let current = manager.is_enabled().unwrap_or(false);

    let result = if current {
        manager.disable()
    } else {
        manager.enable()
    };

    match result {
        Ok(()) => {
            let new_state = !current;
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut flag) = state.launch_at_login.lock() {
                    *flag = new_state;
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to toggle launch at login: {}", e);
        }
    }
}

fn update_public_ip(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_title(Some("加载中..."));
    }

    match ip::fetch_public_ip() {
        Ok(public_ip) => {
            if let Some(tray) = app.tray_by_id("main-tray") {
                let _ = tray.set_title(Some(&public_ip));
                let _ = tray.set_tooltip(Some(&format!("公网 IP: {}", public_ip)));
            }
            // Store in state
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut ip) = state.current_ip.lock() {
                    *ip = public_ip.clone();
                }
            }
            crate::pill::broadcast(app, &public_ip);
        }
        Err(_) => {
            if let Some(tray) = app.tray_by_id("main-tray") {
                let _ = tray.set_title(Some("无连接"));
            }
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut ip) = state.current_ip.lock() {
                    *ip = String::new();
                }
            }
            crate::pill::broadcast(app, "");
        }
    }
}

fn copy_current_ip(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(ip) = state.current_ip.lock() {
            let ip = ip.trim().to_string();
            if !ip.is_empty() {
                let _ = app.clipboard().write_text(&ip);
            }
        }
    }
}

fn select_local_ip(app: &AppHandle, index: usize) {
    // 必须读取菜单快照而非重新扫描：菜单重建前后网卡顺序可能变化，
    // 用实时列表按下标取值会导致"点的是 A 网卡，复制/显示的却是 B 网卡 IP"。
    let selected = app
        .try_state::<AppState>()
        .and_then(|state| state.local_ifaces.lock().ok()?.get(index).cloned());

    if let Some((name, ip)) = selected {
        if let Some(tray) = app.tray_by_id("main-tray") {
            let _ = tray.set_title(Some(&ip));
            let _ = tray.set_tooltip(Some(&format!("本地 IP ({}): {}", name, ip)));
        }
        // Store in state
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut stored_ip) = state.current_ip.lock() {
                *stored_ip = ip.clone();
            }
        }
        crate::pill::broadcast(app, &ip);
    }
}
