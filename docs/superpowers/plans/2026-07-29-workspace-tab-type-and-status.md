# 工作区标签类型与状态实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为所有可见工作区标签增加统一的 3 px 左侧状态色带和类型图标，完整、可靠地表达终端聚合状态与远程文档状态，同时保持现有标签交互和 SSH 强调样式。

**Architecture:** 新建 `src/app/workspace_tabs.rs` 承载可独立测试的视觉类型、状态、语义色角色以及终端/文档状态归约函数；`src/app/ui.rs` 只负责从现有 `TerminalTab`、`RemoteDocument` 生成 `WorkspaceTabRenderData`，并在唯一的共享标签分支中把类型映射为图标、把语义色角色映射为当前主题颜色。主题 JSON 只调整两套 JShell 主题的 `base.blue`，现有 `filter_map`、标签激活/关闭/滚动、SSH 黑底与底部指示线均保持原数据流。

**Tech Stack:** Rust 2024、GPUI、gpui-component、Cargo 单元测试、serde_json 主题夹具、Windows Debug 桌面交互验证。

---

## 文件职责

- Create: `src/app/workspace_tabs.rs`：工作区标签视觉类型、状态、语义色角色，以及终端多 pane 与远程文档状态的纯逻辑归约和表驱动测试。
- Modify: `src/app/mod.rs:1-10`：注册 `workspace_tabs` 模块。
- Modify: `src/app/ui.rs:24-49,2350-2580`：构造带类型、状态、dirty 标记的渲染数据，并统一渲染 3 px 色带、类型图标、截断标题、未保存标记和关闭按钮。
- Modify: `src/app/theme.rs:307-332`：锁定 JShell Light、JShell Dark 与 VS Code Dark 的 `base.blue` 主题值。
- Modify: `assets/themes/ashell.json:17,33`：把 JShell Light 与 JShell Dark 的 `base.blue` 改为 `#2f7faa`。

## 状态与渲染契约

- `WorkspaceTabVisualKind::{Terminal, RemoteDocument}` 只表达标签类型；它决定 `SquareTerminal` / `File` 图标以及 `Normal` 状态使用绿色还是蓝色。
- `WorkspaceTabStatus::{Normal, Attention, Error, Unavailable}` 只表达状态；优先级固定为 `Error > Attention > Unavailable > Normal`。
- 终端单实例映射固定为：`connected == true` 为 `Normal`；`connected == false && disconnected_reason.is_some()` 为 `Error`；`connected == false && disconnected_reason.is_none()` 为 `Attention`。
- 终端工作区对每个 pane 标识逐一查找终端；找不到时向聚合函数传入 `None`。空 pane 集合和任一缺失实例都不能被默认成 `Normal`。
- 文档错误源固定覆盖 `LoadState::Failed`、`SaveState::{Failed, Conflict}`、`DocumentConnectionState::Offline`、`large_file.error`；处理中源固定覆盖 `LoadState::Loading`、`SaveState::{Checking, Saving}`、`DocumentConnectionState::Reconnecting`、`large_file.loading` 和 dirty。
- 语义色角色固定为：终端 `Normal -> Success`（绿），文件 `Normal -> Blue`（蓝），`Attention -> Warning`（黄），`Error -> Danger`（红），`Unavailable -> MutedForeground`（灰）。
- 共享标签子元素顺序固定为：3 px 色带、类型图标、截断标题、可选 dirty 圆点、关闭按钮；旧的左侧 6 px 状态圆点和标题字符串中的 ` •` 均删除。

### Task 1：终端单实例状态与多 pane 聚合

**Files:**
- Create: `src/app/workspace_tabs.rs`
- Modify: `src/app/mod.rs:1-10`
- Test: `src/app/workspace_tabs.rs`

- [ ] **Step 1：注册模块并先写终端状态表驱动测试**

在 `src/app/mod.rs` 的模块声明区加入这一行：

```rust
pub mod workspace_tabs;
```

