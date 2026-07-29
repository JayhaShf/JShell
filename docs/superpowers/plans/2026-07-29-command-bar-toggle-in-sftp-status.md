# 命令输入切换按钮下移实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 从窗口顶部移除命令输入切换按钮，并将唯一入口放到 SFTP 状态栏中，同时保持终端底部输入框和现有交互不变。

**Architecture:** 继续复用 `render_command_bar_button` 和 `toggle_command_bar`；两个 SFTP 状态栏分支都在传输记录按钮前插入同一按钮，并使用现有 `active_connected_ssh_tab_id` 作为显示条件。只提取动态工具提示纯函数用于单元测试，不新建第二套状态。

**Tech Stack:** Rust 2024、GPUI、gpui-component、rust-i18n。

---

## 文件职责

- `src/app/command_history.rs`：命令按钮样式、动态提示和已有开关逻辑。
- `src/app/ui.rs`：顶部工具区、无活动 SFTP 状态栏、正常 SFTP 状态栏和终端底部输入框。
- `locales/zh-CN.yml` / `locales/en.yml`：只复用已有 `command_bar_toggle` 与 `command_bar_hide`，不修改。

### Task 1：动态工具提示与状态栏按钮样式

**Files:**
- Modify: `src/app/command_history.rs:440-453`
- Test: `src/app/command_history.rs` existing test module or new `#[cfg(test)] mod tests`

- [ ] **Step 1：先写提示键纯函数测试**

```rust
#[test]
fn command_bar_toggle_tooltip_matches_open_state() {
    assert_eq!(command_bar_toggle_tooltip_key(false), "command_bar_toggle");
    assert_eq!(command_bar_toggle_tooltip_key(true), "command_bar_hide");
}
```

- [ ] **Step 2：运行测试确认辅助函数尚不存在**

Run: `cargo test command_bar_toggle_tooltip_matches_open_state -- --nocapture`

Expected: 编译失败，提示找不到 `command_bar_toggle_tooltip_key`。

- [ ] **Step 3：实现纯函数并接入按钮**

```rust
fn command_bar_toggle_tooltip_key(command_bar_open: bool) -> &'static str {
    if command_bar_open {
        "command_bar_hide"
    } else {
        "command_bar_toggle"
    }
}
```

`render_command_bar_button` 必须调用该纯函数决定本地化键，避免测试与真实渲染逻辑分离：

```rust
pub(crate) fn render_command_bar_button(
    &self,
    cx: &mut Context<Self>,
) -> impl gpui::IntoElement {
    let tooltip = match command_bar_toggle_tooltip_key(self.command_bar_open) {
        "command_bar_hide" => t!("command_bar_hide").to_string(),
        "command_bar_toggle" => t!("command_bar_toggle").to_string(),
        _ => unreachable!("command bar tooltip key is fixed"),
    };
    Button::new("command-bar-toggle")
        .ghost()
        .small()
        .icon(IconName::SquareTerminal)
        .tooltip(tooltip)
        .on_click(cx.listener(|this, _, window, cx| {
            this.toggle_command_bar(window, cx);
        }))
}
```

删除顶部胶囊专用的 `.rounded(px(999.))`，保持与状态栏相邻按钮一致。

- [ ] **Step 4：运行测试与格式检查**

Run: `cargo test command_bar_toggle_tooltip_matches_open_state -- --nocapture`

Run: `cargo fmt --check`

Expected: 测试和格式检查通过。

- [ ] **Step 5：提交提示与样式**

```bash
git add src/app/command_history.rs
git commit -m "fix(ui): reflect command bar state in its toggle"
```

### Task 2：从顶部移除并插入两个 SFTP 状态栏分支

**Files:**
- Modify: `src/app/ui.rs:333-445`
- Modify: `src/app/ui.rs:760-877`
- Modify: `src/app/ui.rs:2584-2633`

- [ ] **Step 1：记录移动前的结构基线**

Run: `rg -n "render_command_bar_button|open-transfers|sftp-minimize-toggle" src/app/ui.rs`

Expected: `render_command_bar_button` 仅在顶部约 2630 行调用；SFTP 两个状态栏分别包含传输和展开/折叠按钮。

- [ ] **Step 2：删除顶部入口**

从顶部 `has_active_terminal && !has_active_document` 分支删除：

```rust
.when(self.active_connected_ssh_tab_id().is_some(), |this| {
    this.child(self.render_command_bar_button(cx))
})
```

保留新建会话、水平/垂直分屏和搜索按钮的原顺序。

- [ ] **Step 3：在无 active_sftp 的状态栏插入入口**

在左侧 `div().flex_1()` 与 `Button::new("open-transfers")` 之间加入：

```rust
.when(self.active_connected_ssh_tab_id().is_some(), |this| {
    this.child(self.render_command_bar_button(cx))
})
```

该分支覆盖 SSH 已连接但 SFTP 句柄尚未建立或不可用的状态。

- [ ] **Step 4：在正常 SFTP 状态栏插入入口**

在状态文本 `div().flex_1().min_w(px(0.)).overflow_hidden()` 与传输按钮之间加入完全相同的 `.when(...)`。最终顺序固定为“状态/路径、命令按钮、传输记录/进度、展开或折叠”。

- [ ] **Step 5：执行结构检查**

Run: `rg -n "render_command_bar_button" src/app/ui.rs`

Expected: 恰好两个调用，均位于 `render_sftp_panel` 内；顶部工具区不再调用。

Run: `rg -n -C 8 "render_command_bar_button" src/app/ui.rs`

Expected: 两个调用后都紧邻 `open-transfers`，且都受 `active_connected_ssh_tab_id().is_some()` 保护。

- [ ] **Step 6：编译并运行完整测试**

Run: `cargo fmt --check`

Run: `cargo test --quiet`

Run: `cargo check --all-targets`

Expected: 全部退出 0；不得出现重复元素 ID 或借用检查错误。

- [ ] **Step 7：提交位置移动**

```bash
git add src/app/ui.rs
git commit -m "feat(ui): move command toggle to SFTP status bar"
```

### Task 3：Debug 交互与窄窗口验证

**Files:**
- No code changes expected

- [ ] **Step 1：构建并启动 Debug 程序**

Run: `cargo build`

Run: `target\debug\jshell.exe`

Expected: 程序启动且日志中没有 panic 或重复 ID 错误。

- [ ] **Step 2：验证已连接 SSH 的两个 SFTP 状态**

分别检查 SFTP 已就绪与尚未就绪：顶部不显示命令按钮；状态栏在“传输记录”之前显示终端图标按钮；点击后终端底部出现命令输入框，再次点击关闭；工具提示分别为“显示命令输入”和“隐藏命令输入”。

- [ ] **Step 3：验证面板展开、折叠与输入状态**

展开和折叠 SFTP 面板时按钮始终可见；输入框打开时折叠面板不会关闭或移动输入框；执行命令、命令历史和补全行为保持原样。

- [ ] **Step 4：验证不应显示的上下文**

本地终端、未连接 SSH、活动远程文件文档都不显示命令按钮。切回已连接 SSH 后按钮恢复，且不会出现两个同 ID 按钮。

- [ ] **Step 5：验证窄窗口**

缩窄窗口直到路径和传输区域开始收缩。路径/状态允许截断；命令、传输和展开/折叠按钮不重叠、不换行，状态栏高度保持不变。

- [ ] **Step 6：最终静态检查**

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: 退出 0。
