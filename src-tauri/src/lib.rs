mod ip;
mod pill;
mod tray;

use tauri::Manager;

/// 获取公网 IP（异步命令）。
///
/// 关键修复：ureq 是同步阻塞 HTTP 客户端，必须放入 spawn_blocking
/// 线程池执行。旧版为同步命令，会运行在 Tauri 主线程上，网络超时时
/// 直接卡死 UI 与 IPC 回调，表现为公网 IP 永远"正在获取..."。
#[tauri::command]
async fn get_public_ip() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(ip::fetch_public_ip)
        .await
        .map_err(|e| format!("公网IP查询任务执行失败: {e}"))?
}

/// 获取本机网卡 IP 列表（异步命令）。
///
/// 独立于公网 IP 命令，使本地网卡扫描（系统调用、毫秒级返回）
/// 不再被缓慢的公网请求拖慢。返回结构：
/// [{ "name": 网卡名, "ip": 地址, "primary": 是否默认路由主网卡 }]
/// 列表已按"主 IPv4 → 物理 IPv4 → 虚拟 IPv4 → IPv6"排序，首条即当前主 IP。
#[tauri::command]
async fn get_local_ifaces() -> Vec<serde_json::Value> {
    tauri::async_runtime::spawn_blocking(|| {
        ip::get_local_ips()
            .into_iter()
            .map(|iface| {
                serde_json::json!({
                    "name": iface.name,
                    "ip": iface.ip,
                    "primary": iface.primary,
                })
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 返回应用版本号，取自 Cargo.toml 的 package.version（编译期注入）。
/// 版本递进只需修改 Cargo.toml 一处，界面与托盘自动同步，避免多处硬编码不一致。
#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn copy_to_clipboard(app_handle: tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app_handle
        .clipboard()
        .write_text(text)
        .map_err(|e| e.to_string())
}

/// 返回当前展示的 IP（托盘中当前显示的那一条）。
///
/// Windows 浮窗（pill）首次加载时调用，避免依赖"事件早于监听注册"的时序。
#[tauri::command]
fn get_current_ip(app_handle: tauri::AppHandle) -> String {
    match app_handle.try_state::<tray::AppState>() {
        Some(state) => state
            .current_ip
            .lock()
            .map(|ip| ip.clone())
            .unwrap_or_default(),
        None => String::new(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_public_ip,
            get_local_ifaces,
            get_app_version,
            get_current_ip,
            copy_to_clipboard
        ]);

    #[cfg(desktop)]
    let builder = builder.setup(|app| {
        // Hide dock icon on macOS — run as agent
        #[cfg(target_os = "macos")]
        {
            use objc2::runtime::AnyObject;
            use objc2::{class, msg_send};
            unsafe {
                let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![ns_app, setActivationPolicy: 1i64];
            }
        }

        // Create the tray
        let _tray = tray::create_tray(app.handle())?;

        // Windows 端托盘无法显示文字，创建一个常驻 IP 浮窗；macOS 不创建。
        // 失败只影响浮窗，不应阻断应用启动。
        if let Err(e) = pill::ensure(app.handle()) {
            eprintln!("create ip pill window failed: {e}");
        }

        // Auto-show the main window on launch so user sees IP info immediately
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }

        Ok(())
    });

    #[cfg(not(desktop))]
    let builder = builder.setup(|app| {
        // Create the tray (mobile fallback)
        let _tray = tray::create_tray(app.handle())?;
        Ok(())
    });

    builder
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关闭窗口时只隐藏，不退出应用（应用继续驻留托盘）
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running ip地址");
}
