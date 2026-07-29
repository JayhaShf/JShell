# SSH 直连与统一代理实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 SSH 会话提供“跟随全局 / 直连 / 使用代理”策略，支持 SOCKS5、HTTP 和经过证书及主机名校验的 HTTPS CONNECT，并保持旧配置兼容。

**Architecture:** 继续使用 `Session.proxy_type` 作为唯一持久化入口；`resolve_proxy` 先处理显式 `direct`，再处理会话代理、环境代理和全局代理。HTTP 与 HTTPS 共用泛型 CONNECT 解析器，HTTPS 在 CONNECT 前通过 Tokio Rustls 建立 TLS；生产配置加载系统根证书，测试配置显式注入只信任测试 CA 的 `RootCertStore`。

**Tech Stack:** Rust 2024、Tokio、Rustls 0.23.40 Ring、tokio-rustls 0.26.4 Ring、rustls-native-certs 0.8.4、rcgen 0.13、GPUI Component、rust-i18n。

---

## 文件职责

- `Cargo.toml`、`Cargo.lock`：固定 Ring TLS 依赖和测试证书依赖。
- `src/session/config.rs`：代理解析、路由、系统根证书、TLS、CONNECT 和连接层测试。
- `src/session/mod.rs`：会话代理策略映射、保存校验、SSH Config 选择流程及停用代理时清除凭据。
- `src/app/dialogs.rs`：三种认证共用的两层代理控件，以及全局 HTTPS 选项。
- `src/backend/ssh.rs`：生成不含凭据的 DIRECT、SOCKS5、HTTP、HTTPS 连接状态。
- `locales/zh-CN.yml`、`locales/en.yml`：新增策略、协议和校验提示翻译。
- `AUDIT_REPORT.md`：按实际命令输出更新测试数、锁定依赖数和审计基线，不修改发布文件。

### Task 1：固定 TLS 依赖并扩展代理路由

**Files:**
- Modify: `Cargo.toml:10-80`
- Modify: `Cargo.lock`
- Modify: `src/session/config.rs:1-8, 38-69, 1262-1420`
- Test: `src/session/config.rs:1857-2000`

- [ ] **Step 1：固定依赖版本和密码学提供者**

在 `[dependencies]` 增加：

```toml
rustls = { version = "=0.23.40", default-features = false, features = ["ring", "std", "tls12"] }
rustls-native-certs = "=0.8.4"
tokio-rustls = { version = "=0.26.4", default-features = false, features = ["ring", "tls12"] }
```

在 `[dev-dependencies]` 增加：

```toml
rcgen = "0.13"
```

运行：

```powershell
cargo check --quiet
cargo tree -e features -i rustls@0.23.40
```

预期：依赖解析成功；树中存在 `rustls feature "ring"`，不存在 `aws-lc-rs` 或 `aws_lc_rs`。

- [ ] **Step 2：先写路由兼容测试**

在 `src/session/config.rs` 的现有 `tests` 模块增加以下测试：

```rust
#[test]
fn explicit_direct_bypasses_environment_global_and_persistence_gate() {
    let mut session = Session::password(
        "example.test".to_string(),
        22,
        "root".to_string(),
        "secret".to_string(),
    );
    session.proxy_type = "direct".to_string();
    let mut config = direct_proxy_config();
    config.read_env_proxy = true;
    config.env_proxy = Err("broken environment proxy".to_string());
    config.use_global_proxy = true;
    config.global_proxy = ProxyEndpoint {
        proxy_type: "unknown".to_string(),
        host: String::new(),
        port: Some(0),
        user: String::new(),
        password: String::new(),
    };
    config.allow_direct = false;

    assert_eq!(resolve_proxy(&session, &config).unwrap(), ProxyRoute::Direct);
}

#[test]
fn none_and_empty_session_proxy_inherit_environment_then_global() {
    for proxy_type in ["", "none"] {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = proxy_type.to_string();
        let mut config = direct_proxy_config();
        config.read_env_proxy = true;
        config.env_proxy = Ok(Some(proxy_endpoint("https", "env.proxy", 443)));
        config.use_global_proxy = true;
        config.global_proxy = proxy_endpoint("http", "global.proxy", 8080);

        assert!(matches!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy {
                kind: ProxyKind::Https,
                ref host,
                port: 443,
                ..
            }) if host == "env.proxy"
        ));

        config.env_proxy = Ok(None);
        assert!(matches!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy {
                kind: ProxyKind::Http,
                ref host,
                port: 8080,
                ..
            }) if host == "global.proxy"
        ));
    }
}

#[test]
fn legacy_explicit_proxy_types_remain_supported() {
    for (proxy_type, expected_kind, port) in [
        ("socks5", ProxyKind::Socks5, 1080),
        ("socks5h", ProxyKind::Socks5, 1080),
        ("http", ProxyKind::Http, 8080),
    ] {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = proxy_type.to_string();
        session.proxy_host = "session.proxy".to_string();
        session.proxy_port = Some(port);
        assert!(matches!(
            resolve_proxy(&session, &direct_proxy_config()).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy { kind, .. }) if kind == expected_kind
        ));
    }
}

#[test]
fn https_environment_proxy_uses_port_443_by_default() {
    let endpoint = parse_env_proxy("HTTPS_PROXY", "https://proxy.example").unwrap();
    assert_eq!(endpoint.proxy_type, "https");
    assert_eq!(endpoint.host, "proxy.example");
    assert_eq!(endpoint.port, Some(443));
}

#[test]
fn global_https_proxy_is_resolved_for_an_inherited_session() {
    let session = Session::password(
        "example.test".to_string(),
        22,
        "root".to_string(),
        "secret".to_string(),
    );
    let mut config = direct_proxy_config();
    config.use_global_proxy = true;
    config.global_proxy = proxy_endpoint("https", "global.proxy", 443);

    assert!(matches!(
        resolve_proxy(&session, &config).unwrap(),
        ProxyRoute::Proxy(ResolvedProxy {
            kind: ProxyKind::Https,
            ref host,
            port: 443,
            ..
        }) if host == "global.proxy"
    ));
}
```

