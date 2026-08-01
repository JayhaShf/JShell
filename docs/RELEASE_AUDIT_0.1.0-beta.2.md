# JShell v0.1.0-beta.2 发布候选审计与验收记录

- 记录日期：2026-08-01
- 候选版本：`0.1.0-beta.2`
- 当前状态：本地门禁通过，待分批提交、推送、托管 CI、标签和 GitHub Release
- 审查范围：相对当前 `origin/main` 的本地功能、安全、品牌、文档与发布收尾修改

## 1. 发布状态

本文件记录 `v0.1.0-beta.2` 发布候选，不是已发布声明。旧标签 `v0.1.0-beta.1` 保持不动；只有本地提交推送后 GitHub Actions 全部通过，才创建并推送 `v0.1.0-beta.2` 标签。

## 2. 候选功能状态

| 范围 | 状态 | 证据或后续要求 |
|---|---|---|
| P0 主机密钥校验、配置凭据保护和原子保存 | 已实现 | 全量测试、Clippy、审计通过 |
| SSH 直连、继承代理、自定义 SOCKS5/SOCKS5H/HTTP/HTTPS | 已实现 | 路由、TLS、失败关闭测试通过 |
| Linux SFTP 权限、上传、删除、双击目录和连接监视 | 已实现 | 自动测试通过；发布前建议真实 Linux 主机复核 |
| 远程编辑器、独立窗口、终端/编辑器分屏、自动换行和保存状态 | 已实现 | 布局与状态测试通过；发布前建议真实远端文件复核 |
| 嵌套分屏拖动 | 已修复 | 使用每个 split 的实际 Bounds；专项测试通过 |
| Cloudflare R2 手动配置同步 | 已实现 | SigV4、HEAD、条件 PUT、限长下载和冲突测试通过 |
| 同步载荷 v1/v2、严格 DTO 和快捷键校验 | 已实现 | 21 条 payload 专项测试通过 |
| 配置写入串行化和凭据补偿 | 已实现 | 并发版本屏障、失败回滚测试通过 |
| 下载预览与冲突失效 | 已实现 | 完整连接快照、当前 remember 选择和表单变化测试通过 |
| 动态偏好变化计数 | 已实现 | 相同配置显示 0，按类别准确计数 |
| B1 标签色带、动态窗口标题、终端 `Ctrl+滚轮` 缩放 | 已实现 | 自动测试及既有桌面验收通过 |
| JShell 品牌、三平台资源和 README | 已更新 | README 使用中文默认入口和真实 Release 截图 |
| 关于页和原项目致谢 | 已更新 | 版本来自 Cargo，中英文键集合测试通过 |

## 3. 安全与依赖

- 锁定依赖图中不存在 RustCrypto `rsa` crate，`RUSTSEC-2023-0071` 不再适用。
- JShell 仍支持 RSA 用户密钥，但只允许 `rsa-sha2-512` 与 `rsa-sha2-256`，不启用 `ssh-rsa`/SHA-1 回退。
- SSH 依赖固定到公开、可复现的提交：
  - `JayhaShf/RustCrypto-SSH@5455fc09fc2508d09a59bd60879bb05111a9d013`
  - `JayhaShf/russh@705e92dfb551e458d376016ecc83f43be71b0aa1`
- 2026-08-01 的 RustSec 官方公告库 HEAD 为 `685d32fd681b540aa64019820639613c5a4fd922`；1177 条公告扫描 1024 个依赖，无 deny-warning 结果。
- CI 使用固定 `cargo-audit@0.22.0` 检查当前公告库；Release 使用固定公告库提交和 `--no-fetch`，保证历史标签重跑可复现。

## 4. 最终自动化复验

- [x] `cargo fmt --all -- --check`
- [x] `cargo test --locked --quiet`：316 passed，0 failed
- [x] `cargo check --locked --all-targets`
- [x] `cargo clippy --locked --all-targets --all-features -- -D warnings`
- [x] `cargo audit --deny warnings --file Cargo.lock --db <clean-db> --no-fetch`
- [x] `cargo tree -i rsa --locked`：未匹配到 `rsa`
- [x] GitHub Actions YAML、最小权限、checkout 凭据和 40 位 Action SHA 静态检查
- [x] `git diff --check`
- [x] `cargo build --locked --release`
- [x] 隔离 HOME/USERPROFILE 启动最终 Windows EXE
- [x] README 截图来自 Release 和隔离配置环境，不含真实主机、账号或路径
- [ ] 推送后的 GitHub CI 与 Release workflow 托管验证

最终本地产物：

```text
D:\Git\ashell\target\release\jshell.exe
79,746,560 bytes
SHA-256 F8BF82325F1F74D90F02AEB27BF981FA563F50CE1118A4CFBA5FB5DFD4A2FA11
```

## 5. GUI 与真实环境验收

- [x] WebDAV/S3/R2 provider 可切换
- [x] 960×700 窄窗口可滚动到全部同步操作按钮
- [x] 空凭据测试显示同步参数错误，不写远端对象
- [x] 下载、冲突、凭据失败和本地写盘失败通过自动故障注入验证为“不改配置或补偿回滚”
- [x] 隔离启动前后用户配置 SHA-256 保持 `B6D937EA3591FCBE1C52465BCD8808F94491F7C9E7BDFBB4572FCA6A69872461`
- [x] 关于页、版本、中文/英文致谢和项目链接已验收
- [ ] 使用非生产 Linux SSH/SFTP 主机复核权限、上传、删除和静默断线恢复
- [ ] 使用非生产远端文件复核编辑冲突、结果未知和断线保留内容
- [ ] 如提供测试 R2 账户，执行一次真实对象上传、下载预览和条件覆盖；当前发布不依赖该账户

## 6. 工作流安全

- 顶层默认权限为 `contents: read`。
- 仅 `publish` job 使用 `contents: write`。
- 所有 checkout 均禁用凭据持久化。
- 18 个 Action 调用均固定到完整提交 SHA。
- Release 审计数据库固定为 `685d32fd681b540aa64019820639613c5a4fd922`。
- `publish` 仅在 tag ref 上运行，并依赖已通过 quality gate 的多平台 build。

## 7. 发布顺序

1. 按“分屏与配置一致性 / R2 与载荷安全 / 关于页与文档资源 / 工作流与审计”分批提交。
2. 再次核对远端 `main` 无分叉后推送。
3. 等待托管 CI 全部通过。
4. 创建并推送 `v0.1.0-beta.2` 标签。
5. 等待 Release workflow 完成，核对 Windows、Linux 和 macOS 产物。
6. 发布后记录 GitHub 产物哈希；本地哈希不替代托管发布产物哈希。

## 8. 发布判定

当前判定：**本地发布候选通过，可以提交并推送；托管 CI 通过前不创建标签。**

项目总体审计结论见 [项目审计报告](../AUDIT_REPORT.md)。
