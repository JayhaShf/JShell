# SFTP 重连图标区分设计

日期：2026-08-03

## 背景

远程文件工具栏中的“立即重连 SFTP”和“刷新”按钮当前都使用 `IconName::Redo2`，只能依靠悬停提示区分，容易误操作。

## 目标

- SFTP 重连按钮改用 `IconName::Network`。
- 刷新按钮继续使用 `IconName::Redo2`。
- 两个相邻按钮在不依赖提示文本时也能清楚区分。

## 范围

仅修改图标映射。按钮位置、尺寸、样式、提示文本、点击行为和显示条件均保持不变，不调整 SFTP 连接或重试逻辑。

## 实现

在 `src/app/ui.rs` 中为 SFTP 重连和刷新操作使用明确的私有图标映射，并让工具栏按钮引用该映射。重连映射到 `Network`，刷新映射到 `Redo2`。

## 测试与验证

- 增加单元测试，分别断言重连与刷新图标的具体映射，防止两者再次设置成同一图标。
- 运行聚焦测试、`cargo fmt --check`、`cargo check --locked --all-targets` 和 `cargo clippy --locked --all-targets --all-features -- -D warnings`。
- 检查最终差异仅涉及设计说明、图标映射和对应测试。

## 交付

改动提交到 `codex/sftp-reconnect-icon` 分支，不合并到 `main`，保留给后续修改一并合并。
