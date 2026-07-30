# 远程文件编辑与混合工作区实施计划

> **For agentic workers:** 按任务顺序执行。实现与测试完成前不得提交；提交、推送和发布只在最后一个任务统一进行。

**目标：** 完成已确认的远程文件管理、SFTP 断线恢复、终端/编辑器混合工作区、独立编辑器窗口、动态标题和项目收尾。

**架构：** 将终端与远程文档统一为类型安全的 Pane 树；每个 SSH 工作区保留一个稳定的 SFTP 句柄，由内部 supervisor 管理连接代次和退避重连。文档标签、编辑器 Pane、独立编辑器窗口共享同一个文档模型和保存状态。

**技术栈：** Rust 2024、GPUI、gpui-component、Tokio、russh、russh-sftp、serde、Tree-sitter。

---

## 执行约束

- 实际仓库：`D:/Git/ashell`。
- 保留当前所有未提交修改，不使用 `git reset --hard`、`git checkout --` 或其他破坏性回退。
- 发布前校正 GitHub Actions 与三平台产物名；发布任务只能写入当前 `JayhaShf/ashell` 仓库，不得更新上游仓库或上游 Homebrew Tap。
- 默认用户界面和新增文档使用中文；保留英文 README 的结构，但不让它覆盖中文默认文档。
- 测试通过前不提交。每个任务可以形成工作区检查点，但不创建提交。
- 删除旧资源或无用代码前必须通过 `git grep`、Rust 编译/Clippy 和构建引用检查确认没有消费者。

## 文件边界

- 修改：`src/app/mod.rs`、`src/app/ui.rs`、`src/app/startup.rs`、`src/app/search.rs`、`src/document/mod.rs`、`src/document/controller.rs`、`src/document/ui.rs`、`src/session/mod.rs`、`src/session/config.rs`、`src/sftp/mod.rs`、`src/sftp/ops.rs`、`src/terminal/mod.rs`、`build.rs`、`Cargo.toml`、`scripts/package-macos-app.sh`、`assets/jshell.desktop`、`.github/workflows/release.yml`、`README.md`。
- 新增：`src/sftp/permissions.rs`、`src/sftp/connection.rs`、`src/document/window.rs`；分别承载权限格式化、SFTP 连接状态机和独立窗口 host，避免把新逻辑继续堆入现有大模块。
- 保留并接入：`assets/icons/jshell.png`、`assets/icons/jshell.ico`、`assets/icons/jshell.icns`。
- 不触碰：用户未要求的主题视觉；图标母版仅作为发布资源保留，不重新处理像素。

## Task 1：建立基线并整理当前差异

**Files:**
- Inspect: 当前 `git status`、`git diff`、`git log`、`docs/superpowers/specs/2026-07-30-remote-editor-workspace-design.md`
- Test: 现有 Rust 全量测试和格式检查

- [ ] **Step 1: 记录当前工作树基线**

运行：

```text
git status --short --branch
git diff --stat
cargo fmt --check
cargo test --locked --quiet
cargo clippy --locked --all-targets --all-features -- -D warnings
git diff --check
```

预期：当前已有修改保持不变；若基线测试失败，先记录失败文件和原因，不将失败归因于本轮新代码。

- [ ] **Step 2: 建立本轮变更清单**

按功能记录现有未提交文件：B1 标签、Shiki 评估、图标资源、中文设计和历史文档。后续只在清单内合并，不覆盖用户修改。

## Task 2：接入 JShell 图标和身份资源

**Files:**
- Modify: `build.rs:9`、`src/app/startup.rs` 的运行时图标 include、`Cargo.toml` 的 deb 资源、`scripts/package-macos-app.sh` 的 icns 名称、`assets/jshell.desktop`
- Use: `assets/icons/jshell.png`、`assets/icons/jshell.ico`、`assets/icons/jshell.icns`
- Test: 资源引用静态检查和 Rust 构建

- [ ] **Step 1: 先写资源引用检查**