保留现有 `invalid_selected_proxy_does_not_fall_back_to_direct_connection` 和 `unavailable_persistent_configuration_refuses_unconfirmed_direct_connection`，再增加未知类型与零端口断言：

```rust
#[test]
fn unknown_type_and_zero_port_fail_closed() {
    let mut session = Session::password(
        "example.test".to_string(),
        22,
        "root".to_string(),
        "secret".to_string(),
    );
    session.proxy_type = "ftp".to_string();
    session.proxy_host = "proxy.example".to_string();
    session.proxy_port = Some(21);
    assert!(
        resolve_proxy(&session, &direct_proxy_config())
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );

    session.proxy_type = "http".to_string();
    session.proxy_port = Some(0);
    assert!(
        resolve_proxy(&session, &direct_proxy_config())
            .unwrap_err()
            .to_string()
            .contains("missing or invalid")
    );
}
```

- [ ] **Step 3：分别运行单一过滤词测试并确认失败**

Run: `cargo test explicit_direct_bypasses_environment_global_and_persistence_gate -- --nocapture`

Run: `cargo test none_and_empty_session_proxy_inherit_environment_then_global -- --nocapture`

Run: `cargo test legacy_explicit_proxy_types_remain_supported -- --nocapture`

Run: `cargo test https_environment_proxy_uses_port_443_by_default -- --nocapture`

Run: `cargo test global_https_proxy_is_resolved_for_an_inherited_session -- --nocapture`

Expected: 新测试因 `direct`、`ProxyKind::Https` 或 `https://` 尚未支持而失败。

- [ ] **Step 4：实现类型解析和路由顺序**

把 `Session.proxy_type` 注释更新为：

```rust
pub proxy_type: String, // "none", "direct", "socks5", "socks5h", "http", "https"
```

把 `ProxyKind` 及其字符串映射替换为：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyKind {
    Socks5,
    Http,
    Https,
}

impl ProxyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}
```

`parse_env_proxy` 的协议匹配改为：

```rust
let proxy_type = match url.scheme() {
    "socks5" | "socks5h" => url.scheme().to_string(),
    "http" => "http".to_string(),
    "https" => "https".to_string(),
    scheme => {
        return Err(format!(
            "{variable} uses unsupported proxy scheme '{scheme}'"
        ));
    }
};
```

`validate_proxy` 的类型匹配改为：

```rust
let kind = match endpoint.proxy_type.trim().to_ascii_lowercase().as_str() {
    "socks5" | "socks5h" => ProxyKind::Socks5,
    "http" => ProxyKind::Http,
    "https" => ProxyKind::Https,
    proxy_type => bail!("{source} proxy type '{proxy_type}' is unsupported"),
};
```

`resolve_proxy` 开头替换为以下顺序，后续环境、全局和 `allow_direct` 分支保持原样：

```rust
let session_proxy_type = session.proxy_type.trim();
if session_proxy_type.eq_ignore_ascii_case("direct") {
    return Ok(ProxyRoute::Direct);
}
if !session_proxy_type.is_empty() && !session_proxy_type.eq_ignore_ascii_case("none") {
    return validate_proxy(
        ProxyEndpoint {
            proxy_type: session.proxy_type.clone(),
            host: session.proxy_host.clone(),
            port: session.proxy_port,
            user: session.proxy_user.clone(),
            password: session.proxy_password.clone(),
        },
        "session",
    )
    .map(ProxyRoute::Proxy);
}
```

- [ ] **Step 5：运行路由回归**

Run: `cargo test proxy -- --nocapture`

Expected: 路由、旧值、环境 HTTPS、全局 HTTPS和失败关闭测试全部通过。

- [ ] **Step 6：提交该任务**

```powershell
git add Cargo.toml Cargo.lock src/session/config.rs
git commit -m "feat(proxy): add direct and HTTPS proxy routes"
```

### Task 2：提取严格的泛型 HTTP CONNECT

**Files:**
- Modify: `src/session/config.rs:1422-1540`
- Test: `src/session/config.rs` 的 `tests` 模块

- [ ] **Step 1：增加 2xx、16 KiB、不完整响应和预读数据测试**

在测试模块导入 `tokio::io::{AsyncReadExt as _, AsyncWriteExt as _}`，并增加：

```rust
async fn connect_over_duplex(response: Vec<u8>) -> Result<tokio::io::BufStream<tokio::io::DuplexStream>> {
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut request = Vec::new();
        loop {
            request.push(server.read_u8().await.unwrap());
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8(request).unwrap().starts_with(
            "CONNECT target.example:22 HTTP/1.1\r\nHost: target.example:22\r\n"
        ));
        let _ = server.write_all(&response).await;
    });
    establish_http_connect(
        client,
        &ResolvedProxy {
            kind: ProxyKind::Http,
            host: "proxy.example".to_string(),
            port: 8080,
            user: String::new(),
            password: String::new(),
        },
        "target.example",
        22,
    )
    .await
}

