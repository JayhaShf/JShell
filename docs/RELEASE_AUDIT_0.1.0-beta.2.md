# JShell v0.1.0-beta.2 发布审计与任务完成记录

审计日期：2026-07-31
审计范围：当前分支相对 `v0.1.0-beta.1` 的依赖、安全、品牌和发布收尾修改
审计原则：复核本轮修改及此前对话要求的落地状态，不重新审计无关模块

## 1. 结论

此前对话要求的代码、中文文档、品牌资源和构建流程均已落地。`RUSTSEC-2023-0071` 对应的 RustCrypto `rsa` crate 已从最终依赖图移除，`cargo audit` 当前无漏洞或维护性告警。旧标签 `v0.1.0-beta.1` 保持不动，本轮版本递增为 `0.1.0-beta.2`。

## 2. 功能完成度

| 范围 | 当前状态 |
|---|---|
| P0 主机密钥校验、P1 配置凭据保护与原子保存 | 已实现并有回归测试 |
| SSH 直连、全局代理、自定义代理和统一 SOCKS5/SOCKS5H/HTTP/HTTPS 入口 | 已实现并有路由、TLS 与失败关闭测试 |
| 命令输入显示开关移到底部，保留大文件分页按钮 | 已实现；分页按钮位于大文件底部工具栏 |
| B1 标签方案和全部标签状态色带 | 已实现；终端、文件、警告、错误和离线状态语义有测试 |
| Linux 文件权限、删除、单击聚焦、复选框选择、双击目录/文件 | 已实现并有 SFTP 操作测试 |
| SFTP 连接监视、退避重连和副作用操作不自动重放 | 已实现并有连接代次与重放策略测试 |
| 终端/编辑器混合分屏、编辑器独立窗口、自动换行持久化 | 已实现并有布局与窗口状态测试 |
| 断线保留编辑内容、保存状态、冲突和结果未知语义 | 已实现；断线不自动保存、不盲目重试 |
| 混合工作区标题只显示当前文件名，Windows 任务栏动态标题 | 已实现并有窗口标题测试 |
| Tree-sitter 多语言语法着色与 Shiki 评估 | 已完成；Shiki 不接入原生 GPUI 热路径 |
| SSH 终端 Ctrl+滚轮缩放 | 已接入终端滚轮事件，按 0.5 px 步进并持久化字体设置 |
| JShell 名称、PNG/ICO/ICNS、Windows 版本资源和三平台发布流程 | 已统一；旧 `ashell` 路径仅保留配置迁移和远端临时文件兼容用途 |
| 双语 README 重写、中文默认文档和项目致谢 | 已完成；README 截图已替换为脱敏 JShell 窗口图 |

## 3. RSA 安全修复

JShell 继续支持读取 RSA 私钥和 RSA 用户认证，但只允许 `rsa-sha2-512` 与 `rsa-sha2-256`：

- 禁止 `ssh-rsa`/SHA-1 默认、回退、签名和验签路径。
- 删除 RSA 密钥生成所需的易受攻击 RustCrypto `rsa` 后端。
- `russh` 与 `RustCrypto-SSH` 均固定到公开、可复现的完整提交。

固定来源：

- `JayhaShf/RustCrypto-SSH@5455fc09fc2508d09a59bd60879bb05111a9d013`
- `JayhaShf/russh@705e92dfb551e458d376016ecc83f43be71b0aa1`

专项验证：RustCrypto-SSH RSA 测试 `36/36`，russh 测试 `82/82`，JShell 测试 `197/197`。

## 4. 最终验证

| 命令或检查 | 结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `cargo test --locked --quiet` | 通过，197 passed，0 failed |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | 通过 |
| `cargo build --locked --release` | 通过 |
| `cargo audit` | 通过；扫描 1023 个依赖，无公告输出 |
| `cargo tree -i rsa --locked` | 未找到 `rsa` 包，符合预期 |
| `git diff --check` | 通过 |

Windows Release 产物：

- 路径：`target/release/jshell.exe`
- 大小：79,115,264 字节
- 版本：`0.1.0-beta.2`
- `ProductName` / `FileDescription` / `InternalName`：`JShell`
- SHA-256：`BD476B94CE06F79AB09E7FB71E708E63299FB4507DFC5B9F81A8E3ABA43F28C7`

测试和构建中的唯一提示是 MSVC 链接器创建导入库的标准输出，不是代码告警或失败。

## 5. 人工验收边界

自动化已覆盖状态机、布局、权限格式、保存决策、连接代次和构建结果。以下项目仍属于发布前的真实环境冒烟，不应描述成自动化已经验证：

- 真实 Linux SSH/SFTP 服务器上的权限、删除失败重试和静默断线恢复。
- 真实远端文件的分屏编辑、独立窗口重挂和冲突保存。
- 多窗口状态下 Windows 任务栏标题的实时切换。

这些是运行环境验收项，不是当前已知代码残留。未经新的实测证据，不移动旧标签、不创建 beta.2 标签，也不发布 GitHub Release。