确认以下引用全部指向 JShell 资源：

```text
build.rs -> assets/icons/jshell.ico
startup.rs -> assets/icons/jshell.png
Cargo.toml -> assets/icons/jshell.png
package-macos-app.sh -> assets/icons/jshell.icns
assets/jshell.desktop -> Icon=jshell
```

同时确认 `Name=JShell`、`StartupWMClass=jshell` 和可执行文件名一致。

- [ ] **Step 2: 修改资源引用**

替换资源文件名和用户可见的旧 Ashell 应用名；发布 workflow 同步改为 JShell 产物并移除上游 Tap 写入，不修改图标像素。

- [ ] **Step 3: 验证资源**

运行：

```text
Test-Path assets/icons/jshell.png
Test-Path assets/icons/jshell.ico
Test-Path assets/icons/jshell.icns
cargo check --locked --all-targets
```

预期：三个文件存在，构建资源可解析，旧 `ashell` 图标引用只剩在尚未清理的兼容文件中并有明确消费者。

## Task 3：实现远程权限模型和文件列表交互

**Files:**
- Modify: `src/sftp/mod.rs` 的 `RemoteEntry` 和目录枚举、`src/sftp/ops.rs` 的选择/打开逻辑、`src/app/ui.rs` 的 SFTP 行渲染
- Create: `src/sftp/permissions.rs` 的纯逻辑权限格式化和测试

- [ ] **Step 1: 写权限格式化失败测试**

覆盖普通文件、目录、符号链接、特殊位和未知权限：

```rust
assert_eq!(format_permissions(Some(0o100644)), "-rw-r--r-- 0644");
assert_eq!(format_permissions(Some(0o040755)), "drwxr-xr-x 0755");
assert_eq!(format_permissions(None), "--");
```

测试必须使用真实权限转换函数，不能只断言 UI 字符串常量。

- [ ] **Step 2: 扩展 `RemoteEntry`**

保留现有调用兼容性，同时增加远程类型和 `permissions: Option<u32>`。从 `FileAttributes.permissions` 读取类型位和权限位；权限缺失时传递 `None`。

- [ ] **Step 3: 实现权限格式化并使测试通过**

类型位使用远程 Unix 语义；符号权限包括用户、组、其他用户以及 setuid、setgid、sticky 位。八进制输出使用四位权限数字，未知值输出 `--`。

- [ ] **Step 4: 写文件交互失败测试**

为条目动作增加纯逻辑分派测试：单击只聚焦，双击文件触发编辑，双击目录触发导航，复选框只切换选择。

- [ ] **Step 5: 修改 SFTP 行事件**

行容器的单击只调用 `mark_sftp_entry_selected`；双击根据 `is_dir` 分派 `navigate_sftp` 或 `open_remote_document`。复选框事件阻止向行容器冒泡，避免单击复选框触发行打开逻辑。

- [ ] **Step 6: 完善删除状态**

删除确认只提交一次，删除过程中禁用相关操作；后端完成后刷新父目录并显示成功或逐项失败原因。保留目录递归删除能力，不在断线时自动重放。

## Task 4：持久化全局自动换行并校正保存状态

**Files:**
- Modify: `src/session/config.rs`、`src/document/mod.rs`、`src/document/controller.rs`、`src/document/ui.rs`、中文 locale 文件
- Test: `src/session/config.rs`、`src/document/remote.rs` 和 controller 纯逻辑测试

- [ ] **Step 1: 写配置兼容失败测试**

从没有新字段的旧配置反序列化，断言 `editor_soft_wrap == false`；序列化新配置后断言字段存在且值可恢复。

- [ ] **Step 2: 增加全局配置字段**

在 `ConfigFile` 和 `ConfigStore` 增加带 `serde(default)` 的布尔字段、getter 和 setter。setter 使用现有偏好保存路径，不创建第二套配置文件。

- [ ] **Step 3: 让文档使用全局默认值**