创建 `src/app/workspace_tabs.rs`，先写入以下完整内容。测试刻意引用尚未实现的两个函数，以建立 RED 基线：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabVisualKind {
    Terminal,
    RemoteDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabStatus {
    Normal,
    Attention,
    Error,
    Unavailable,
}

impl WorkspaceTabStatus {
    fn priority(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Unavailable => 1,
            Self::Attention => 2,
            Self::Error => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_workspace_status_aggregates_every_pane_by_priority() {
        use WorkspaceTabStatus::{Attention, Error, Normal, Unavailable};

        let terminal_cases = [
            ("connected", true, None, Normal),
            ("initial connection", false, None, Attention),
            ("reported disconnect", false, Some("closed"), Error),
            ("connected wins over stale reason", true, Some("stale"), Normal),
        ];
        for (name, connected, reason, expected) in terminal_cases {
            assert_eq!(terminal_tab_status(connected, reason), expected, "{name}");
        }

        let aggregate_cases = vec![
            (
                "all pane terminals connected",
                vec![Some(Normal), Some(Normal)],
                Normal,
            ),
            (
                "one pane still connecting",
                vec![Some(Normal), Some(Attention)],
                Attention,
            ),
            (
                "one pane reported disconnect",
                vec![Some(Normal), Some(Error)],
                Error,
            ),
            ("empty workspace", vec![], Unavailable),
            (
                "connected pane plus missing instance",
                vec![Some(Normal), None],
                Unavailable,
            ),
            (
                "attention outranks missing instance",
                vec![Some(Normal), None, Some(Attention)],
                Attention,
            ),
            (
                "error outranks attention and missing instance",
                vec![Some(Attention), None, Some(Error)],
                Error,
            ),
        ];
        for (name, pane_statuses, expected) in aggregate_cases {
            assert_eq!(
                aggregate_terminal_workspace_status(&pane_statuses),
                expected,
                "{name}"
            );
        }
    }
}
```

- [ ] **Step 2：运行单一过滤测试并确认 RED**

Run: `cargo test terminal_workspace_status_aggregates_every_pane_by_priority -- --nocapture`

Expected: 编译失败，错误明确指出找不到 `terminal_tab_status` 和 `aggregate_terminal_workspace_status`。

- [ ] **Step 3：实现终端单实例映射和严格优先级聚合**

在 `#[cfg(test)]` 之前插入：

```rust
pub(crate) fn terminal_tab_status(
    connected: bool,
    disconnected_reason: Option<&str>,
) -> WorkspaceTabStatus {
    if connected {
        WorkspaceTabStatus::Normal
    } else if disconnected_reason.is_some() {
        WorkspaceTabStatus::Error
    } else {
        WorkspaceTabStatus::Attention
    }
}

pub(crate) fn aggregate_terminal_workspace_status(
    pane_statuses: &[Option<WorkspaceTabStatus>],
) -> WorkspaceTabStatus {
    if pane_statuses.is_empty() {
        return WorkspaceTabStatus::Unavailable;
    }

    pane_statuses.iter().fold(
        WorkspaceTabStatus::Normal,
        |aggregate, pane_status| {
            let pane_status = pane_status.unwrap_or(WorkspaceTabStatus::Unavailable);
            if pane_status.priority() > aggregate.priority() {
                pane_status
            } else {
                aggregate
            }
        },
    )
}
```

该实现必须保留 `Normal=0`、`Unavailable=1`、`Attention=2`、`Error=3` 的数值顺序；这样缺失 pane 不会判正常，同时错误和连接中状态不会被缺失数据遮蔽。

- [ ] **Step 4：重新运行终端聚合测试并确认 GREEN**

Run: `cargo test terminal_workspace_status_aggregates_every_pane_by_priority -- --nocapture`

Expected: 1 项测试通过；初次连接映射为 `Attention`，空工作区与“正常 + 缺失实例”映射为 `Unavailable`，两条混合优先级用例通过。

- [ ] **Step 5：提交终端状态核心**

```powershell
git add src/app/mod.rs src/app/workspace_tabs.rs
git commit -m "feat(ui): model terminal workspace tab status"
```

### Task 2：远程文档全状态归约

**Files:**
- Modify: `src/app/workspace_tabs.rs`
- Test: `src/app/workspace_tabs.rs`

- [ ] **Step 1：先写覆盖全部状态源的文档表驱动测试**

在 `src/app/workspace_tabs.rs` 顶部加入：

```rust
use crate::document::{DocumentConnectionState, LoadState, SaveState};
```

在现有 `tests` 模块中、终端测试之后加入：

```rust
    #[test]
    fn document_workspace_status_covers_all_sources_and_priority() {
        use WorkspaceTabStatus::{Attention, Error, Normal};

        struct Case {
            name: &'static str,
            load_state: LoadState,
            save_state: SaveState,
            connection_state: DocumentConnectionState,
            dirty: bool,
            large_file_loading: bool,
            large_file_error: bool,
            expected: WorkspaceTabStatus,
        }

        let cases = vec![
            Case {
                name: "ready idle clean",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Normal,
            },
            Case {
                name: "ready saved clean",
                load_state: LoadState::Ready,
                save_state: SaveState::Saved,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Normal,
            },
            Case {
                name: "unsaved edits",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "loading document",
                load_state: LoadState::Loading,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "checking save precondition",
                load_state: LoadState::Ready,
                save_state: SaveState::Checking,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "saving document",
                load_state: LoadState::Ready,
                save_state: SaveState::Saving,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "reconnecting document backend",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Reconnecting,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "loading large file page",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: true,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "load failed",
                load_state: LoadState::Failed("load failed".into()),
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "save failed",
                load_state: LoadState::Ready,
                save_state: SaveState::Failed("save failed".into()),
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "save conflict",
                load_state: LoadState::Ready,
                save_state: SaveState::Conflict,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "connection offline",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Offline("offline".into()),
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "large file page failed",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: true,
                expected: Error,
            },
            Case {
                name: "error outranks every processing source",
                load_state: LoadState::Failed("load failed".into()),
                save_state: SaveState::Saving,
                connection_state: DocumentConnectionState::Reconnecting,
                dirty: true,
                large_file_loading: true,
                large_file_error: true,
                expected: Error,
            },
        ];

        for case in cases {
            assert_eq!(
                document_workspace_status(
                    &case.load_state,
                    &case.save_state,
                    &case.connection_state,
                    case.dirty,
                    case.large_file_loading,
                    case.large_file_error,
                ),
                case.expected,
                "{}",
                case.name
            );
        }
    }
```

这组数据逐一出现 `LoadState` 的 3 个变体、`SaveState` 的 6 个变体、`DocumentConnectionState` 的 3 个变体，并分别驱动 `large_file.loading` 与 `large_file.error`。

- [ ] **Step 2：运行单一过滤测试并确认 RED**

Run: `cargo test document_workspace_status_covers_all_sources_and_priority -- --nocapture`

Expected: 编译失败，错误指出找不到 `document_workspace_status`。

- [ ] **Step 3：实现错误优先、处理中其次的文档状态函数**

在终端聚合函数之后、`#[cfg(test)]` 之前加入：

```rust
pub(crate) fn document_workspace_status(
    load_state: &LoadState,
    save_state: &SaveState,
    connection_state: &DocumentConnectionState,
    dirty: bool,
    large_file_loading: bool,
    large_file_error: bool,
) -> WorkspaceTabStatus {
    if matches!(load_state, LoadState::Failed(_))
        || matches!(save_state, SaveState::Failed(_) | SaveState::Conflict)
        || matches!(connection_state, DocumentConnectionState::Offline(_))
        || large_file_error
    {
        WorkspaceTabStatus::Error
    } else if matches!(load_state, LoadState::Loading)
        || matches!(save_state, SaveState::Checking | SaveState::Saving)
        || matches!(connection_state, DocumentConnectionState::Reconnecting)
        || large_file_loading
        || dirty
    {
        WorkspaceTabStatus::Attention
    } else {
        WorkspaceTabStatus::Normal
    }
}
```

- [ ] **Step 4：重新运行文档状态测试并确认 GREEN**

Run: `cargo test document_workspace_status_covers_all_sources_and_priority -- --nocapture`

Expected: 1 项测试通过；正常和已保存均为 `Normal`，dirty/加载/检查/保存/重连/分页加载均为 `Attention`，全部五类错误源均为 `Error`。

- [ ] **Step 5：提交文档状态核心**

```powershell
git add src/app/workspace_tabs.rs
git commit -m "feat(ui): classify remote document tab status"
```

### Task 3：语义色角色与内置主题蓝色

**Files:**
- Modify: `src/app/workspace_tabs.rs`
- Modify: `src/app/theme.rs:307-332`
- Modify: `assets/themes/ashell.json:17,33`
- Test: `src/app/workspace_tabs.rs`
- Test: `src/app/theme.rs`

- [ ] **Step 1：先写绿/蓝/黄/红/灰角色映射测试**

在 `WorkspaceTabStatus` 之后加入角色枚举：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabColorRole {
    Success,
    Blue,
    Warning,
    Danger,
    MutedForeground,
}
```

在 `tests` 模块末尾加入：

```rust
    #[test]
    fn workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics() {
        use WorkspaceTabColorRole::{Blue, Danger, MutedForeground, Success, Warning};
        use WorkspaceTabStatus::{Attention, Error, Normal, Unavailable};
        use WorkspaceTabVisualKind::{RemoteDocument, Terminal};

        let cases = [
            (Terminal, Normal, Success),
            (RemoteDocument, Normal, Blue),
            (Terminal, Attention, Warning),
            (RemoteDocument, Attention, Warning),
            (Terminal, Error, Danger),
            (RemoteDocument, Error, Danger),
            (Terminal, Unavailable, MutedForeground),
            (RemoteDocument, Unavailable, MutedForeground),
        ];

        for (visual_kind, status, expected) in cases {
            assert_eq!(
                workspace_tab_color_role(visual_kind, status),
                expected
            );
        }
    }
```

- [ ] **Step 2：运行单一过滤测试并确认 RED**

Run: `cargo test workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics -- --nocapture`

Expected: 编译失败，错误指出找不到 `workspace_tab_color_role`。

- [ ] **Step 3：实现类型和状态到语义色角色的映射**

在文档状态函数之后、`#[cfg(test)]` 之前加入：

```rust
pub(crate) fn workspace_tab_color_role(
    visual_kind: WorkspaceTabVisualKind,
    status: WorkspaceTabStatus,
) -> WorkspaceTabColorRole {
    match (visual_kind, status) {
        (WorkspaceTabVisualKind::Terminal, WorkspaceTabStatus::Normal) => {
            WorkspaceTabColorRole::Success
        }
        (WorkspaceTabVisualKind::RemoteDocument, WorkspaceTabStatus::Normal) => {
            WorkspaceTabColorRole::Blue
        }
        (_, WorkspaceTabStatus::Attention) => WorkspaceTabColorRole::Warning,
        (_, WorkspaceTabStatus::Error) => WorkspaceTabColorRole::Danger,
        (_, WorkspaceTabStatus::Unavailable) => WorkspaceTabColorRole::MutedForeground,
    }
}
```

- [ ] **Step 4：重新运行语义色角色测试并确认 GREEN**

Run: `cargo test workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics -- --nocapture`

Expected: 1 项测试通过，终端正常与文件正常分别映射 `Success` 和 `Blue`，其余三种状态不受视觉类型影响。

- [ ] **Step 5：先锁定三套主题的蓝色值**

用以下完整函数替换 `src/app/theme.rs` 中现有的 `bundled_themes_match_the_workspace_preview_palette`：

```rust
    #[test]
    fn bundled_themes_match_the_workspace_preview_palette() {
        let ashell_theme: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/ashell.json")).unwrap();
        let vscode_theme: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/vscode.json")).unwrap();

        let light = &ashell_theme["themes"][0]["colors"];
        assert_eq!(light["panel.background"], "#f5f5f5");
        assert_eq!(light["muted.background"], "#ededed");
        assert_eq!(light["secondary.hover.background"], "#e6e6e6");
        assert_eq!(light["sidebar.background"], "#f5f5f5");
        assert_eq!(light["tab.active.background"], "#151515");
        assert_eq!(light["base.blue"], "#2f7faa");

        let dark = &ashell_theme["themes"][1]["colors"];
        assert_eq!(dark["base.green"], "#a7d797");
        assert_eq!(dark["base.yellow"], "#d8ca77");
        assert_eq!(dark["base.red"], "#d75050");
        assert_eq!(dark["base.blue"], "#2f7faa");
        assert_eq!(dark["sidebar.background"], "#171717");
        assert_eq!(dark["tab.active.background"], "#f5f5f5");

        let vscode = &vscode_theme["themes"][0]["colors"];
        assert_eq!(vscode["muted.foreground"], "#a5a5a5");
        assert_eq!(vscode["sidebar.background"], "#252526");
        assert_eq!(vscode["tab.active.background"], "#094771");
        assert_eq!(vscode["base.blue"], "#569cd6");
    }
```

- [ ] **Step 6：运行主题夹具测试并确认 RED**

Run: `cargo test bundled_themes_match_the_workspace_preview_palette -- --nocapture`

Expected: 断言失败；JShell Light 的实际值仍是 `#202020`，JShell Dark 的实际值仍是 `#f5f5f5`。

- [ ] **Step 7：只修改两套 JShell 主题的 `base.blue`**

在 `assets/themes/ashell.json` 中执行两处精确替换：

```json
"base.yellow": "#816200", "base.blue": "#2f7faa", "base.magenta": "#4a4a4a"
```

```json
"base.yellow": "#d8ca77", "base.blue": "#2f7faa", "base.magenta": "#b8b8b8"
```

不要修改 `assets/themes/vscode.json`；它的 `base.blue` 必须继续为 `#569cd6`。

- [ ] **Step 8：重新运行主题夹具测试并确认 GREEN**

Run: `cargo test bundled_themes_match_the_workspace_preview_palette -- --nocapture`

Expected: 1 项测试通过，两个 JShell 主题均为 `#2f7faa`，VS Code Dark 仍为 `#569cd6`。

- [ ] **Step 9：提交语义色与主题值**

```powershell
git add src/app/workspace_tabs.rs src/app/theme.rs assets/themes/ashell.json
git commit -m "feat(theme): define workspace tab status colors"
```

### Task 4：接入渲染数据并重构共享标签结构

**Files:**
- Modify: `src/app/ui.rs:24-49,2350-2580`
- Test: `src/app/workspace_tabs.rs`

- [ ] **Step 1：记录旧标签结构基线**

Run: `rg -n -C 4 'accent|connected_ssh|size\(px\(6\.\)\)|format!\("\{\} •"|min_w\(px\(80\.\)\)' src/app/ui.rs`

Expected: `WorkspaceTabRenderData` 仍含 `accent`；终端颜色只读取首个 pane；文档标题拼接 ` •`；共享标签包含左侧 6 px 圆点且最小宽度为 80 px。

- [ ] **Step 2：导入状态模块并扩展 `WorkspaceTabRenderData`**

用以下完整 `use crate` 块替换 `src/app/ui.rs:24-35`：

```rust
use crate::{
    Ashell, PaneLayout,
    app::{
        constants::{
            COLLAPSED_SIDEBAR_WIDTH, COMPACT_ICON_SIZE, SFTP_STATUS_HEIGHT,
            SFTP_TOOLBAR_HEIGHT, SIDEBAR_PRIMARY_ACTION_HEIGHT, SIDEBAR_SECTION_HEIGHT,
            SIDEBAR_WIDTH, TAB_BAR_HEIGHT, TERMINAL_KEY_CONTEXT, TERMINAL_PADDING_X,
            TERMINAL_PADDING_Y, TERMINAL_SCROLLBAR_GUTTER,
        },
        workspace_tabs::{
            WorkspaceTabColorRole, WorkspaceTabStatus, WorkspaceTabVisualKind,
            aggregate_terminal_workspace_status, document_workspace_status, terminal_tab_status,
            workspace_tab_color_role,
        },
    },
    sftp::format_mtime,
    sftp::ops::is_editable_text_file,
    system::format_bytes,
    terminal,
};
```

用以下定义替换现有 `WorkspaceTabRenderData`：

```rust
struct WorkspaceTabRenderData {
    workspace_id: String,
    label: String,
    visual_kind: WorkspaceTabVisualKind,
    status: WorkspaceTabStatus,
    dirty: bool,
    connected_ssh: bool,
    close_target: WorkspaceCloseTarget,
}
```

`accent` 被删除；主题颜色只能在共享渲染阶段由 `visual_kind + status` 决定。

- [ ] **Step 3：让终端工作区逐一解析所有 pane 状态**

用以下完整 match arm 替换 `WorkspaceTab::Session` 分支：

```rust
                crate::document::WorkspaceTab::Session {
                    id: workspace_id,
                    group_id,
                } => {
                    let group = self.tab_groups.iter().find(|group| &group.id == group_id)?;
                    let pane_ids: Vec<String> = group
                        .pane_root
                        .tab_ids()
                        .iter()
                        .map(|id| id.to_string())
                        .collect();
                    let label = if pane_ids.len() > 1 {
                        format!("{} ({})", group.title, pane_ids.len())
                    } else {
                        group.title.clone()
                    };
                    let close_id = if self.active_group.as_ref() == Some(group_id) {
                        self.active_tab
                            .clone()
                            .or_else(|| pane_ids.first().cloned())
                            .unwrap_or_default()
                    } else {
                        pane_ids.first().cloned().unwrap_or_default()
                    };
                    let pane_statuses = pane_ids
                        .iter()
                        .map(|id| {
                            self.tabs
                                .iter()
                                .find(|tab| tab.id == *id)
                                .map(|tab| {
                                    terminal_tab_status(
                                        tab.connected,
                                        tab.disconnected_reason.as_deref(),
                                    )
                                })
                        })
                        .collect::<Vec<_>>();
                    let status = aggregate_terminal_workspace_status(&pane_statuses);
                    let connected_ssh = pane_ids.iter().any(|id| {
                        self.tabs.iter().any(|tab| {
                            tab.id == *id
                                && tab.kind == crate::terminal::TabKind::Ssh
                                && tab.connected
                        })
                    });
                    Some(WorkspaceTabRenderData {
                        workspace_id: workspace_id.clone(),
                        label,
                        visual_kind: WorkspaceTabVisualKind::Terminal,
                        status,
                        dirty: false,
                        connected_ssh,
                        close_target: WorkspaceCloseTarget::Session(close_id),
                    })
                }
```

每个非空 pane 标识都会执行一次 `self.tabs.iter().find`；未找到时保留 `None` 交给聚合器，不能通过 `filter_map` 丢掉缺失状态。工作区引用的 `TabGroup` 缺失时，最外层现有 `?` 仍跳过整个失效工作区。

- [ ] **Step 4：让远程文档同时消费 load/save/connection/large_file/dirty**

用以下完整 match arm 替换 `WorkspaceTab::RemoteDocument` 分支：

```rust
                crate::document::WorkspaceTab::RemoteDocument {
                    id: workspace_id,
                    document_id,
                } => {
                    let document = self.documents.get(document_id)?;
                    let dirty = document.revisions.is_dirty();
                    let large_file_loading = document
                        .large_file
                        .as_ref()
                        .is_some_and(|state| state.loading);
                    let large_file_error = document
                        .large_file
                        .as_ref()
                        .is_some_and(|state| state.error.is_some());
                    let status = document_workspace_status(
                        &document.load_state,
                        &document.save_state,
                        &document.connection_state,
                        dirty,
                        large_file_loading,
                        large_file_error,
                    );
                    Some(WorkspaceTabRenderData {
                        workspace_id: workspace_id.clone(),
                        label: document.title().to_string(),
                        visual_kind: WorkspaceTabVisualKind::RemoteDocument,
                        status,
                        dirty,
                        connected_ssh: false,
                        close_target: WorkspaceCloseTarget::Document(document_id.clone()),
                    })
                }
```

这里的 `label` 只含文件标题，不再拼接未保存字符；远程文档本身缺失时现有 `?` 继续跳过整个失效工作区。

- [ ] **Step 5：用唯一共享分支渲染色带、图标、标题、dirty 标记和关闭按钮**

用以下完整闭包替换从 `.children(tabs_data.iter().enumerate().map(|(ix, tab_data)| {` 开始、到其配对 `}))` 结束的当前闭包；外层 `h_flex().id("ashell-tab-bar")`、滚动句柄和尾部间隔元素 `div().flex_none().w_3().h_full()` 不变：

```rust
                            .children(tabs_data.iter().enumerate().map(|(ix, tab_data)| {
                                let workspace_id = tab_data.workspace_id.clone();
                                let close_target = tab_data.close_target.clone();
                                let is_selected = ix == selected;
                                let connected_ssh = tab_data.connected_ssh;
                                let foreground = if connected_ssh && is_selected {
                                    gpui::white()
                                } else if connected_ssh {
                                    cx.theme().muted_foreground
                                } else if is_selected {
                                    cx.theme().tab_active_foreground
                                } else {
                                    cx.theme().tab_foreground
                                };
                                let status_color = match workspace_tab_color_role(
                                    tab_data.visual_kind,
                                    tab_data.status,
                                ) {
                                    WorkspaceTabColorRole::Success => cx.theme().success,
                                    WorkspaceTabColorRole::Blue => cx.theme().blue,
                                    WorkspaceTabColorRole::Warning => cx.theme().warning,
                                    WorkspaceTabColorRole::Danger => cx.theme().danger,
                                    WorkspaceTabColorRole::MutedForeground => {
                                        cx.theme().muted_foreground
                                    }
                                };
                                let type_icon = match tab_data.visual_kind {
                                    WorkspaceTabVisualKind::Terminal => IconName::SquareTerminal,
                                    WorkspaceTabVisualKind::RemoteDocument => IconName::File,
                                };

                                h_flex()
                                    .id(("workspace-tab", ix))
                                    .flex_none()
                                    .relative()
                                    .min_w(px(104.))
                                    .max_w(px(240.))
                                    .h(px(32.))
                                    .pr(px(8.))
                                    .gap_2()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded_tl(px(6.))
                                    .rounded_tr(px(6.))
                                    .bg(if connected_ssh {
                                        gpui::black()
                                    } else if is_selected {
                                        cx.theme().tab_active
                                    } else {
                                        cx.theme().transparent
                                    })
                                    .text_color(foreground)
                                    .cursor_pointer()
                                    .when(!is_selected && !connected_ssh, |this| {
                                        this.hover(|this| this.bg(cx.theme().secondary_hover))
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.activate_workspace(workspace_id.clone(), window, cx)
                                    }))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(3.))
                                            .h(px(24.))
                                            .bg(status_color),
                                    )
                                    .child(Icon::new(type_icon).size(px(14.)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_size(rems(0.833))
                                            .when(is_selected, |this| {
                                                this.font_weight(FontWeight::SEMIBOLD)
                                            })
                                            .child(tab_data.label.clone()),
                                    )
                                    .when(tab_data.dirty, |this| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(foreground),
                                        )
                                    })
                                    .child(
                                        Button::new(("tab-close", ix))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Close)
                                            .text_color(foreground)
                                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                                match &close_target {
                                                    WorkspaceCloseTarget::Session(close_id)
                                                        if !close_id.is_empty() =>
                                                    {
                                                        this.close_tab(close_id.clone(), cx)
                                                    }
                                                    WorkspaceCloseTarget::Document(document_id) => {
                                                        this.request_close_document(
                                                            document_id.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    }
                                                    _ => {}
                                                }
                                            })),
                                    )
                                    .when(connected_ssh && is_selected, |this| {
                                        this.child(
                                            div()
                                                .absolute()
                                                .left(px(6.))
                                                .right(px(6.))
                                                .bottom_0()
                                                .h(px(2.))
                                                .rounded_tl(px(2.))
                                                .rounded_tr(px(2.))
                                                .bg(cx.theme().primary),
                                        )
                                    })
                            }))
```

关键布局事实：父标签删除 `.px_2()`，因此首个 3 px 子元素真正贴在标签最左侧；色带高 24 px，在 32 px 标签内由 `.items_center()` 形成上下各 4 px 间距；图标固定 14 px；只有标题是 `flex_1 + min_w(0) + truncate`；dirty 圆点和关闭按钮位于标题截断容器之外。色带不调用 `.opacity`，已连接 SSH 的黑底和底部 2 px 活动线保持原条件。

- [ ] **Step 6：执行结构检查**

Run: `rg -n -C 6 'w\(px\(3\.\)\)|IconName::SquareTerminal|IconName::File|min_w\(px\(104\.\)\)|max_w\(px\(240\.\)\)|when\(tab_data\.dirty' src/app/ui.rs`

Expected: 共享标签闭包中出现 3 px 色带、两个类型图标分支、104/240 px 宽度和标题之后的 dirty 条件元素。

Run: `rg -n 'tab_data\.accent|format!\("\{\} •"|opacity\(if connected_ssh|min_w\(px\(80\.\)\)' src/app/ui.rs`

Expected: 无输出；旧左侧状态圆点的数据字段、标题内未保存字符、旧圆点透明度和 80 px 最小宽度均已移除。

Run: `rg -n -C 8 'when\(tab_data\.dirty' src/app/ui.rs`

Expected: dirty 元素是独立的 `flex_none` 6 px 圆点，代码顺序位于 `.truncate()` 标题之后、`Button::new(("tab-close", ix))` 之前。

- [ ] **Step 7：运行三个纯逻辑测试和编译检查**

Run: `cargo test terminal_workspace_status_aggregates_every_pane_by_priority -- --nocapture`

Expected: 通过。

Run: `cargo test document_workspace_status_covers_all_sources_and_priority -- --nocapture`

Expected: 通过。

Run: `cargo test workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics -- --nocapture`

Expected: 通过。

Run: `cargo check --all-targets`

Expected: 退出码 0；`cx.theme().blue`、`IconName::SquareTerminal`、`IconName::File` 和新模块可正确解析。

- [ ] **Step 8：提交共享标签渲染**

```powershell
git add src/app/ui.rs
git commit -m "feat(ui): render workspace tab type and status"
```

### Task 5：完整静态检查与 Debug 前端交互验证

**Files:**
- No code changes expected

- [ ] **Step 1：运行格式检查**

Run: `cargo fmt --check`

Expected: 退出码 0；若失败，先运行 `cargo fmt`，再重新运行 `cargo fmt --check` 并确认通过。

- [ ] **Step 2：逐个运行本功能的四个单一过滤测试**

Run: `cargo test terminal_workspace_status_aggregates_every_pane_by_priority -- --nocapture`

Expected: 通过。

Run: `cargo test document_workspace_status_covers_all_sources_and_priority -- --nocapture`

Expected: 通过。

Run: `cargo test workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics -- --nocapture`

Expected: 通过。

Run: `cargo test bundled_themes_match_the_workspace_preview_palette -- --nocapture`

Expected: 通过。

- [ ] **Step 3：运行完整测试、全目标检查与严格 Clippy**

Run: `cargo test --quiet`

Expected: 全部测试通过。

Run: `cargo check --all-targets`

Expected: 退出码 0。

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: 退出码 0，无 warning。

- [ ] **Step 4：构建并启动 Debug 桌面程序**

Run: `cargo build`

Expected: 生成 `target/debug/jshell.exe`。

Run: `.\target\debug\jshell.exe`

Expected: JShell 窗口启动，无 panic、重复元素 ID 或主题解析错误。

- [ ] **Step 5：验证终端标签状态和聚合优先级**

按以下顺序操作并逐项记录结果：

1. 打开本地终端：标签显示 `SquareTerminal`，左侧为绿色 3 px 色带。
2. 打开一个已有可用 SSH 会话：连接建立前色带为黄色，成功后变为绿色；成功连接的 SSH 标签保留黑底，选中时保留底部 2 px 活动线。
3. 打开一个指向不可达端口的 SSH 会话：`connected=false` 且尚无 `disconnected_reason` 的等待阶段为黄色；后端报告失败并写入断开原因后，同一标签变为红色，而不是从一开始就显示红色。
4. 在同一工作区分屏放置一个正常终端和一个失败终端：整个工作区标签为红色，证明错误不会被第一个正常 pane 遮蔽。
5. 在同一工作区分屏放置一个正常终端和一个仍在连接的终端：整个工作区标签为黄色。

- [ ] **Step 6：验证远程文档的正常、dirty、处理和错误状态**

使用已有可用 SSH/SFTP 会话逐项操作：

1. 打开已加载且未修改的远程文件：标签显示 `File`，左侧色带为蓝色。
2. 编辑文件但不保存：色带变为黄色，标题与关闭按钮之间出现独立圆点。
3. 保存文件：检查 `Checking` 和 `Saving` 阶段为黄色；保存完成且 dirty 清除后，圆点消失、色带恢复蓝色。
4. 触发远端版本冲突或只读/权限导致的保存失败：色带为红色，文档工具栏原有冲突或失败文字仍可见。
5. 打开 10 MiB 到 100 MiB 的分页只读文件：分页读取期间色带为黄色；在分页请求期间断开对应 SSH 连接，使 `large_file.error` 写入错误后，色带变为红色。
6. 在文档保留打开时中断其连接：`DocumentConnectionState::Reconnecting` 时为黄色，`Offline` 时为红色。

- [ ] **Step 7：验证三套主题、窄窗口和大量标签布局**

依次切换 `JShell Light`、`JShell Dark`、`VS Code Dark`，在每套主题中检查绿色 `success`、蓝色 `blue`、黄色 `warning`、红色 `danger` 和灰色 `muted_foreground` 色带均可辨认；两套 JShell 主题的正常文件蓝色应一致，VS Code Dark 保留自身蓝色。

把窗口缩窄并打开足够多的终端与远程文件，使标签栏产生横向滚动，然后确认：

1. 每个可见标签都有最左侧 3 px 色带和固定类型图标。
2. 单个标签不窄于 104 px、不宽于 240 px；标题承担截断，图标、dirty 圆点和关闭按钮不被压扁或覆盖。
3. 使用一个超过 240 px 显示宽度的远程文件名并制造 dirty：文件名被截断时，独立 dirty 圆点和关闭按钮仍可见。
4. 鼠标悬停、点击切换、关闭终端标签、关闭 dirty 文档确认框和横向滚动均保持原行为。
5. 活动标签仍为半粗体和活动背景；已连接 SSH 仍为黑底，未选中 SSH 的文字样式保持原样。

- [ ] **Step 8：最终差异与范围检查**

Run: `git diff --check`

Expected: 无空白错误。

Run: `git status --short`

Expected: 功能实现只涉及 `src/app/mod.rs`、`src/app/workspace_tabs.rs`、`src/app/ui.rs`、`src/app/theme.rs` 和 `assets/themes/ashell.json`；不包含持久化格式、文档编辑器、SFTP、命令栏、Release 工作流或发布脚本改动。

## 验收映射

- 所有标签 3 px 左侧色带、`SquareTerminal` / `File` 图标、104/240 px 宽度、共享渲染分支：Task 4 Steps 2-6。
- 绿色/黄色/蓝色/红色/灰色语义及 `Error > Attention > Unavailable > Normal`：Task 1、Task 3。
- pane 缺失与空工作区不得判正常：Task 1 的 `None`/空集合用例，Task 4 的逐 pane 查找接线。
- 文档 `load_state`、`save_state`、`connection_state`、`large_file.loading/error` 全覆盖：Task 2 的 14 项表驱动用例，Task 4 Step 4 的生产接线。
- dirty 圆点位于标题截断容器外、旧状态圆点删除：Task 4 Steps 4-6。
- JShell Light/Dark `base.blue=#2f7faa` 且 VS Code Dark 不变：Task 3 Steps 5-8。
- 单元测试、严格静态检查和桌面交互验证：Task 5。