#[test]
fn http_connect_validation_accepts_only_real_2xx_statuses() {
    for status in [200, 204, 299] {
        let response = format!("HTTP/1.1 {status} Result\r\n\r\n");
        validate_http_connect_response(response.as_bytes()).unwrap();
    }
    let error =
        validate_http_connect_response(b"HTTP/1.1 300 Redirect\r\n\r\n").unwrap_err();
    assert!(error.to_string().contains("status 300"));
}

#[tokio::test]
async fn http_connect_rejects_headers_larger_than_16_kib() {
    let mut response = b"HTTP/1.1 200 OK\r\nX-Fill: ".to_vec();
    response.extend(vec![b'a'; 16 * 1024]);
    let error = connect_over_duplex(response).await.unwrap_err();
    assert!(error.to_string().contains("exceed 16 KiB"));
}

#[tokio::test]
async fn http_connect_rejects_an_incomplete_response() {
    let error = connect_over_duplex(b"HTTP/1.1 200 OK\r\n".to_vec())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("read HTTP proxy CONNECT response"));
}

#[tokio::test]
async fn http_connect_preserves_bytes_after_response_headers() {
    let mut stream = connect_over_duplex(
        b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n".to_vec(),
    )
    .await
    .unwrap();
    let mut banner = [0_u8; 14];
    stream.read_exact(&mut banner).await.unwrap();
    assert_eq!(&banner, b"SSH-2.0-test\r\n");
}
```

- [ ] **Step 2：分别运行测试并确认泛型函数尚不存在**

Run: `cargo test http_connect_validation_accepts_only_real_2xx_statuses -- --nocapture`

Run: `cargo test http_connect_rejects_headers_larger_than_16_kib -- --nocapture`

Run: `cargo test http_connect_rejects_an_incomplete_response -- --nocapture`

Run: `cargo test http_connect_preserves_bytes_after_response_headers -- --nocapture`

Expected: 异步测试因 `establish_http_connect` 尚未定义而无法编译。

- [ ] **Step 3：实现共享 CONNECT 函数**

把 16 KiB 常量移到模块级，并在 `validate_http_connect_response` 后加入：

```rust
const MAX_HTTP_CONNECT_RESPONSE_BYTES: usize = 16 * 1024;

async fn establish_http_connect<S>(
    stream: S,
    proxy: &ResolvedProxy,
    target_host: &str,
    target_port: u16,
) -> Result<tokio::io::BufStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let mut stream = tokio::io::BufStream::new(stream);
    let authority = format_authority(target_host, target_port);
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !proxy.user.is_empty() {
        let auth = format!("{}:{}", proxy.user, proxy.password);
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(auth)
        ));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .context("write HTTP proxy CONNECT request")?;
    stream
        .flush()
        .await
        .context("flush HTTP proxy CONNECT request")?;

    let mut response = Vec::with_capacity(512);
    loop {
        response.push(
            stream
                .read_u8()
                .await
                .context("read HTTP proxy CONNECT response")?,
        );
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() >= MAX_HTTP_CONNECT_RESPONSE_BYTES {
            bail!("HTTP proxy CONNECT response headers exceed 16 KiB");
        }
    }
    validate_http_connect_response(&response)?;
    Ok(stream)
}
```

`ProxyKind::Http` 分支只负责 TCP 连接并调用共享函数：

```rust
ProxyRoute::Proxy(proxy) if proxy.kind == ProxyKind::Http => {
    let stream = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|error| anyhow::anyhow!("HTTP proxy connection failed: {error}"))?;
    let tunnel = establish_http_connect(stream, &proxy, &target_host, target_port).await?;
    Ok(Box::new(tunnel) as Box<dyn ProxyStream>)
}
```

- [ ] **Step 4：运行 CONNECT 回归**

Run: `cargo test http_connect -- --nocapture`

Expected: 真实 2xx、非 2xx、16 KiB、不完整响应及预读数据测试全部通过。

### Task 3：实现真实 HTTPS TLS 与禁止降级

**Files:**
- Modify: `src/session/config.rs:1-8, 1449-1540`
- Test: `src/session/config.rs` 的 `tests` 模块

- [ ] **Step 1：加入生产根证书和 HTTPS 连接函数**

在 `std::sync` 导入 `Arc`，并加入：

```rust
fn native_https_client_config() -> Result<Arc<rustls::ClientConfig>> {
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        tracing::warn!(%error, "[proxy] failed to load one system root certificate");
    }
    let mut roots = rustls::RootCertStore::empty();
    let (accepted, rejected) = roots.add_parsable_certificates(native.certs);
    if rejected > 0 {
        tracing::warn!(
            rejected,
            "[proxy] ignored invalid system root certificates"
        );
    }
    if accepted == 0 {
        bail!("load HTTPS proxy system root certificates failed: no usable roots");
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

async fn connect_https_proxy_with_config(
    proxy: &ResolvedProxy,
    target_host: &str,
    target_port: u16,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Box<dyn ProxyStream>> {
    let tcp = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .context("connect TCP socket to HTTPS proxy")?;
    let server_name = rustls::pki_types::ServerName::try_from(proxy.host.clone())
        .context("HTTPS proxy host is not a valid TLS server name")?;
    let tls = tokio_rustls::TlsConnector::from(tls_config)
        .connect(server_name, tcp)
        .await
        .context("perform HTTPS proxy TLS handshake and certificate validation")?;
    let tunnel = establish_http_connect(tls, proxy, target_host, target_port)
        .await
        .context("establish CONNECT tunnel through HTTPS proxy")?;
    Ok(Box::new(tunnel))
}
```

把当前 `connect_proxy` 主体移入以下私有入口，公开入口传 `None`；测试只通过 `Some(test_config)` 注入测试根，不读取机器证书：

```rust
async fn connect_proxy_with_tls_config(
    session: &Session,
    config: &ConnectionProxyConfig,
    test_tls_config: Option<Arc<rustls::ClientConfig>>,
) -> Result<Box<dyn ProxyStream>> {
    let route = resolve_proxy(session, config)?;
    let target_host = session.host.clone();
    let target_port = session.port;
    let connect_fut = async move {
        match route {
            ProxyRoute::Direct => {
                let stream =
                    tokio::net::TcpStream::connect((target_host.as_str(), target_port))
                        .await
                        .with_context(|| {
                            format!("direct connection to {target_host}:{target_port} failed")
                        })?;
                Ok(Box::new(stream) as Box<dyn ProxyStream>)
            }
            ProxyRoute::Proxy(proxy) if proxy.kind == ProxyKind::Socks5 => {
                let proxy_address = (proxy.host.as_str(), proxy.port);
                if proxy.user.is_empty() {
                    let stream = tokio_socks::tcp::Socks5Stream::connect(
                        proxy_address,
                        (target_host.as_str(), target_port),
                    )
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("SOCKS5 proxy connection failed: {error}")
                    })?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                } else {
                    let stream = tokio_socks::tcp::Socks5Stream::connect_with_password(
                        proxy_address,
                        (target_host.as_str(), target_port),
                        &proxy.user,
                        &proxy.password,
                    )
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("SOCKS5 proxy connection failed: {error}")
                    })?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                }
            }
            ProxyRoute::Proxy(proxy) if proxy.kind == ProxyKind::Http => {
                let stream =
                    tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("HTTP proxy connection failed: {error}")
                        })?;
                let tunnel =
                    establish_http_connect(stream, &proxy, &target_host, target_port).await?;
                Ok(Box::new(tunnel) as Box<dyn ProxyStream>)
            }
            ProxyRoute::Proxy(proxy) => {
                let tls_config = match test_tls_config {
                    Some(config) => config,
                    None => native_https_client_config()?,
                };
                connect_https_proxy_with_config(
                    &proxy,
                    &target_host,
                    target_port,
                    tls_config,
                )
                .await
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(16), connect_fut)
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out after 16 seconds"))?
}

