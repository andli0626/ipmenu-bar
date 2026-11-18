# GitHub Actions 多端打包接入指南

> 适用版本：Tauri 2.x / Rust 2021，工程根目录 `ipmenu-bar`（Rust 后端在 `src-tauri/`，前端为 `public/index.html` 静态页）。

## 一、结论

**可以直接接入，改动量很小。** 这个工程没有 Node 构建链（`beforeBuildCommand` 为空、`frontendDist` 直接指向 `../public`），CI 只需要 **Rust 工具链 + Tauri CLI** 两样东西，不存在"前端装包失败"这类常见坑。

已落地的两个工作流：

| 文件 | 触发 | 作用 |
|---|---|---|
| `.github/workflows/release.yml` | push tag `v*` / 手动 | 构建 macOS 通用 dmg + Windows setup.exe/msi，创建 **草稿 Release** 并上传资产 |
| `.github/workflows/build-check.yml` | PR / push main（限 `src-tauri`、`public`、workflows 改动） | `--no-bundle` 编译校验 + 版本一致性校验，不发布 |

## 二、现状评估

| 维度 | 现状 | 结论 |
|---|---|---|
| 技术栈 | Tauri 2 + Rust 2021，`crate-type = staticlib/cdylib/rlib` | ✅ 标准结构，官方 CI 模板可直接用 |
| 前端 | 静态 `public/index.html`，`withGlobalTauri` | ✅ 无需 `npm install` / 前端构建 |
| 依赖锁定 | `src-tauri/Cargo.lock` 已入库 | ✅ 构建可复现 |
| 打包目标 | `bundle.targets = "all"` | ✅ macOS→app+dmg，Windows→msi+nsis |
| 图标 | `icon.ico` + 多尺寸 PNG 齐备 | ⚠️ 见 P1-3 |
| 签名 | macOS 无签名、Windows 无证书 | ⚠️ 见 P0-1 |
| 版本号 | `Cargo.toml` 与 `tauri.conf.json` 各写一份（均 1.2.4） | ⚠️ 见 P1-1 |
| 构建目录忽略 | `target/`、`src-tauri/target/` 均已忽略 | ✅ 无需处理 |

## 三、阻塞问题：问题 → 根因 → 措施

### P0-1 macOS 下载后提示"已损坏"

- **问题**：未签名的 dmg 在 Apple Silicon 上解压后被 Gatekeeper 判定"已损坏，无法打开"。这是因为 arm64 要求所有来自网络的 app 必须具备有效签名，而非单纯的"未受信任开发者"提示。
- **根因**：没有 Developer ID 证书 → 签名无效 → 系统直接拒绝加载。
- **措施（按成本排序）**：
  1. **零成本（已采用）**：`src-tauri/tauri.conf.json` 的 `bundle.macOS` 已加 `"signingIdentity": "-"`，走 ad-hoc 签名。不会再报"已损坏"，但用户首次打开仍需「右键 → 打开」，或在 系统设置 → 隐私与安全性 点「仍要打开」。这是无账号场景下的最优解。
  2. **$99/年 Apple Developer（后续可平滑升级）**：导出 `Developer ID Application` 证书 + 公证，用户在浏览器下载后双击即可安装，无任何提示。`release.yml` 里已把所需全部 Secrets 以注释形式写好，取消注释即可生效——**环境变量优先级高于配置**，所以配了证书后 `signingIdentity: "-"` 会被自动覆盖，两套方案不冲突，不需要回改配置。

### P0-2 Windows 装上后看不到 IP

- **问题**：`tray.set_title()` 在 Windows 上是 **Unsupported**（Tauri 文档明确：Linux 需配合图标才显示，Windows 不支持）。Windows 端只有托盘图标 + tooltip，README 里"菜单栏直接显示 IP"的核心卖点在 Windows 是缺失的。
- **根因**：不是打包问题，是跨平台 API 能力差异——打包能成功，但装完体验不完整，用户会以为是坏包。
- **措施（已实现）**：新增 `src-tauri/src/pill.rs` + `public/pill.html`，在 Windows 上创建一个 248×54 的无边框置顶小窗，常驻屏幕右下角显示当前 IP，右侧提供复制与收起；点托盘图标可重新唤出。窗口用运行时 `cfg!` 判断（而非 `#[cfg]`），保证这段代码在 macOS 上同样参与编译，不会变成"只有 CI 才编译"的黑盒。
  - 本地预览布局：`IPMENUBAR_PILL_PREVIEW=1 ./src-tauri/target/release/ipmenubar`（macOS 上窗口不透明，仅用于看排版）。
  - 透明窗口在 macOS 需要 `macos-private-api` feature，故 `transparent(true)` 只在 Windows 编译进来。

