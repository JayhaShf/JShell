# JShell 代理链路与审计基线报告

审计日期：2026-07-29
审计范围：`D:\Git\ashell`
范围约束：本轮仅修改 `src/backend/ssh.rs` 与 `AUDIT_REPORT.md`，未改 `.github`、发布流程、脚本或安装包配置，本轮未执行 `push`

## 1. 本轮结论

本轮完成了 SSH 代理链路状态文本与审计基线刷新：

1. SSH 连接状态现在明确显示实际路由：
   - 无代理时显示 `DIRECT`
   - SOCKS5 代理时显示 `SOCKS5`
   - HTTP 代理时显示 `HTTP`
   - HTTPS 代理时显示 `HTTPS`
2. 连接状态文本仅包含协议、主机、端口，不包含代理密码、`Basic` 值、`Proxy-Authorization` 头或完整 CONNECT 请求。
3. `connect_and_authenticate` 使用 `active_proxy(...)` 的实际解析结果生成状态文本，状态展示与真实连接路线一致。
4. 本轮未发现新的高危问题；`cargo audit` 仍有 1 条已知漏洞和 7 条维护性告警，退出码为 1，原因符合预期。

## 2. 代码变更结论

### 2.1 SSH 状态文本

`src/backend/ssh.rs` 新增：

- `proxy_connection_status(target, proxy)` 统一生成连接状态文本。

实际行为：

- `proxy_connection_status("host:22", None)` 返回包含 `DIRECT`
- `proxy_connection_status("host:22", Some(("socks5", ...)))` 返回包含 `SOCKS5`
- `proxy_connection_status("host:22", Some(("http", ...)))` 返回包含 `HTTP`
- `proxy_connection_status("host:22", Some(("https", ...)))` 返回包含 `HTTPS`

### 2.2 脱敏边界

本轮核对 `src/backend/ssh.rs` 后确认：

1. 状态函数只接收 `proxy_type`、`host`、`port`。
2. `connect_and_authenticate` 不把 `proxy.password`、`Session.proxy_password` 传给状态函数。
3. 本文件未新增对 `Basic`、`Proxy-Authorization` 或完整 CONNECT 请求的日志输出。
4. 允许暴露的信息仍限于协议、主机、端口和用户名。

## 3. 代理与 TLS 审计基线

根据当前代码与本轮复核结果：

1. 环境代理支持 `SOCKS5`、`HTTP`、`HTTPS`。
2. 会话显式选择 `direct` 时，会绕过环境代理与全局代理，并显式走 `DIRECT`。
3. HTTPS 代理通过系统根证书建立 TLS，执行证书链与主机名校验。
4. HTTPS 代理握手失败或 CONNECT 失败时，不会降级为直连。
5. 现有测试继续覆盖：
   - HTTPS 代理可信证书建立隧道
   - 不可信证书拒绝
   - 主机名不匹配拒绝
   - CONNECT 非成功状态拒绝
   - TLS / CONNECT 失败不回退直连
   - 错误消息不泄露 `Basic` 和 `Proxy-Authorization`

## 4. TDD 证据

### RED

先新增测试 `proxy_connection_status_reports_route_without_credentials` 后执行：

```text
cargo test --locked --quiet proxy_connection_status_reports_route_without_credentials
```

得到预期失败，原因是生产函数尚不存在：

```text
cannot find function `proxy_connection_status` in module `super`
```

报错位置为 `src/backend/ssh.rs:856` 与 `src/backend/ssh.rs:868`。

### GREEN

实现 `proxy_connection_status(...)` 并在 `connect_and_authenticate(...)` 中接入后，再执行同一测试：

```text
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 149 filtered out
```

## 5. 本轮实际验证结果

以下结果均来自本轮在 `D:\Git\ashell` 的实际执行：

| 命令 | 实际结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `cargo test --locked --quiet` | 通过，150 passed，0 failed，0 ignored，0 measured，0 filtered out |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | 通过 |
| `cargo build` | 通过 |
| `cargo audit` | 退出码 1，实际为 1 条漏洞 + 7 条维护性告警 |
| `git diff --check` | 通过 |

补充说明：

1. `cargo check` 与 `cargo clippy` 均提示 `proc-macro-error2 v2.0.1` 存在 future-incompatibility 提示，但未导致失败。
2. `cargo build` 与单测链接阶段出现 Windows 链接器标准提示，未导致构建失败。

## 6. `cargo audit` 实际基线

本轮 `cargo audit` 输出显示：

- 锁定依赖总数：`1032`
- 已确认漏洞：`1`
- 维护性告警：`7`
- 退出码：`1`

### 6.1 已知漏洞

1. `RUSTSEC-2023-0071`
   - Crate：`rsa 0.10.0-rc.16`
   - 标题：`Marvin Attack: potential key recovery through timing sidechannels`
   - 严重级别：`5.9 (medium)`
   - 状态：`No fixed upgrade is available!`
   - 结论：本轮保持记录，不新增 ignore 规则

### 6.2 维护性告警

本轮实际告警如下：

1. `RUSTSEC-2025-0052`：`async-std 1.13.2` 已停止维护
2. `RUSTSEC-2024-0384`：`instant 0.1.13` 已停止维护
3. `RUSTSEC-2024-0436`：`paste 1.0.15` 已停止维护
4. `RUSTSEC-2026-0173`：`proc-macro-error2 2.0.1` 已停止维护
5. `RUSTSEC-2025-0134`：`rustls-pemfile 2.2.0` 已停止维护
6. `RUSTSEC-2026-0206`：`rustybuzz 0.20.1` 已停止维护
7. `RUSTSEC-2026-0192`：`ttf-parser 0.25.1` 已停止维护

与此前报告相比，基线仍为“1 条漏洞 + 7 条维护性告警”，但本轮按实际输出精确更新了公告编号与条目。

## 7. 手动 Debug 冒烟

本轮未阻塞在手动 Debug 窗口验证。

原因与限制：

1. 本任务目标是状态文本、脱敏和审计基线，不依赖 GUI 交互才能确认。
2. 若启动 Debug 窗口，自动验证其显示内容的可靠性不足，不能稳定证明状态栏文案。
3. 因已有单元测试与全量构建验证，本轮未额外启动长期运行实例。

## 8. 事实状态

1. 当前工作仅形成本地改动与本地提交，本轮未 `push`。
2. 审计报告已与本轮实测一致，不再保留“已推送”或“已同步远端”表述。
3. 本轮仅应存在以下改动文件：
   - `src/backend/ssh.rs`
   - `AUDIT_REPORT.md`