pub async fn connect_proxy(
    session: &Session,
    config: &ConnectionProxyConfig,
) -> Result<Box<dyn ProxyStream>> {
    connect_proxy_with_tls_config(session, config, None).await
}
```

该匹配只有一个已解析路由；SOCKS5、HTTP、TLS 或 CONNECT 返回错误后立即结束，不重新调用 `resolve_proxy`，也不进入 `Direct` 分支。

- [ ] **Step 2：增加只信任测试 CA 的 TLS 辅助**

在测试模块增加以下辅助。证书链由测试 CA 签发，客户端根库只注入该 CA：

```rust
fn test_tls_configs(
    names: &[&str],
) -> (
    Arc<rustls::ServerConfig>,
    Arc<rustls::ClientConfig>,
) {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose,
    };
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec!["ashell-test-ca".to_string()]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf_params = CertificateParams::new(
        names.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
    )
    .unwrap();
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .unwrap();

    let server = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf_cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        )
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca_cert.der().clone()).unwrap();
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (Arc::new(server), Arc::new(client))
}

async fn spawn_tls_proxy(
    server_config: Arc<rustls::ServerConfig>,
    response: Vec<u8>,
) -> (
    u16,
    tokio::task::JoinHandle<Result<()>>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await?;
        let mut tls = tokio_rustls::TlsAcceptor::from(server_config)
            .accept(tcp)
            .await
            .context("accept test TLS connection")?;
        let mut request = Vec::new();
        loop {
            request.push(tls.read_u8().await.context("read test CONNECT request")?);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
            if request.len() >= 16 * 1024 {
                bail!("test CONNECT request exceeded 16 KiB");
            }
        }
        tls.write_all(&response).await?;
        tls.flush().await?;
        Ok(())
    });
    (port, task)
}

fn resolved_https_proxy(port: u16) -> ResolvedProxy {
    ResolvedProxy {
        kind: ProxyKind::Https,
        host: "localhost".to_string(),
        port,
        user: String::new(),
        password: String::new(),
    }
}
```

- [ ] **Step 3：增加真实 TLS、证书、主机名、状态码和预读测试**

```rust
#[tokio::test]
async fn trusted_https_proxy_completes_tls_connect_and_preserves_tunnel_bytes() {
    use tokio::io::AsyncReadExt as _;

    let (server_config, client_config) = test_tls_configs(&["localhost"]);
    let (port, server) = spawn_tls_proxy(
        server_config,
        b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n".to_vec(),
    )
    .await;
    let mut stream = connect_https_proxy_with_config(
        &resolved_https_proxy(port),
        "target.example",
        22,
        client_config,
    )
    .await
    .unwrap();
    let mut banner = [0_u8; 14];
    stream.read_exact(&mut banner).await.unwrap();
    assert_eq!(&banner, b"SSH-2.0-test\r\n");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn https_proxy_rejects_an_untrusted_certificate() {
    let (server_config, _) = test_tls_configs(&["localhost"]);
    let (port, server) =
        spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
    let client = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    );
    let error = connect_https_proxy_with_config(
        &resolved_https_proxy(port),
        "target.example",
        22,
        client,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("certificate validation"));
    let _ = server.await;
}

#[tokio::test]
async fn https_proxy_rejects_a_mismatched_server_name() {
    let (server_config, client_config) = test_tls_configs(&["wrong.example"]);
    let (port, server) =
        spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
    let error = connect_https_proxy_with_config(
        &resolved_https_proxy(port),
        "target.example",
        22,
        client_config,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("certificate validation"));
    let _ = server.await;
}