### P1-1 版本号双源，安装包与界面可能不一致

- **问题**：安装包版本取 `tauri.conf.json`，界面「关于」取 `env!("CARGO_PKG_VERSION")`（即 `Cargo.toml`）。任一处漏改，就会出现文件名是 1.2.4、关于对话框显示 1.2.3。
- **根因**：Tauri 的 bundle 版本与 crate 版本是两套独立字段。
- **措施**：已在两个 workflow 里加了 `Verify version consistency` 步骤，三向校验 `Cargo.toml` / `tauri.conf.json` / git tag，不一致直接 fail。

### P1-2 中文 productName 带来的资产命名问题

- **问题**：`productName` 是「ip地址」，默认产物名变成 `ip地址_1.2.4_universal.dmg`。上传到 Release 后下载 URL 会被 percent-encode（`%E4%BE%9B...` 这类长串），`curl`/脚本/部分下载器处理中文资产名容易出错，统计与外部引用也难读。
- **根因**：productName 参与 bundle 文件名生成。
- **措施**：
  - 已在 workflow 用 `releaseAssetNamePattern: 'IPMenuBar_[version]_[tag][setup][ext]'`，**只改资产名、不改应用名**，用户下载到的仍是干净的中文应用；
  - 若愿意改应用名：`productName` 设 `IPMenuBar`，macOS 用 `CFBundleDisplayName`（Info.plist 覆盖）保留中文显示。

### P1-3 图标清单尺寸不全（已修复）

- **问题**：`bundle.icon` 只列到 256px，macOS icns 没有 512/1024 层，Retina 下图标偏糊。
- **根因**：Tauri 按**文件名**匹配图标类型，`icons/1024x1024.png` 这种命名不在白名单里，加进数组会直接报 `Failed to create app icon: No matching IconType` 导致打包失败——1024px 必须命名为 `512x512@2x.png`。
- **措施（已完成）**：新增 `icons/512x512@2x.png`（1024px 内容），并把 `256x256.png`、`512x512.png`、`512x512@2x.png` 补进 `bundle.icon`。实测 icns 由 34 KB 增至 116 KB，包含 512 与 1024 层。

### P2 其他（不阻塞）

- `src-tauri/gen/schemas/*` 已被 git 跟踪且我已在 `.gitignore` 加规则——但 ignore 对已跟踪文件无效，需要 `git rm --cached src-tauri/gen/schemas -r` 才生效，否则每次 Tauri 升级都会产生大段 diff。
- 没有 `rust-toolchain.toml`：CI 用 `stable`（当前 1.94），本地也是 1.94，暂无不一致风险；建议后续 pin 版本。
- 没有 updater：`tauri-plugin-updater` 未接入。**注意这是"越早越好"的项**——公钥必须固化在旧版本里，如果首发不带上，后面所有已安装用户都无法自动升级，只能手动重装。
- 私有仓库成本：GitHub 私有仓库 macOS runner 按 **10 倍**计费分钟数。本工程冷缓存约 8~12 分钟/次（通用二进制要编译两遍），约等于 80~120 计费分钟/次发布。公开仓库无此限制。

## 四、工作流设计要点