创建文档和分页查看器时读取全局设置；切换任意文档的自动换行时更新配置并把值应用到所有打开文档和查看器。

- [ ] **Step 4: 写保存结果未知测试**

模拟写入请求的 oneshot 在远端操作期间关闭，断言文档保持 dirty，状态为结果未知/失败，不调用自动保存或二次写入。

- [ ] **Step 5: 完善保存状态 UI**

为 `SaveState` 增加结果未知语义或等价的明确状态文本；断线时保存按钮禁用但文本保留，重连后只恢复手动保存入口。标签页、编辑器工具栏和底部状态栏共用同一状态源。

## Task 5：把 PaneLayout 迁移为混合 Pane 树

**Files:**
- Modify: `src/app/mod.rs`、`src/session/mod.rs`、`src/app/ui.rs`、`src/app/search.rs`、`src/document/mod.rs`、`src/document/controller.rs`、`src/document/ui.rs`
- Create: `src/app/pane_layout.rs`，承载 `PaneLeaf`、`PaneLayout` 和布局纯逻辑测试

- [ ] **Step 1: 写混合布局失败测试**

覆盖以下行为：

```rust
let layout = PaneLayout::Vertical(
    vec![
        PaneLayout::Leaf(PaneLeaf::Terminal("ssh".into())),
        PaneLayout::Leaf(PaneLeaf::Document("doc".into())),
    ],
    0.5,
);
assert_eq!(layout.terminal_ids(), vec!["ssh"]);
assert_eq!(layout.document_ids(), vec!["doc"]);
assert!(matches!(
    layout.focused_leaf(&[1]),
    Some(PaneLeaf::Document(id)) if id == "doc"
));
```

另测右侧插入、删除文档后压缩树、相邻焦点和 `Empty` 不计入有效终端。

- [ ] **Step 2: 引入类型安全叶子**

新增 `PaneLeaf::{Terminal(String), Document(String), Empty}`，并把布局叶子改为 `PaneLayout::Leaf(PaneLeaf)`。提供 `leaves`、`terminal_ids`、`document_ids`、`contains_terminal`、`contains_document`、`remove_leaf` 和焦点查询方法。

- [ ] **Step 3: 迁移终端会话逻辑**

更新终端打开、关闭、终端分屏、焦点导航、系统监视、组激活和组状态同步。所有这些路径只操作 `Terminal` 叶子；文档叶子不能触发新终端、SSH 重连或系统采样。

- [ ] **Step 4: 接入文档打开和关闭**

打开文档时在当前 SSH 组右侧插入 `Document(document_id)`，重复路径只移动焦点。关闭文档时删除叶子并压缩树；若当前焦点是文档，恢复相邻 Pane 和 `active_tab`。

- [ ] **Step 5: 修正工作区状态语义**

`active_group` 保留 SFTP 所属组，`active_tab` 只保留终端 ID，当前焦点内容从 Pane 路径派生。文件标签仍是一个顶层 UI 引用，但不创建第二份文档模型。

- [ ] **Step 6: 渲染混合 Pane**

`render_pane_tree` 对 `Terminal` 调用现有终端渲染，对 `Document` 调用文档渲染；文档组件 ID必须包含 document ID，避免多个编辑器共享固定 GPUI ID。终端面板下方的 SFTP/监视面板继续绑定当前 SSH 组。

- [ ] **Step 7: 覆盖混合工作区行为测试**

验证文档位于右侧、SFTP 仍绑定原组、文档焦点不污染终端搜索/复制/滚动、关闭最后终端会进入 detached 文档路径。

## Task 6：实现 SFTP supervisor 和断线恢复

**Files:**
- Modify: `src/sftp/mod.rs`、`src/document/remote.rs`、`src/document/controller.rs`、`src/app/mod.rs`、`src/app/ui.rs`、`src/terminal/mod.rs`
- Create: `src/sftp/connection.rs` for backoff、连接状态和命令分类纯逻辑
- Test: supervisor 状态机、命令生命周期和文档后端测试