#[tokio::test]
async fn https_proxy_surfaces_non_success_connect_status() {
    let (server_config, client_config) = test_tls_configs(&["localhost"]);
    let (port, server) = spawn_tls_proxy(
        server_config,
        b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
    )
    .await;
    let error = connect_https_proxy_with_config(
        &resolved_https_proxy(port),
        "target.example",
        22,
        client_config,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("status 407"));
    server.await.unwrap().unwrap();
}
```

再加入以下两项完整测试。它们都通过公开路由的测试入口发起连接，并在目标端监听 200 ms，证明 TLS 或 CONNECT 失败后没有转为直连：

```rust
#[tokio::test]
async fn https_tls_failure_does_not_fall_back_to_direct() {
    let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let target_port = target_listener.local_addr().unwrap().port();
    let (server_config, _) = test_tls_configs(&["localhost"]);
    let (proxy_port, proxy_server) =
        spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
    let untrusted_client = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    );
    let mut session = Session::password(
        "127.0.0.1".to_string(),
        target_port,
        "root".to_string(),
        "secret".to_string(),
    );
    session.proxy_type = "https".to_string();
    session.proxy_host = "localhost".to_string();
    session.proxy_port = Some(proxy_port);

    let error = connect_proxy_with_tls_config(
        &session,
        &direct_proxy_config(),
        Some(untrusted_client),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("certificate validation"));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            target_listener.accept(),
        )
        .await
        .is_err()
    );
    let _ = proxy_server.await;
}

#[tokio::test]
async fn https_connect_failure_does_not_fall_back_to_direct() {
    let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let target_port = target_listener.local_addr().unwrap().port();
    let (server_config, client_config) = test_tls_configs(&["localhost"]);
    let (proxy_port, proxy_server) = spawn_tls_proxy(
        server_config,
        b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
    )
    .await;
    let mut session = Session::password(
        "127.0.0.1".to_string(),
        target_port,
        "root".to_string(),
        "secret".to_string(),
    );
    session.proxy_type = "https".to_string();
    session.proxy_host = "localhost".to_string();
    session.proxy_port = Some(proxy_port);

    let error = connect_proxy_with_tls_config(
        &session,
        &direct_proxy_config(),
        Some(client_config),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("status 407"));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            target_listener.accept(),
        )
        .await
        .is_err()
    );
    proxy_server.await.unwrap().unwrap();
}
```

- [ ] **Step 4：逐项运行 HTTPS 测试**

Run: `cargo test trusted_https_proxy_completes_tls_connect_and_preserves_tunnel_bytes -- --nocapture`

Run: `cargo test https_proxy_rejects_an_untrusted_certificate -- --nocapture`

Run: `cargo test https_proxy_rejects_a_mismatched_server_name -- --nocapture`

Run: `cargo test https_proxy_surfaces_non_success_connect_status -- --nocapture`

Run: `cargo test https_tls_failure_does_not_fall_back_to_direct -- --nocapture`

Run: `cargo test https_connect_failure_does_not_fall_back_to_direct -- --nocapture`

Expected: 全部通过；测试不读取 Windows、macOS 或 Linux 机器根证书。

- [ ] **Step 5：提交 TLS 隧道**

```powershell
git add Cargo.toml Cargo.lock src/session/config.rs
git commit -m "feat(proxy): support verified HTTPS CONNECT proxies"
```

### Task 4：会话策略映射、三认证流程和凭据清理

**Files:**
- Modify: `src/session/mod.rs:16, 119-241, 503-564`
- Test: `src/session/mod.rs:1911-1969`

- [ ] **Step 1：增加策略与清理测试**

```rust
#[test]
fn proxy_policy_maps_inherit_direct_and_legacy_custom_values() {
    assert_eq!(session_proxy_policy(""), SessionProxyPolicy::Inherit);
    assert_eq!(session_proxy_policy("none"), SessionProxyPolicy::Inherit);
    assert_eq!(session_proxy_policy("direct"), SessionProxyPolicy::Direct);
    assert_eq!(session_proxy_policy("socks5"), SessionProxyPolicy::Custom);
    assert_eq!(session_proxy_policy("socks5h"), SessionProxyPolicy::Custom);
    assert_eq!(session_proxy_policy("http"), SessionProxyPolicy::Custom);
    assert_eq!(session_proxy_policy("https"), SessionProxyPolicy::Custom);
}

#[test]
fn proxy_policy_selection_preserves_a_known_protocol_or_defaults_to_socks5() {
    assert_eq!(
        proxy_type_for_policy("none", SessionProxyPolicy::Custom),
        "socks5"
    );
    assert_eq!(
        proxy_type_for_policy("https", SessionProxyPolicy::Custom),
        "https"
    );
    assert_eq!(
        proxy_type_for_policy("socks5h", SessionProxyPolicy::Custom),
        "socks5h"
    );
    assert_eq!(
        proxy_type_for_policy("http", SessionProxyPolicy::Direct),
        "direct"
    );
}

