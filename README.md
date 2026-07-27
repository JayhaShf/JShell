[中文](README.md) | [English](README.en.md)

# ashell

ashell 是一个使用 Rust 与 GPUI 构建的原生桌面终端工作区。它把 SSH、SFTP、文件传输和主机状态集中在一个紧凑的 Windows 风格界面中。

![ashell 紧凑工作区预览](assets/screenshots/compact-workspace.png)

## 功能

- 本地终端、SSH 与串口会话。
- 已连接的保存会话再次打开时直接跳转既有标签，不会创建重复连接。
- 保存会话支持批量打开，已连接会话会被自动跳过。
- 底部 SFTP 面板支持远程目录浏览、上传、下载、新建目录、删除、文件编辑和传输进度。
- SFTP 使用列表视图，支持显示以 `.` 开头的隐藏文件和目录。
- 左侧紧凑工作区显示 CPU、内存、Swap、网络和磁盘状态。
- 自定义 Windows 标题栏，提供会话标签、搜索、设置及系统窗口控制。
- 三套主题：Ashell Light、Ashell Dark、VS Code Dark。
- UI 与终端默认使用系统安装的 `Noto Sans CJK SC` 字体。

## 运行

### 前置条件

- Rust stable toolchain
- Windows 上建议安装 `Noto Sans CJK SC`

### 开发运行

```bash
cargo run
```

### Release 构建

```bash
cargo build --release
```

Windows 可执行文件输出到：

```text
target/release/ashell.exe
```

## 配置

默认配置文件位于：

```text
~/.config/ashell/sessions.json
```

Windows 的默认位置通常为：

```text
C:\Users\<用户名>\.config\ashell\sessions.json
```

配置会保存已保存会话、主题、字体、布局和 SFTP 显示选项。

## 验证

```bash
cargo fmt --check
cargo test
cargo build --release
```

## 致谢

本项目基于 [TomZz](https://github.com/TomZz) 创建的 [ashell](https://github.com/rust-kotlin/ashell) 演进而来。感谢原作者提供的项目基础与持续贡献。

## 许可证

本项目采用 [GPL-3.0-or-later](LICENSE) 许可证。