- [ ] **Step 1: 写退避和命令分类失败测试**

测试退避序列 `1/2/4/8/16/30`、成功重置、关闭取消；测试读取命令可重试，写入/删除/上传/下载不可自动重放。

- [ ] **Step 2: 建立稳定句柄和连接状态模型**

保留一个 `SftpHandle` 命令发送端，supervisor 内部持有可替换连接、home 和 connection generation。新增专用连接状态事件，事件至少包含 workspace ID、generation、状态和错误文本。

- [ ] **Step 3: 把连接建立改为 supervisor 循环**

连接失败或传输层关闭后进入重连状态，按照退避等待；成功后刷新 home 和当前目录。窗口/工作区关闭时取消循环并结束所有待处理请求。

- [ ] **Step 4: 迁移文档请求的完成路径**

不要让 stat/read/write 子任务脱离 supervisor。每个请求有唯一 ID和唯一 oneshot 完成路径；读取请求可在明确连接错误时重试，保存请求在结果未知时只返回一次失败/未知结果。

- [ ] **Step 5: 迁移传输和副作用命令**

连接断开时终止或标记当前传输，清理临时状态；不把副作用命令重新放回队列。应用层保留手动重试按钮。

- [ ] **Step 6: 处理旧事件和共享句柄**

应用层按 workspace/generation 丢弃旧 SFTP 事件；文档重连改为恢复共享 supervisor，不再为每个文档创建新的 SFTP supervisor。最后一个终端关闭时，文档持有的句柄继续工作，所有文档关闭后才释放。

- [ ] **Step 7: 增加中文连接状态文案和 UI**

SFTP 面板显示连接中、已连接、正在重连、离线、最近错误和立即重连按钮；状态变化不复用终端 `Closed`。

- [ ] **Step 8: 运行 supervisor 定向测试**

先运行新增测试和 `cargo test --locked --quiet sftp`；确认所有请求在断线、取消和句柄关闭时都结束，再继续全量验证。

## Task 7：实现独立编辑器窗口

**Files:**
- Modify: `src/app/startup.rs`、`src/app/mod.rs`、`src/document/controller.rs`、`src/document/ui.rs`、`src/app/ui.rs`
- Create: `src/document/window.rs`（共享文档引用和窗口生命周期）
- Test: 窗口标题/关闭决策和文档 host 状态纯逻辑测试

- [ ] **Step 1: 写窗口关闭失败测试**

清洁文档关闭应允许窗口退出；dirty 文档关闭应进入现有保存确认；原 SSH 组已关闭时关闭独立窗口不得删除仍打开的文档模型。

- [ ] **Step 2: 增加共享文档窗口 host**

独立窗口只保存主 `Ashell` 实体引用和 document ID，不复制文本、InputState、backend 或保存状态。文档只能有一个可编辑 host，避免两个窗口同时写同一输入状态。

- [ ] **Step 3: 打开、移出和重挂窗口**

从分屏编辑器工具栏提供独立窗口操作；移出时从 Pane 树删除文档并记录原位置，关闭时按原位置重挂，原组不存在时保持 detached 文档视图。

- [ ] **Step 4: 复用保存保护和标题**

独立窗口关闭流程复用主应用 dirty 文档队列；窗口图标使用 JShell；窗口标题为 `JShell - 文件名`，dirty 时附加 `*`。

## Task 8：动态窗口标题和任务栏状态

**Files:**
- Modify: `src/app/startup.rs`、`src/app/ui.rs`、`src/app/mod.rs`、`src/terminal/mod.rs`、`src/document/ui.rs`
- Test: 标题格式纯逻辑测试

- [ ] **Step 1: 写标题格式失败测试**

覆盖默认窗口、终端、混合工作区当前文件、多文件聚焦文件、独立编辑器和 dirty 标记。

- [ ] **Step 2: 实现统一标题计算**