#[test]
fn inactive_proxy_policy_clears_all_saved_endpoint_fields() {
    for proxy_type in ["none", "direct"] {
        let mut session = test_session(proxy_type);
        apply_session_proxy(
            &mut session,
            proxy_type,
            "proxy.example".to_string(),
            Some(443),
            "proxy-user".to_string(),
            "proxy-secret".to_string(),
        );
        assert_eq!(session.proxy_type, proxy_type);
        assert!(session.proxy_host.is_empty());
        assert_eq!(session.proxy_port, None);
        assert!(session.proxy_user.is_empty());
        assert!(session.proxy_password.is_empty());
    }
}
```

- [ ] **Step 2：运行单一过滤词测试并确认失败**

Run: `cargo test proxy_policy_maps_inherit_direct_and_legacy_custom_values -- --nocapture`

Run: `cargo test proxy_policy_selection_preserves_a_known_protocol_or_defaults_to_socks5 -- --nocapture`

Run: `cargo test inactive_proxy_policy_clears_all_saved_endpoint_fields -- --nocapture`

Expected: 辅助类型和函数尚未定义，编译失败。

- [ ] **Step 3：实现策略和保存函数**

在 `impl Ashell` 前增加：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionProxyPolicy {
    Inherit,
    Direct,
    Custom,
}

pub(crate) fn session_proxy_policy(proxy_type: &str) -> SessionProxyPolicy {
    match proxy_type.trim().to_ascii_lowercase().as_str() {
        "" | "none" => SessionProxyPolicy::Inherit,
        "direct" => SessionProxyPolicy::Direct,
        _ => SessionProxyPolicy::Custom,
    }
}

pub(crate) fn proxy_type_for_policy(
    current: &str,
    policy: SessionProxyPolicy,
) -> String {
    match policy {
        SessionProxyPolicy::Inherit => "none".to_string(),
        SessionProxyPolicy::Direct => "direct".to_string(),
        SessionProxyPolicy::Custom => {
            match current.trim().to_ascii_lowercase().as_str() {
                "socks5" | "socks5h" | "http" | "https" => {
                    current.trim().to_ascii_lowercase()
                }
                _ => "socks5".to_string(),
            }
        }
    }
}

fn apply_session_proxy(
    session: &mut Session,
    proxy_type: &str,
    proxy_host: String,
    proxy_port: Option<u16>,
    proxy_user: String,
    proxy_password: String,
) {
    session.proxy_type = if proxy_type.trim().is_empty() {
        "none".to_string()
    } else {
        proxy_type.trim().to_ascii_lowercase()
    };
    match session_proxy_policy(&session.proxy_type) {
        SessionProxyPolicy::Inherit | SessionProxyPolicy::Direct => {
            session.proxy_host.clear();
            session.proxy_port = None;
            session.proxy_user.clear();
            session.proxy_password.clear();
        }
        SessionProxyPolicy::Custom => {
            session.proxy_host = proxy_host;
            session.proxy_port = proxy_port;
            session.proxy_user = proxy_user;
            session.proxy_password = proxy_password;
        }
    }
}
```

`connect_ssh` 的代理校验条件改为：

```rust
if session_proxy_policy(&self.ssh_proxy_type) == SessionProxyPolicy::Custom {
    let proxy_host = self.proxy_host_input.read(cx).value().trim().to_string();
    let proxy_port = self
        .proxy_port_input
        .read(cx)
        .value()
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0);
    if proxy_host.is_empty() || proxy_port.is_none() {
        self.status = t!("proxy_host_port_required").into();
        cx.notify();
        return;
    }
}
```

把直接写五个代理字段的代码替换为：

```rust
let proxy_host = self.proxy_host_input.read(cx).value().trim().to_string();
let proxy_port = self
    .proxy_port_input
    .read(cx)
    .value()
    .trim()
    .parse::<u16>()
    .ok();
let proxy_user = self.proxy_user_input.read(cx).value().trim().to_string();
let proxy_password = self.proxy_password_input.read(cx).value().to_string();
apply_session_proxy(
    &mut session,
    &self.ssh_proxy_type,
    proxy_host,
    proxy_port,
    proxy_user,
    proxy_password,
);
```

增加策略 setter，并保留现有协议 setter：

```rust
pub(crate) fn set_ssh_proxy_policy(
    &mut self,
    policy: SessionProxyPolicy,
    cx: &mut Context<Self>,
) {
    self.ssh_proxy_type = proxy_type_for_policy(&self.ssh_proxy_type, policy);
    cx.notify();
}

pub(crate) fn set_ssh_proxy_type(
    &mut self,
    proxy_type: String,
    cx: &mut Context<Self>,
) {
    self.ssh_proxy_type = proxy_type;
    cx.notify();
}
```

- [ ] **Step 4：让 SSH Config 选择后等待用户确认**

`select_ssh_config_entry` 填完表单后删除立即调用 `connect_ssh` 的语句，改为：

```rust
Self::set_input_value(&self.password_input, String::new(), window, cx);
Self::set_input_value(&self.key_inline_input, String::new(), window, cx);
Self::set_input_value(&self.passphrase_input, String::new(), window, cx);
cx.notify();
```

这样密码、密钥和 SSH Config 三种认证都能在连接前选择同一代理策略。

- [ ] **Step 5：运行会话测试**

Run: `cargo test proxy_policy -- --nocapture`

Run: `cargo test inactive_proxy_policy_clears_all_saved_endpoint_fields -- --nocapture`

Expected: `none`、空值、`direct` 和旧协议映射正确；保存 `none` 或 `direct` 后四个端点及凭据字段为空。

### Task 5：统一会话 UI、全局 HTTPS 和双语文本

**Files:**
- Modify: `src/app/dialogs.rs:1-22, 411-764, 2646-2712`
- Modify: `locales/zh-CN.yml:121-126, 274-284`
- Modify: `locales/en.yml:120-125, 273-283`

- [ ] **Step 1：导入策略类型并计算两层状态**

`src/app/dialogs.rs` 的 crate 导入改为：

```rust
use crate::{
    Ashell,
    session::{
        SessionProxyPolicy, session_proxy_policy,
        config::AuthMethod,
    },
    system::format_bytes,
};
```