| 项 | 取值 | 原因 |
|---|---|---|
| 触发 | tag `v*` + `workflow_dispatch` | tag 即版本，天然对齐；手动可补发 |
| 结构 | `build`(矩阵并行) → `release`(汇总发布) | 关键设计：**任一平台失败都不会留下"只有一半资产"的 Release** |
| macOS 构建 | `--target universal-apple-darwin --bundles dmg` | 单个 dmg 同时兼容 M 系和 Intel，下载页不用做架构判断 |
| Windows 构建 | `--bundles nsis,msi` | NSIS 免管理员、单文件，普通用户首选；MSI 给批量分发 |
| Tauri CLI | `npm i -g @tauri-apps/cli@^2` | 比 `cargo install tauri-cli` 快一个数量级（后者要现编 5~8 分钟） |
| 缓存 | `Swatinem/rust-cache@v2`，`workspaces: src-tauri -> target` | target 目录 2~3 GB，冷/热构建差 4 倍以上 |
| 构建步骤 | shell 里的 `tauri build \| tee build.log` | 不用 tauri-action：CLI 原始报错能进日志，失败时可直接定位 |
| 失败留痕 | 失败时把日志尾部写入 `$GITHUB_STEP_SUMMARY` | 可观测；避免"只有 exit code 1、没有原因"的黑盒失败 |
| 产物归档 | `actions/upload-artifact` → `artifacts/download-artifact` | 构建与发布解耦，发布失败可单独重跑 |
| 发布 | `gh release create/upload --clobber` | 统一 ASCII 命名，重复执行幂等 |
| 并发 | `concurrency` 按 ref 分组 | 避免重复 tag 触发两次构建互相覆盖 Release |

产物命名（构建后统一改名，与 `productName` 的中文无关）：

```
IPMenuBar_1.2.4_macos-universal.dmg
IPMenuBar_1.2.4_windows-x64-setup.exe
IPMenuBar_1.2.4_windows-x64.msi
```

## 五、发版流程

1. 提交 `.github/workflows/`（已生成，直接 commit）。
2. 确认仓库 Actions 权限：Settings → Actions → General → Workflow permissions 选 **Read and write**（workflow 里虽已声明 `permissions: contents: write`，组织策略可能仍会拦截）。
3. 本地验证版本一致后打 tag：
   ```bash
   git tag v1.2.4 && git push origin v1.2.4
   ```
4. 进 Actions 看 `build`（两个平台并行）→ `release`（汇总发布）两个 job。
5. 全部绿后，Releases 里会直接出现已发布的版本，三个安装包齐备。
6. 若只想验证编译不发布：推 `main` 即可触发 `Build Check`（`--no-bundle`，不产包、不建 Release）。

## 六、签名 Secrets 清单（可选，按需启用）

**macOS（在 `release.yml` 里取消对应注释）**

| Secret | 来源 |
|---|---|
| `APPLE_CERTIFICATE` | 钥匙串导出 `.p12` → `openssl base64 -A -in cert.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码 |
| `APPLE_SIGNING_IDENTITY` | `security find-identity -v -p codesigning` 输出的证书名 |
| `APPLE_API_ISSUER` / `APPLE_API_KEY` / `APPLE_API_KEY_PATH` | App Store Connect → 用户与访问 → Integrations，创建 Developer 密钥（推荐方式） |
| 或 `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` | Apple ID + App 专用密码（旧方式） |

**Windows**

Tauri v2 的推荐路径是 `bundle.windows.signCommand`（接 Azure Trusted Signing / signtool），或 `certificateThumbprint` + runner 预装证书。无证书时接受 SmartScreen 提示即可，不影响安装。

## 七、常见失败排查

| 现象 | 原因 | 处理 |
|---|---|---|
| `tauri build` 报找不到 `wix`/`nsis` | CLI 首次运行需从 GitHub 下载打包器，runner 网络抖动 | `retryAttempts: 2` 已配；仍失败则重跑 job |
| `error: failed to run custom build command for tauri-build` | `icons/icon.ico` 缺失或尺寸非法 | 确认 `bundle.icon` 中路径相对 `src-tauri` |
| macOS 构建报 target 未安装 | 矩阵 `rust_targets` 未生效 | 检查 `dtolnay/rust-toolchain` 的 `targets` 输入 |
| Release 资产名带中文乱码 | 未配 `releaseAssetNamePattern` | 已配，勿删 |
| macOS 提示"已损坏" | 无有效签名 | 加 `"signingIdentity": "-"` 或上 Developer ID |
| 版本校验失败 | 改版本只改了一处 | 同步 `Cargo.toml` + `tauri.conf.json` |

## 八、建议的后续动作（按优先级）

1. **（已完成）ad-hoc 签名** —— 已写入 `tauri.conf.json`，消除"下载即损坏"的首发事故。
2. **Windows 托盘 IP 可视化** —— 否则 Windows 包等于半成品。
3. **接 updater** —— 首发就要带，否则后期无 OTA 能力。
4. 版本单源化：发版脚本读取 `Cargo.toml` 并自动回写 `tauri.conf.json` 的 `version`。
