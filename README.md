[中文](README.md) | [English](README.en.md)

# JShell

JShell 是一个使用 Rust、GPUI 与 `alacritty_terminal` 构建的桌面终端工作区，面向需要同时管理本地终端、SSH 会话、SFTP 文件和远程文本的用户。当前源码版本为 `0.1.0-beta.4`；界面语言默认跟随系统，也可在设置中切换，项目默认文档入口为中文。

![JShell 工作区预览](assets/screenshots/compact-workspace.png)

## 核心能力

- 本地终端、SSH 和串口会话，支持会话保存、文件夹整理和重复连接复用。
- 水平或垂直分屏，可在同一工作区混合显示 SSH 终端和远程文本编辑器。
- 类型色带区分终端、文件和系统标签；标签状态可显示连接、保存和错误状态。
- Windows 自定义标题栏与动态任务栏标题，例如 `JShell - production`、`JShell - nginx.conf*`。
- CPU、内存、Swap、网络和磁盘状态监视。
- JShell Light、JShell Dark 和 VS Code Dark 主题，以及可持久化的字体、布局和快捷键设置。

## SSH 与代理

- 支持密码、私钥文件、内联私钥和私钥口令认证。
- 每个 SSH 会话可选择直连、使用全局代理或自定义代理。
- 代理入口统一支持 `SOCKS5`、`SOCKS5H`、`HTTP` 和 `HTTPS`；直连模式不会读取代理地址。
- SSH 断开后可在原终端中重连，旧后端迟到的输出和关闭事件不会覆盖新连接状态。

## SFTP 文件管理

- 浏览目录、显示隐藏文件、上传、下载、新建目录和删除文件或目录。
- Linux 远程文件显示类型、符号权限和八进制权限，例如 `-rw-r--r-- 0644`。
- 单击文件行只聚焦，不会切换前方复选框；复选框用于批量选择。
- 双击目录进入目录，双击普通文本文件在当前 SSH 工作区右侧打开编辑器。
- 删除失败的项目会保留选择，便于修正原因后重试；上传、下载和删除不会在断线后自动重复执行。
- SFTP 连接由独立监视器维护，定期检测静默断线，按退避策略自动恢复，也可立即重连。

## 远程编辑器

- 终端和文本编辑器可以分屏显示，例如左侧 SSH、右侧配置文件。
- 普通文本可移动到独立窗口；主窗口保留原分屏位置，关闭独立窗口只会重挂编辑器。
- 自动换行为全局偏好，切换后会持久化并应用到已经打开和之后打开的文档。
- 标签、工具栏和底部状态栏共享同一保存状态，包括未保存、检查中、保存中、冲突、失败和结果未知。
- 断线不会触发自动保存，也不会覆盖远端文件；本地编辑内容保留在内存中。连接恢复后由用户手动保存。
- 写入阶段断线时不会盲目重试；无法确认远端结果时，文档保持 dirty 并显示“保存结果未知”。
- 大文件使用分页只读模式；二进制文件和不支持的编码不会按普通 UTF-8 文本写回。

## 语法着色

编辑器使用 `gpui-component` 的 Tree-sitter 高亮能力，当前构建包含 Bash、C、C++、CSS、Diff、Go、HTML、JavaScript、Lua、Makefile、Markdown、Python、Rust、SQL、TOML、TypeScript 和 YAML。Shiki 更适合 Web/HTML 渲染，当前未接入原生 GPUI 编辑器路径。

## 安装与运行

预编译版本发布在 [GitHub Releases](https://github.com/JayhaShf/JShell/releases)。发布产物名称为：

- Windows：`jshell-<版本>-windows-x86_64.zip`
- Linux：`jshell-<版本>-linux-x86_64.tar.gz`
- macOS：`jshell-<版本>-macos-aarch64.zip` 或 `jshell-<版本>-macos-x86_64.zip`

macOS 应用使用临时签名。系统拦截首次启动时，可在确认下载来源后执行：

```bash
xattr -cr /Applications/JShell.app
```

## 配置与兼容

默认配置文件：

```text
~/.config/jshell/sessions.json
```

Windows 上通常位于：

```text
C:\Users\<用户名>\.config\jshell\sessions.json
```

首次启动会迁移旧 `~/.config/ashell` 配置。旧主题名称会映射到 JShell 主题；远端原子写入继续保留 `.ashell-*` 临时文件命名，以兼容已有恢复和清理逻辑。

配置可保存会话、代理、主题、字体、快捷键、窗口布局、SFTP 选项和自动换行偏好。敏感字段使用系统安全存储或加密配置流程处理。

## 开发与构建

要求 Rust `1.89.0` 或更高版本，以及目标平台所需的 GPUI 系统依赖。

```bash
cargo run --locked
cargo build --locked --release
```

Windows 可执行文件位于 `target/release/jshell.exe`。macOS 本地应用包可使用：

```bash
./scripts/package-macos-app.sh
```

## 验证

```bash
cargo fmt --check
cargo test --locked --quiet
cargo check --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release
cargo audit
git diff --check
```

## 许可证

本项目采用 [GPL-3.0-or-later](LICENSE) 许可证。

## 致谢

JShell 基于 [TomZz](https://github.com/TomZz) 创建的 [rust-kotlin/ashell](https://github.com/rust-kotlin/ashell) 演进。感谢 TomZz、`rust-kotlin/ashell` 的所有维护者，以及每一位上游贡献者提供的基础与持续投入。