在 SSH 对话框内容闭包中，用以下变量替换原 `show_proxy_fields`：

```rust
let proxy_type = view.read(cx).ssh_proxy_type.clone();
let proxy_policy = session_proxy_policy(&proxy_type);
let show_proxy_fields = proxy_policy == SessionProxyPolicy::Custom;
let socks5_selected = matches!(
    proxy_type.trim().to_ascii_lowercase().as_str(),
    "socks5" | "socks5h"
);
let ssh_config_selected = view.read(cx).ssh_config_selected.is_some();
let can_submit = !is_config || is_editing || ssh_config_selected;
```

- [ ] **Step 2：把代理区域移到三个认证分支之后**

删除包裹代理区域的 `.when(!is_config, ...)`，使区域位于 `.when(is_config, ...)` 之后且仍在 `.when(is_ssh, ...)` 内。第一层固定三个按钮：

```rust
.child(
    div()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .child(t!("proxy").to_string()),
)
.child(
    h_flex()
        .gap_2()
        .child(
            Button::new("proxy-inherit")
                .label(t!("proxy_none").to_string())
                .when(proxy_policy == SessionProxyPolicy::Inherit, |button| {
                    button.primary()
                })
                .on_click(window.listener_for(&view, |this, _, _, cx| {
                    this.set_ssh_proxy_policy(SessionProxyPolicy::Inherit, cx)
                })),
        )
        .child(
            Button::new("proxy-direct")
                .label(t!("proxy_direct").to_string())
                .when(proxy_policy == SessionProxyPolicy::Direct, |button| {
                    button.primary()
                })
                .on_click(window.listener_for(&view, |this, _, _, cx| {
                    this.set_ssh_proxy_policy(SessionProxyPolicy::Direct, cx)
                })),
        )
        .child(
            Button::new("proxy-custom")
                .label(t!("proxy_use").to_string())
                .when(proxy_policy == SessionProxyPolicy::Custom, |button| {
                    button.primary()
                })
                .on_click(window.listener_for(&view, |this, _, _, cx| {
                    this.set_ssh_proxy_policy(SessionProxyPolicy::Custom, cx)
                })),
        ),
)
```

只在 `show_proxy_fields` 为真时追加协议按钮及现有输入框：

```rust
.when(show_proxy_fields, |this| {
    this.child(
        div()
            .text_sm()
            .child(t!("proxy_protocol").to_string()),
    )
    .child(
        h_flex()
            .gap_2()
            .child(
                Button::new("proxy-type-socks5")
                    .label("SOCKS5")
                    .when(socks5_selected, |button| button.primary())
                    .on_click(window.listener_for(&view, |this, _, _, cx| {
                        this.set_ssh_proxy_type("socks5".to_string(), cx)
                    })),
            )
            .child(
                Button::new("proxy-type-http")
                    .label("HTTP")
                    .when(proxy_type == "http", |button| button.primary())
                    .on_click(window.listener_for(&view, |this, _, _, cx| {
                        this.set_ssh_proxy_type("http".to_string(), cx)
                    })),
            )
            .child(
                Button::new("proxy-type-https")
                    .label("HTTPS")
                    .when(proxy_type == "https", |button| button.primary())
                    .on_click(window.listener_for(&view, |this, _, _, cx| {
                        this.set_ssh_proxy_type("https".to_string(), cx)
                    })),
            ),
    )
    .child(
        h_flex()
            .gap_2()
            .child(Input::new(&proxy_host_input).flex_1())
            .child(Input::new(&proxy_port_input).w(px(96.))),
    )
    .child(
        h_flex()
            .gap_2()
            .child(Input::new(&proxy_user_input).flex_1())
            .child(Input::new(&proxy_password_input).flex_1()),
    )
})
```

确认按钮条件从 `!is_config` 改为 `can_submit`。新建 SSH Config 会话必须先选择条目；编辑已有 Config 会话可以直接保存。

- [ ] **Step 3：增加全局 HTTPS 并显示保存错误**

在全局 SOCKS5、HTTP 按钮后增加：

```rust
.child(
    Button::new("global-proxy-type-https")
        .small()
        .label("HTTPS")
        .when(proxy_type == "https", |button| button.primary())
        .on_click(window.listener_for(&view, |this, _, _, cx| {
            this.global_proxy_type = "https".to_string();
            cx.notify();
        })),
)
```

全局保存校验替换为：

```rust
if host.is_empty() || port.is_none() {
    this.status = t!("proxy_host_port_required").into();
    cx.notify();
    return;
}
```

该返回发生在读取密码之后但不修改输入实体，因此校验失败时保留已输入密码。

- [ ] **Step 4：增加中英文键**

`locales/zh-CN.yml` 增加：

```yaml
proxy_direct: "直连"
proxy_use: "使用代理"
proxy_protocol: "代理协议"
proxy_host_port_required: "代理地址和端口为必填项"
```

`locales/en.yml` 增加：

```yaml
proxy_direct: "Direct"
proxy_use: "Use Proxy"
proxy_protocol: "Proxy Protocol"
proxy_host_port_required: "Proxy host and port are required"
```

- [ ] **Step 5：编译并检查翻译键**

Run: `cargo fmt --check`

Run: `cargo check --all-targets`

Run: `rg -n "proxy_direct|proxy_use|proxy_protocol|proxy_host_port_required" locales/zh-CN.yml locales/en.yml`

Expected: 编译通过；每个语言文件包含四个键。

- [ ] **Step 6：提交 UI 与翻译**

```powershell
git add src/app/dialogs.rs src/session/mod.rs locales/zh-CN.yml locales/en.yml
git commit -m "feat(proxy): unify SSH proxy controls"
```

