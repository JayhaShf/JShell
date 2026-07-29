# 按钮交互精简实施计划

> **执行要求：** 使用 `superpowers:executing-plans` 在当前任务中逐项实施和复查。

**目标：** 删除主工作区无效和重复的按钮入口，修复 Windows 下载位置打开逻辑，并完成 Release 编译与桌面调试。

**架构：** 保持现有 GPUI 组件结构，不增加新的导航状态。展开时直接渲染完整侧栏，折叠时渲染只包含恢复按钮的 40 px 窄栏；系统文件打开统一经过可测试的跨平台辅助函数。

**技术栈：** Rust 2024、GPUI、gpui-component、`open` crate、Cargo 单元测试。

## 全局约束

- 保留 SSH 会话、会话文件夹、终端、文档编辑和 SFTP 的现有核心行为。
- 保留标题栏的会话选择器、分屏、终端搜索、设置和 Windows 窗口控制。
- 不新增依赖。
- 不暂存文件，不创建提交。

---

### 任务 1：修复完成下载后的打开位置操作

**文件：**
- 修改：`src/app/dialogs.rs`
- 测试：`src/app/dialogs.rs` 内部测试模块

**接口：**
- 新增 `open_completed_download_target_with<E>(target: &str, opener: impl FnOnce(&str) -> Result<(), E>) -> Result<(), String>`。
- 新增 `open_completed_download_target(target: &str)`，生产环境使用 `open::that` 并记录失败警告。

- [ ] **步骤 1：先写失败测试**

```rust
#[test]
fn completed_download_target_is_forwarded_to_platform_opener() {
    let mut opened = None;
    let result = open_completed_download_target_with("C:/Downloads/archive.zip", |target| {
        opened = Some(target.to_string());
        Ok::<(), &'static str>(())
    });

    assert_eq!(result, Ok(()));
    assert_eq!(opened.as_deref(), Some("C:/Downloads/archive.zip"));
}

#[test]
fn completed_download_open_error_is_preserved_for_logging() {
    let result = open_completed_download_target_with("C:/Downloads/archive.zip", |_| {
        Err::<(), _>("cannot open")
    });

    assert_eq!(result, Err("cannot open".to_string()));
}
```

- [ ] **步骤 2：运行测试并确认因辅助函数不存在而失败**

运行：`cargo test completed_download_ -- --nocapture`

预期：编译失败，提示找不到 `open_completed_download_target_with`。

- [ ] **步骤 3：实现最小跨平台辅助函数并替换硬编码命令**

```rust
fn open_completed_download_target_with<E>(
    target: &str,
    opener: impl FnOnce(&str) -> Result<(), E>,
) -> Result<(), String>
where
    E: std::fmt::Display,
{
    opener(target).map_err(|error| error.to_string())
}

fn open_completed_download_target(target: &str) {
    if let Err(error) =
        open_completed_download_target_with(target, |target| open::that(target))
    {
        tracing::warn!(target = target, %error, "failed to open completed download target");
    }
}
```

完成下载按钮只调用 `open_completed_download_target(&target)`。

- [ ] **步骤 4：运行针对性测试并确认通过**

运行：`cargo test completed_download_ -- --nocapture`

预期：两项测试通过。

### 任务 2：缩窄折叠快捷栏

**文件：**
- 修改并测试：`src/app/constants.rs`

**接口：**
- `COLLAPSED_SIDEBAR_WIDTH` 从 `60.0` 调整为 `40.0`。

- [ ] **步骤 1：先把现有宽度测试预期改为 40 px**

```rust
assert_eq!(COLLAPSED_SIDEBAR_WIDTH, 40.0);
```

- [ ] **步骤 2：运行测试并确认失败**

运行：`cargo test workspace_rail_width_matches_the_preview_layout -- --nocapture`

预期：断言失败，实际值为 `60.0`。

- [ ] **步骤 3：把生产常量改为 40 px**

```rust
pub(crate) const COLLAPSED_SIDEBAR_WIDTH: f32 = 40.0;
```

- [ ] **步骤 4：重新运行测试并确认通过**

运行：`cargo test workspace_rail_width_matches_the_preview_layout -- --nocapture`

预期：测试通过。

### 任务 3：精简工作区按钮和重复入口

**文件：**
- 修改：`src/app/ui.rs`

**行为：**
- `render_activity_rail` 只渲染 `activity-rail-sessions`，按钮无条件恢复侧栏。
- 完整侧栏头部只保留折叠按钮，不再显示重复设置按钮。
- 展开侧栏分支不再调用 `render_activity_rail`。
- 标题栏删除 `windows-title-search`，保留 `+` 会话选择器和设置。

- [ ] **步骤 1：精简折叠快捷栏及侧栏头部按钮**

删除 `activity-rail-selector`、`activity-rail-transfers`、`activity-rail-monitoring`、`activity-rail-new-ssh`、`activity-rail-settings` 和 `sidebar-settings`，并将恢复侧栏回调简化为直接设置 `sidebar_collapsed = false`。

- [ ] **步骤 2：删除展开侧栏分支中的快捷栏**

展开分支直接渲染 `h_resizable("ashell-workspace")`，不再额外包裹 `render_activity_rail`。

- [ ] **步骤 3：删除标题栏重复会话按钮**

删除 `windows-title-search`；`render_tab_bar` 中的 `open-selector` 继续作为唯一可见会话选择入口。

- [ ] **步骤 4：执行静态复查**

运行：

```powershell
rg -n 'activity-rail-(selector|transfers|monitoring|new-ssh|settings)|sidebar-settings|windows-title-search' src/app/ui.rs
rg -n 'render_activity_rail\(cx\)' src/app/ui.rs
```

预期：第一条没有结果；第二条只有折叠分支的一处调用。

### 任务 4：删除失效旧实现

**文件：**
- 修改：`src/app/ui.rs`

- [ ] **步骤 1：删除不可达的旧监控实现**

保留 `render_sidebar_monitoring_panel` 在提前返回之前的 154 px 四项进度条实现，删除 `return` 后的全部旧布局，并移除 `#[allow(unreachable_code)]`。

- [ ] **步骤 2：删除未调用的旧折叠侧栏实现**

删除 `#[allow(dead_code)] fn render_collapsed_sidebar` 及其完整函数体；当前折叠行为统一由精简后的 `render_activity_rail` 提供。

- [ ] **步骤 3：运行格式检查与编译检查**

运行：`cargo fmt --check`，然后运行 `cargo check`。

预期：均成功，且没有新增警告。

### 任务 5：完整验证和桌面调试

**文件：**
- 不再修改生产文件，除非验证发现本次改动造成的问题。

- [ ] **步骤 1：运行完整测试**

运行：`cargo test`

预期：全部测试通过。

- [ ] **步骤 2：编译 Release 版本**

运行：`cargo build --release`

预期：生成 `target/release/ashell.exe`。

- [ ] **步骤 3：启动桌面程序并调试按钮**

验证：侧栏折叠/展开、标题栏 `+`、设置、分屏、终端搜索、SFTP 展开/收起、传输记录、会话文件夹展开/编辑/批量打开和 Windows 最小化/最大化/关闭。

- [ ] **步骤 4：最终状态检查**

运行：`git status --short` 和 `git diff --check`。

预期：没有暂存内容、没有提交，且没有空白错误。