混合工作区只显示当前聚焦文件名，例如 `JShell - nginx.conf`，不拼接 SSH 会话名；无文件时回退到会话名或本地终端名。标题不得包含密码、完整敏感路径或配置内容。

- [ ] **Step 3: 接入状态变更**

工作区切换、Pane 焦点变化、终端动态标题、文档保存状态和独立窗口打开/关闭时同步系统窗口标题，并缓存上次标题避免重复写入。

## Task 9：项目清理和中文 README 重写

**Files:**
- Rewrite: `README.md`
- Update as required: `README.en.md`、`assets/jshell.desktop`、`Cargo.toml`、`scripts/package-macos-app.sh`、`.github/workflows/release.yml`
- Cleanup candidates: 通过引用检查确认后的旧 `ashell` 图标、无消费者函数、旧用户可见名称

- [ ] **Step 1: 生成当前功能清单**

README 中文版按以下顺序重写：项目简介、核心能力、会话与代理、SFTP 文件管理、混合编辑器、断线恢复、主题与标签、配置路径、开发与构建、验证、许可证、致谢。

- [ ] **Step 2: 校正项目身份**

用户可见名称统一为 JShell；保留配置文件兼容路径和必要的 Rust 模块名，避免无意义的大范围重命名。默认 `README.md` 使用中文。

- [ ] **Step 3: 清理已确认无用残留**

对 `#[allow(dead_code)]`、无调用函数、旧图标引用和过时 Ashell 用户文案逐项执行 `git grep`、`cargo check` 和 `cargo clippy`。只有确认无消费者的代码/资源才删除；终端高亮中的待办和修复提示关键词属于产品功能，不把业务关键词误删为死代码。

- [ ] **Step 4: 确保 README 最后是致谢**

许可证说明放在致谢之前，README 文件最后明确感谢原项目作者 TomZz、`rust-kotlin/ashell` 及上游贡献者，不在致谢之后追加其他章节。

## Task 10：发布前整合、验证和分批提交

**Files:**
- Inspect all current diff
- Modify only files required by the confirmed design and cleanup
- Preserve: `AUDIT_REPORT.md`、设计文档和实施计划

- [ ] **Step 1: 整合当前所有修改**

按功能审阅工作区差异：代理/SSH、标签与命令栏、图标、Shiki 评估、文档、文件管理器、混合 Pane、SFTP supervisor、README。删除重复实现和过时说明，确保当前代码与 README、设计文档一致。

- [ ] **Step 2: 执行完整验证**

运行：

```text
cargo fmt --check
cargo test --locked --quiet
cargo check --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release
cargo audit
git diff --check
```

`cargo audit` 的已知无修复漏洞和维护性告警必须记录实际输出，不得伪装成通过；其他命令必须零失败。

- [ ] **Step 3: 运行应用级冒烟检查**

至少验证：启动窗口图标和标题、创建 SSH 会话、SFTP 目录双击进入、单击文件不勾选、双击文件打开右侧编辑器、切换自动换行后重启仍保持、断开/恢复 SFTP、保存状态、独立窗口和关闭保护、Windows 任务栏标题。

- [ ] **Step 4: 分批提交**

测试全部通过后按逻辑批次提交，建议顺序：

1. `feat: use JShell identity and resources`
2. `feat: improve remote file manager and editor persistence`
3. `feat: add mixed terminal document workspace`
4. `feat: add resilient SFTP supervision`
5. `docs: rewrite Chinese README and record release audit`
6. `ci: publish JShell release artifacts`

每个提交前检查 staged diff，不提交 `.superpowers/`、`target/`、临时日志和未跟踪生成物；保留用户明确需要的图标母版和审计文档。

- [ ] **Step 5: 提交后发布前检查**

确认提交历史、工作树、远端分支和版本信息；检查发布包使用 JShell 图标、README 与实际功能一致、最终文件末尾是对原项目作者的感谢。没有用户明确批准前不强制推送；本轮用户已允许测试通过后提交和进入发布流程，因此在验证完成后执行对应发布步骤并记录结果。
