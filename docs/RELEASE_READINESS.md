# JShell v1.0 发布就绪清单（修改意见 / 优化建议 / 上线前事项）

> 本文档是 v1.0 正式版发布的检查依据，与 `docs/TEST_PLAN.md`（六类测试计划）、
> `AUDIT_REPORT.md`（历史审计）配套使用。状态列在发布前逐项刷新。

---

## 1. 代码审查修改意见（按优先级）

| 优先级 | 编号 | 发现 | 处置与状态 |
|---|---|---|---|
| P0 | R-001 | tar/tar.gz 目录归档解包疑似路径穿越（`sftp/mod.rs`） | **已核实为 crate 层防护**：`tar` 0.4.46 `unpack_in` 跳过 `..` 条目并经 canonicalize 拒绝符号链接逃逸；zip 分支有 `enclosed_name()`。本次以 4 项回归测试锁定行为 + 函数注释说明，防止未来重构破坏。✅ 已闭环 |
| P0 | R-002 | CI `cargo audit --deny warnings` 因依赖告警红灯，无逐项风险记录 | ✅ `.cargo/audit.toml` 精确记录并豁免 9 条 `unmaintained` 与 1 条 `unsound`；`RUSTSEC-2024-0429` 作为 Linux GTK3/glib 残余风险保留，见 AUDIT_REPORT 第 7 节；本地 `--deny warnings` 通过 |
| P1 | R-003 | i18n 缺键：`confirm` 在 zh-CN 存在、en.yml 缺失（en 为 fallback，英文界面会显示键名） | ✅ en.yml 补 `confirm: "Confirm"`，新增 locale 键对齐测试防复发 |
| P1 | R-004 | TOFU 主机密钥首次自动接受、无用户确认弹窗（设计文档已记录该取舍） | ⏳ 保留现状；v1.0 安全评审项（TEST_PLAN S-010），后续版本评估"提示确认"设置项 |
| P1 | R-005 | v1 配置加密硬件 UUID 检测失败的兜底常量公开可知（`session/config.rs`） | ⏳ 仅限旧版 v1 迁移路径；建议后续 fail-closed 或告警，本次不动（避免迁移兼容风险） |
| P2 | R-006 | 日志目录未收紧权限（可能含主机名/用户名） | ✅ `app/startup.rs` 创建后 chmod 0700（Unix） |
| P2 | R-007 | `scripts/finalize-fork-hide.sh` 未跟踪且过期（rev/sed 目标均已失效） | ✅ 已删除 |
| P2 | R-008 | `ssh_config.rs` 不支持 tab 分隔、端口 0 被接受 | ✅ 支持 tab 分隔；端口 0 按默认 22 处理；补 12 项边界测试 |

## 2. 优化建议（后续版本，按收益排序）

| 编号 | 建议 | 说明 |
|---|---|---|
| O-001 | macOS Intel 实机测试 | CI 已在 Windows/Linux/macOS aarch64 原生运行测试，macOS x86_64 受 runner 架构限制只做 `cargo test --no-run`，发布前宜补 Intel 实机运行 |
| O-002 | llvm-cov 覆盖率接入 quality job + 下限阈值 | 防回归；当前无任何覆盖率度量 |
| O-003 | 高亮引擎性能 | `terminal/highlight.rs` 每帧正则：建议行级结果缓存/预编译；`render_snapshot` 评估 scrollback 上限；先按 TEST_PLAN 第 2 节实测再优化 |
| O-004 | cargo-deny 许可证合规 | GPL-3.0 项目依赖多，补 license 检查 |
| O-005 | fork 依赖安全跟踪 | gpui/russh/alacritty 均为 fork，cargo audit 覆盖不到 fork 内部改动；建立上游补丁跟踪流程 |
| O-006 | 发布工程 | Windows 代码签名（SmartScreen 提示）、macOS 公证（现 ad-hoc）；发布审计清单模板化 |
| O-007 | 托盘后端迁移评估 | GTK3 链停维护（已豁免）；长期评估 ksni 替代 |
| O-008 | i18n 硬编码文案扫描 | 键对齐已自动化；补硬编码字符串检测 |
| O-009 | CONTRIBUTING.md | 沉淀测试/审计规范与验证命令 |

## 3. v1.0 上线前清单

### 3.1 阻塞级（发布前必须完成）

| 编号 | 事项 | 状态 |
|---|---|---|
| B-001 | CI 红灯解除（audit 豁免）+ 全量门禁通过 | ✅ 已解除；PR #1 CI 质量门禁 + 四平台构建全绿，发布流程质量门禁复跑通过 |
| B-002 | 安全测试落地（解包回归、shell_quote、i18n 对齐、ssh_config 边界） | ✅ 已落地（新增 30 项测试） |
| B-003 | beta.2 审计 4 项未验证关闭 | ✅ ① CI/Release 托管验证：v1.0.0 tag 发布全流程通过；⏳ ② 非生产 Linux SSH/SFTP 主机复核权限/上传/删除/静默断线恢复；③ 远程编辑冲突/结果未知/断线保留内容复核；④ 真实 R2 上传/下载（需测试账户，不阻塞若提供不了） |
| B-004 | Windows `WM_ENDSESSION`（系统关机时托盘应用不保存状态） | ⏳ 需文档化降级方案（发布说明中明示）；代码支持列 backlog |
| B-005 | 易用性关键路径实测：Linux 托盘左键无响应、GNOME Wayland 最小化被忽略 | ⏳ 发布说明中明示；UI 补偿（"隐藏"代替"最小化"）列 backlog |

### 3.2 重要级

| 编号 | 事项 | 状态 |
|---|---|---|
| I-001 | Windows/macOS 代码签名与公证（分发体验） | ⏳ 需证书/Apple ID，列发布工程项 |
| I-002 | i18n 双语键完整性 | ✅ 键对齐自动化 + en.yml 补键 |
| I-003 | 发布审计清单模板化 + CONTRIBUTING | ⏳ O-006/O-009 合并执行 |
| I-004 | 性能基线首测（P-001 启动、P-002 大输出帧率、P-004 大文件） | ⏳ 发布前回填 TEST_PLAN 基线表 |

### 3.3 可延后（backlog）

TOFU 确认弹窗（R-004）、v1 兜底密钥加固（R-005）、WM_ENDSESSION、Wayland 最小化补偿、ksni 迁移、覆盖率、criterion bench、update-check 机制、平台矩阵测试。

---

## 4. 发布流程（v1.0 最终推送步骤）

1. 版本升级：`Cargo.toml` → `1.0.0`，同步 `Cargo.lock`；`packaging/arch/PKGBUILD` `pkgver=1.0.0`、`pkgrel=1` ✅
2. README.md / README.en.md 版本引用 → v1.0.0（历史审计文档保留原版本号）✅
3. 撰写 `.github/release-notes/v1.0.0.zh-CN.md`（发布必需）✅
4. `scripts/verify.sh` 全量门禁 + Linux keyring/fallback 双场景冒烟 ✅（2026-08-19：Debug/release 各 442/442；两种冒烟各 11/11）
5. 提交 → push `main` → CI 绿 ✅（PR #1：质量门禁 + 四平台构建全绿）
6. tag `v1.0.0` 并 push（无 `-`，`release.yml` 自动发布为正式版 latest；tag 与 Cargo 版本一致性由 workflow 校验）✅（2026-08-13 发布成功）
7. 发布后核查：Release 页非 prerelease、四平台产物齐全、PKGBUILD 版本一致 ✅（v1.0.0 为 latest 正式版；windows/linux/macos×2 四个产物齐全；PKGBUILD pkgver=1.0.0）
