//! Windows 端 IP 浮窗。
//!
//! 背景：Tauri 的 `TrayIcon::set_title` 在 Windows 上是 Unsupported（
//! Windows 通知区域只支持图标，不支持在图标旁显示文字），Linux 也需要配合
//! 图标才可能显示。若不做处理，Windows 用户装完后只能在鼠标悬浮 tooltip 里
//! 看到 IP，与"菜单栏常显 IP"的产品定位不符。
//!
//! 方案：在 Windows 上创建一个无边框、置顶、不占任务栏的常驻小窗，实时显示
//! 当前 IP，点击右侧按钮复制。macOS 有原生托盘标题，不创建该窗口。

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

/// 浮窗 label，与 capabilities/default.json 中的 windows 白名单保持一致。
pub const PILL_LABEL: &str = "pill";

/// 浮窗尺寸（逻辑像素），需与 public/pill.html 的布局一致。
/// 宽度按最长 IPv4（255.255.255.255）+ 圆点 + 两个按钮估算，留有余量。
const PILL_WIDTH: f64 = 248.0;
const PILL_HEIGHT: f64 = 54.0;

/// 是否启用浮窗。
///
/// 用运行时 `cfg!` 而非 `#[cfg]`，保证这段代码在所有平台都参与编译，
/// 避免 Windows 专属代码在 macOS 上长期得不到编译校验。
fn enabled() -> bool {
    cfg!(target_os = "windows")
        // 本地预览开关：macOS 上执行 `IPMENUBAR_PILL_PREVIEW=1 ./ipmenubar` 可查看布局
        || std::env::var("IPMENUBAR_PILL_PREVIEW").is_ok()
}

/// 创建浮窗（幂等）。应在 `setup` 中、托盘创建之后调用。
pub fn ensure(app: &AppHandle) -> tauri::Result<()> {
    if !enabled() {
        return Ok(());
    }
    if app.get_webview_window(PILL_LABEL).is_some() {
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(app, PILL_LABEL, WebviewUrl::App("pill.html".into()))
        .title("IP")
        .inner_size(PILL_WIDTH, PILL_HEIGHT)
        .resizable(false)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false);

    // 不使用透明窗口：Windows 上 WebView2 + 分层窗口曾出现"内容已渲染但屏幕不可见"
    // 的问题，改为不透明窗口，由 public/pill.html 自己铺满深色底色，
    // 代价是四角为直角而非圆角。
    let window = builder.build()?;

    place_bottom_right(&window);
    window.show()?;
    Ok(())
}

/// 把窗口放到主屏工作区右下角（工作区已扣除任务栏，无需硬编码任务栏高度）。
fn place_bottom_right(window: &WebviewWindow) {
    let monitor = match window.primary_monitor() {
        Ok(Some(monitor)) => monitor,
        _ => return,
    };

    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let width = (PILL_WIDTH * scale).round() as i32;
    let height = (PILL_HEIGHT * scale).round() as i32;
    let margin = (14.0 * scale).round() as i32;

    let x = work.position.x + work.size.width as i32 - width - margin;
    let y = work.position.y + work.size.height as i32 - height - margin;

    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// 显示浮窗；用户点浮窗的「×」收起后，点托盘图标可重新唤出。
pub fn show(app: &AppHandle) {
    if !enabled() {
        return;
    }
    if let Some(window) = app.get_webview_window(PILL_LABEL) {
        let _ = window.show();
    }
}

/// 把最新的 IP 推给浮窗。浮窗加载完成时也会主动 `get_current_ip` 拉一次，
/// 所以这里不必关心首次 emit 是否早于前端监听注册。
pub fn broadcast(app: &AppHandle, ip: &str) {
    if !enabled() {
        return;
    }
    let _ = app.emit("ip:update", serde_json::json!({ "ip": ip }));
}