### Task 6：连接状态、日志脱敏和审计基线

**Files:**
- Modify: `src/backend/ssh.rs:285-319, 793-853`
- Modify: `AUDIT_REPORT.md`

- [ ] **Step 1：先写四种状态不泄密测试**

在 `connect_and_authenticate` 前增加测试目标函数声明后，在现有测试模块增加：

```rust
#[test]
fn proxy_connection_status_reports_route_without_credentials() {
    let direct = proxy_connection_status("host:22", None);
    let socks = proxy_connection_status("host:22", Some(("socks5", "proxy", 1080)));
    let http = proxy_connection_status("host:22", Some(("http", "proxy", 8080)));
    let https = proxy_connection_status("host:22", Some(("https", "proxy", 443)));

    assert!(direct.contains("DIRECT"));
    assert!(socks.contains("SOCKS5"));
    assert!(http.contains("HTTP"));
    assert!(https.contains("HTTPS"));
    for status in [&direct, &socks, &http, &https] {
        assert!(!status.contains("secret"));
        assert!(!status.contains("Basic"));
        assert!(!status.contains("Proxy-Authorization"));
    }
}
```

- [ ] **Step 2：运行测试并实现状态函数**

Run: `cargo test proxy_connection_status_reports_route_without_credentials -- --nocapture`

Expected: 函数尚未定义，编译失败。

实现：

```rust
fn proxy_connection_status(
    target: &str,
    proxy: Option<(&str, &str, u16)>,
) -> String {
    match proxy {
        Some((proxy_type, host, port)) => format!(
            "connecting to {target} via {} proxy {host}:{port}",
            proxy_type.to_ascii_uppercase()
        ),
        None => format!("connecting to {target} via DIRECT"),
    }
}
```

`connect_and_authenticate` 中替换当前状态拼接：

```rust
let proxy = crate::session::config::active_proxy(session, proxy_config)?;
let status_text = proxy_connection_status(
    &addr,
    proxy
        .as_ref()
        .map(|(proxy_type, host, port)| (proxy_type.as_str(), host.as_str(), *port)),
);
```

不得把 `proxy.password`、完整 CONNECT 请求、Basic 值或 `Session.proxy_password` 传给 `tracing!` 或状态函数。允许继续记录协议、主机、端口和用户名。

- [ ] **Step 3：执行完整验证**

Run: `cargo fmt --check`

Run: `cargo test --quiet`

Run: `cargo check --all-targets`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Run: `cargo build`

Expected: 五条命令退出 0。

- [ ] **Step 4：执行审计并更新实际基线**

Run: `cargo audit`

Expected: 命令因已知 `RUSTSEC-2023-0071` 返回 1；漏洞仍为 `russh 0.60.3 -> rsa 0.10.0-rc.16` 的一个无修复版本公告，维护性警告仍为 7 条，且 TLS 新依赖不引入新公告。

按实际输出更新 `AUDIT_REPORT.md`：

1. 把环境代理仅支持 SOCKS5/HTTP 的旧描述改为同时支持 HTTPS。
2. 记录 `direct` 显式绕过环境和全局代理。
3. 记录 HTTPS 使用系统根、证书和主机名校验，失败不降级。
4. 把锁定依赖数和测试总数改为本次命令实际值。
5. 保留 `RUSTSEC-2023-0071` 和 7 条维护性警告；若实际输出不同，逐项记录新增或消失的公告及依赖链，不增加忽略配置。

- [ ] **Step 5：Debug 冒烟**

Run: `target\debug\jshell.exe`

依次验证：

1. 密码、密钥和 SSH Config 三种认证均显示“跟随全局 / 直连 / 使用代理”。
2. `none` 和空值会话继承环境代理；无环境代理时继承已启用的全局 HTTPS。
3. `direct` 在环境代理和全局代理故意设为无效值时仍直连目标。
4. 旧 `socks5`、`socks5h`、`http` 会话重开后显示正确协议。
5. 保存 `none` 或 `direct` 后，重新编辑时主机、端口、用户名和密码为空。
6. HTTPS 不受信任证书、错误主机名、TLS 失败和 CONNECT 407 均终止连接，不尝试 HTTP 或 DIRECT。
7. 状态分别显示 DIRECT、SOCKS5、HTTP、HTTPS，启动日志和连接日志中没有代理密码或 Basic 认证值。

- [ ] **Step 6：提交状态和审计记录**

```powershell
git add src/backend/ssh.rs AUDIT_REPORT.md
git commit -m "chore(proxy): report active route and refresh audit"
```

## 实施后自检

| 规格要求 | 对应任务 |
|---|---|
| `direct` 绕过环境、全局及非持久化限制 | Task 1 |
| `none`、空值继承环境及全局 | Task 1 |
| 旧 `socks5`、`socks5h`、`http` | Task 1、Task 5 |
| 环境和全局 HTTPS | Task 1、Task 5 |
| 真实 TLS、测试根注入、证书及主机名校验 | Task 3 |
| TLS 与 CONNECT 失败不降级 | Task 3、Task 6 |
| CONNECT 真实 2xx、16 KiB、不完整响应、预读数据 | Task 2、Task 3 |
| 保存 `none`、`direct` 清除凭据 | Task 4 |
| 密码、密钥、SSH Config 三认证统一 UI | Task 4、Task 5 |
| DIRECT、SOCKS5、HTTP、HTTPS 状态且不泄密 | Task 6 |
| 格式、测试、Clippy、构建和审计基线 | Task 6 |

计划执行时不得修改 `.github`、Release 工作流、发布脚本或安装包配置。
