# IPMenuBar (Tauri Edition)

一个基于 **Tauri 2 + Rust** 的跨平台的菜单栏 / 系统托盘应用，用于显示你的 IP 地址。支持 **macOS** 和 **Windows** 多端安装使用。

## 功能特性

- **公网 IP** — 通过 [ipify.org](https://api64.ipify.org) 获取公网 IPv4/IPv6 地址
- **本地 IP** — 枚举网络接口，显示本地 IP 列表
- **复制到剪贴板** — 一键复制当前显示的 IP
- **托盘原生** — 无窗口、无 Dock 图标，驻留系统托盘/菜单栏
- **即时刷新** — 左键点击托盘图标刷新公网 IP

## 项目结构

```
desktop/
├── src-tauri/                  # Rust 后端
│   ├── src/
│   │   ├── main.rs             # 入口点
│   │   ├── lib.rs              # Tauri builder 和初始化
│   │   ├── tray.rs             # 系统托盘创建与菜单事件处理
│   │   └── ip.rs               # IP 获取逻辑(公网 + 本地)
│   ├── icons/                  # 应用图标(各尺寸 PNG)
│   ├── Cargo.toml              # Rust 依赖配置
│   ├── build.rs                # Tauri 构建脚本
│   └── tauri.conf.json         # Tauri 配置文件
├── public/
│   └── index.html              # 最小化前端(托盘应用)
└── README.md                   # 本文件
```

## 环境要求

- [Rust](https://rustup.rs/) 稳定版 (1.70+)
- [Tauri CLI v2](https://v2.tauri.app/start/prerequisites/)
- 平台特定工具:
  - **macOS**: Xcode Command Line Tools
  - **Windows**: MSVC Build Tools 或 GNU 工具链 + WebView2

## 开发运行

```bash
cd desktop
cargo tauri dev
```

以开发模式启动应用，支持前端和 Rust 代码热重载。

## 构建发布包

### macOS (.app + .dmg)

```bash
cd desktop
cargo tauri build
```

输出产物:
- `src-tauri/target/release/bundle/macos/IPMenuBar.app` — 应用 Bundle
- `src-tauri/target/release/bundle/dmg/IPMenuBar_1.0.0_aarch64.dmg` — DMG 安装镜像

### Windows (.msi / .exe)

```bash
cd desktop
cargo tauri build
```

输出产物:
- `src-tauri/target/release/bundle/msi/IPMenuBar_1.0.0_x64_en-US.msi` — MSI 安装包

### 构建所有平台目标

修改 `tauri.conf.json` 中的 `bundle.targets` 字段:
```json
"bundle": {
  "targets": ["dmg", "msi", "app"]
}
```

## 平台说明

### macOS
- 应用以 **Accessory** 模式运行(无 Dock 图标)，与原 Swift 版一致
- 首次启动可能需要在 系统设置 → 隐私与安全性 中允许运行
- 输出 `.app` Bundle 和可选的 `.dmg` 安装镜像

### Windows
- 显示在系统托盘通知区域
- 输出 `.msi` 安装包

## License

MIT
