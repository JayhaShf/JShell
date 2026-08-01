# JShell 项目审计报告

- 审计日期：2026-08-01
- 审计对象：`0.1.0-beta.2` 本地发布候选
- 审计范围：当前修改涉及的分屏、配置写入、R2 手动同步、凭据保护、载荷校验、发布工作流、文档与 Release 产物
- 范围说明：本轮按用户要求只复审当前修改及其直接依赖，不重新审计未改动的全部业务模块。

## 1. 审计结论

当前修改范围内未发现仍未处理的 P0 或 P1 问题。此前审查发现的嵌套分屏比例、配置并发写盘、同步凭据部分提交、旧预览/冲突误用、非法快捷键载荷崩溃风险和 Release 工作流权限过宽均已修复并加入回归测试。

`RUSTSEC-2023-0071` 对应的 RustCrypto `rsa` crate 不在最终锁定依赖图中。2026-08-01 获取的 RustSec 官方公告库提交 `685d32fd681b540aa64019820639613c5a4fd922` 已用于本地审计，1177 条公告扫描 1024 个锁定依赖后返回成功。

本地 Release 已重新构建并通过隔离配置启动冒烟。当前仍未执行的是分批提交、推送后的 GitHub Actions、`v0.1.0-beta.2` 标签和 GitHub Release；因此本报告将该版本判定为“本地发布候选已通过门禁，待推送与托管发布验证”，不声明已经发布。

## 2. 本轮修复

### 2.1 分屏与界面状态

- 每个嵌套 split 容器记录自己的实际 Bounds，拖动比例不再错误使用整个 pane 根区域。
- 回归场景固定为：根宽 1000、嵌套宽 200、水平拖动 20 像素，ratio 变化 10%。
- 保留 5 像素死区、10%-90% 限制、释放鼠标后清理拖动状态。

### 2.2 配置写入一致性

- 新增统一的配置写入协调器，后台偏好保存与会话、文件夹、传输记录、导入配置、窗口布局和同步写盘共用串行锁与版本屏障。
- 同步或其他直接写入会淘汰更早排队的偏好快照，避免旧快照覆盖新 sessions、ETag 或偏好。
- 所有生产路径中的直接 `ConfigStore::save()` 已接入统一协调器；初始化迁移和单元测试除外。

### 2.3 R2、凭据与预览事务

- 上传成功和下载确认先验证并更新系统凭据库，再原子写入本地配置；任一步失败都会保留或恢复旧状态。
- 本地配置写入失败时恢复先前的同步密码和 R2 Secret；先前不存在的条目会被删除，不留下孤儿凭据。
- 关闭“记住同步密码”时，删除失败不会把本地标记写成关闭。
- 下载预览采用确认时的当前“记住密码”选择。
- provider 或同步表单发生变化时，旧下载预览和上传冲突操作立即失效。
- 预览和覆盖确认同时校验目标指纹与完整远端连接快照；R2 Access Key ID 改变时，即使桶和对象路径相同，也不能应用旧预览。

### 2.4 同步载荷校验

- v1/v2 Session 和 v2 SessionFolder 使用 `deny_unknown_fields` 的严格 DTO，再显式转换为运行时类型。
- 快捷键 action 仅允许应用实际支持的动作；按键序列逐 stroke 使用 GPUI `Keystroke::parse` 校验。
- 合法 chord、空字符串和 `none` 保持兼容；未知动作、非法语法和仅空白值会被拒绝，不再进入可能 panic 的运行时绑定路径。
- 预览中的偏好类别数改为与当前本地配置比较后的实际变化数，不再固定显示 3。

### 2.5 发布工作流

- CI 和 Release 顶层权限收紧为 `contents: read`，仅发布 job 使用 `contents: write`。
- 所有 checkout 均设置 `persist-credentials: false`。
- 18 个 Action 引用全部固定到 40 位提交 SHA，并已根据官方 ref 复核。
- `cargo-audit` 固定为 `0.22.0`。
- Release 使用固定 RustSec 公告库提交并加 `--no-fetch`，历史标签重跑不会因公告库漂移而改变结果；日常 CI 仍检查当前公告库。

## 3. 最终本地验证

以下命令均在 `D:\Git\ashell` 的最终工作树执行：

| 检查 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo test --locked --quiet` | 316 passed，0 failed |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | 通过 |
| `cargo audit --deny warnings --file Cargo.lock --db <clean-db> --no-fetch` | 通过；1177 条公告，1024 个依赖 |
| `cargo tree -i rsa --locked` | 未匹配到 `rsa` crate |
| GitHub Actions YAML 与固定 SHA 静态检查 | 通过 |
| `git diff --check` | 通过 |
| `cargo build --locked --release` | 通过 |
| 隔离 HOME/USERPROFILE 启动冒烟 | 通过，测试进程已关闭 |

最终 Windows 产物：

- 路径：`D:\Git\ashell\target\release\jshell.exe`
- 大小：79,746,560 字节
- SHA-256：`F8BF82325F1F74D90F02AEB27BF981FA563F50CE1118A4CFBA5FB5DFD4A2FA11`
- 构建时间：2026-08-01 18:55:52（Asia/Shanghai）

启动冒烟前后，用户现有配置文件 SHA-256 均为：

`B6D937EA3591FCBE1C52465BCD8808F94491F7C9E7BDFBB4572FCA6A69872461`

测试结束后不存在由本轮启动而残留的 JShell 进程。

## 4. 已知边界与发布前事项

- 系统凭据库与配置文件属于两个独立存储，代码已对可报告错误执行补偿回滚；若操作系统在两次存储操作之间被强制终止，仍可能需要用户重新输入同步凭据。
- 本轮未使用真实 Cloudflare R2 账户写入对象；网络语义由本地 HTTP/AWS SigV4 集成测试验证，GUI 已验证 provider 切换、窄窗口布局和错误状态。
- 真实 Linux SSH/SFTP 上传、删除、断线恢复以及远端编辑冲突仍建议在非生产测试主机上做发布前人工复核。
- 推送后必须等待 CI 全部通过，再创建 `v0.1.0-beta.2` 标签；旧标签 `v0.1.0-beta.1` 不移动、不覆盖。

## 5. 发布判定

当前判定：**本地发布候选门禁通过，可以分批提交并推送 `main`；托管 CI 通过后进入标签和 Release 流程。**

详细清单见 [JShell v0.1.0-beta.2 发布候选审计与验收记录](docs/RELEASE_AUDIT_0.1.0-beta.2.md)。
