// The proxy end to end: a TLS server standing in for the internet, the proxy in front of it,
// and a client going through the proxy, all on 127.0.0.1.
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use joey::egress::{Config, Proxy, serve};
use pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, pem::PemObject};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

struct Ca {
    cert_pem: String,
    key_pem: String,
    issuer: Issuer<'static, KeyPair>,
}

fn ca(name: &str) -> Ca {
    let mut p = CertificateParams::default();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    p.distinguished_name.push(DnType::CommonName, name);
    let key = KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    let key_pem = key.serialize_pem();
    Ca {
        cert_pem: cert.pem(),
        key_pem,
        issuer: Issuer::new(p, key),
    }
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "joey-egress-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// An HTTPS server for `localhost` that answers with the path and the request headers it got,
// and counts the connections it accepted.
async fn upstream(ca: &Ca) -> (u16, Arc<AtomicUsize>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut p = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name.push(DnType::CommonName, "localhost");
    let key = KeyPair::generate().unwrap();
    let cert = p.signed_by(&key, &ca.issuer).unwrap();
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(s).await else {
                    return;
                };
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let mut body = format!("path={}\n", req.uri().path());
                    for (n, v) in req.headers() {
                        body.push_str(&format!("{}: {}\n", n, v.to_str().unwrap_or("?")));
                    }
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(body))))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), svc)
                    .await;
            });
        }
    });
    (port, accepted)
}

async fn proxy(config: serde_json::Value) -> String {
    let cfg = Config::from_json(&config.to_string()).unwrap();
    let p = Proxy::new(cfg).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(serve(listener, p));
    addr
}

// The proxy's answer to a CONNECT, and the stream to go on with
async fn connect(proxy: &str, target: &str) -> (String, TcpStream) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n", target, target).as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).await.unwrap() == 0 {
            break;
        }
        head.push(b[0]);
    }
    let status = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    (status, s)
}

// A GET over TLS through an open tunnel, as `sni`, trusting `root`
async fn get(
    s: TcpStream,
    sni: &str,
    root: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(root.as_bytes()).unwrap())
        .unwrap();
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tls = TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from(sni.to_string()).unwrap(), s)
        .await
        .map_err(|e| e.to_string())?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(conn);
    let mut req = Request::get(path).header("host", sni);
    for (n, v) in headers {
        req = req.header(*n, *v);
    }
    let resp = sender
        .send_request(req.body(Empty::<Bytes>::new()).unwrap())
        .await
        .map_err(|e| e.to_string())?;
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&body.to_bytes()).into_owned())
}

#[tokio::test]
async fn a_host_outside_the_lists_is_refused_before_anything_is_dialled() {
    let addr = proxy(serde_json::json!({"allow": ["localhost"]})).await;
    let (status, _) = connect(&addr, "example.invalid:443").await;
    assert!(status.contains("403"), "{}", status);
}

#[tokio::test]
async fn a_port_outside_the_list_is_refused() {
    let up = ca("upstream");
    let (port, accepted) = upstream(&up).await;
    let addr = proxy(serde_json::json!({"allow": ["localhost"]})).await;
    let (status, _) = connect(&addr, &format!("localhost:{}", port)).await;
    assert!(status.contains("403"), "{}", status);
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_plain_request_is_refused() {
    let addr = proxy(serde_json::json!({"allow": ["localhost"]})).await;
    let mut s = TcpStream::connect(&addr).await.unwrap();
    s.write_all(b"GET http://localhost/ HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 403"), "{}", out);
}

#[tokio::test]
async fn an_allowed_host_is_reached_end_to_end_with_its_own_tls() {
    let up = ca("upstream");
    let (port, _) = upstream(&up).await;
    let addr = proxy(serde_json::json!({"allow": ["localhost"], "ports": [port]})).await;
    let (status, s) = connect(&addr, &format!("localhost:{}", port)).await;
    assert!(status.contains("200"), "{}", status);
    // the client trusts the upstream's CA alone: the proxy did not step into the TLS session
    let body = get(s, "localhost", &up.cert_pem, "/x", &[]).await.unwrap();
    assert!(body.starts_with("path=/x\n"), "{}", body);
}

#[tokio::test]
async fn a_server_name_other_than_the_connect_host_goes_nowhere() {
    let up = ca("upstream");
    let (port, accepted) = upstream(&up).await;
    let addr = proxy(serde_json::json!({"allow": ["localhost"], "ports": [port]})).await;
    let (status, s) = connect(&addr, &format!("localhost:{}", port)).await;
    assert!(status.contains("200"), "{}", status);
    assert!(
        get(s, "elsewhere.test", &up.cert_pem, "/", &[])
            .await
            .is_err()
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "the tunnel reached the upstream"
    );
}

#[tokio::test]
async fn an_inject_host_gets_its_headers_only_where_a_rule_matches() {
    let up = ca("upstream");
    let mitm = ca("proxy");
    let (port, _) = upstream(&up).await;
    let d = dir();
    std::fs::write(d.join("ca.pem"), &mitm.cert_pem).unwrap();
    std::fs::write(d.join("ca.key"), &mitm.key_pem).unwrap();
    std::fs::write(d.join("up.pem"), &up.cert_pem).unwrap();
    let addr = proxy(serde_json::json!({
        "ports": [port],
        "ca": {"cert": d.join("ca.pem"), "key": d.join("ca.key")},
        "upstream_roots": [d.join("up.pem")],
        "inject": [{"host": "localhost", "rules": [{
            "methods": ["GET"], "paths": ["/v1/*"], "when_header": "x-marker",
            "set": {"authorization": "Bearer t0ken"}, "remove": ["x-marker"]
        }]}]
    }))
    .await;
    let through = |path: &'static str, headers: &'static [(&'static str, &'static str)]| {
        let addr = addr.clone();
        let root = mitm.cert_pem.clone();
        async move {
            let (status, s) = connect(&addr, &format!("localhost:{}", port)).await;
            assert!(status.contains("200"), "{}", status);
            // the client trusts the proxy's CA: this session is the proxy's
            get(s, "localhost", &root, path, headers).await.unwrap()
        }
    };

    let marked = through("/v1/messages", &[("x-marker", "1")]).await;
    assert!(marked.contains("authorization: Bearer t0ken"), "{}", marked);
    assert!(
        !marked.contains("x-marker"),
        "the marker reached the upstream: {}",
        marked
    );

    let other_path = through("/other", &[("x-marker", "1")]).await;
    assert!(!other_path.contains("authorization"), "{}", other_path);
    let unmarked = through("/v1/messages", &[]).await;
    assert!(!unmarked.contains("authorization"), "{}", unmarked);

    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test]
async fn a_host_header_naming_another_site_is_replaced() {
    let up = ca("upstream");
    let mitm = ca("proxy");
    let (port, _) = upstream(&up).await;
    let d = dir();
    std::fs::write(d.join("ca.pem"), &mitm.cert_pem).unwrap();
    std::fs::write(d.join("ca.key"), &mitm.key_pem).unwrap();
    std::fs::write(d.join("up.pem"), &up.cert_pem).unwrap();
    let addr = proxy(serde_json::json!({
        "ports": [port],
        "ca": {"cert": d.join("ca.pem"), "key": d.join("ca.key")},
        "upstream_roots": [d.join("up.pem")],
        "inject": [{"host": "localhost", "rules": [{"set": {"authorization": "Bearer t0ken"}}]}]
    }))
    .await;
    let (_, s) = connect(&addr, &format!("localhost:{}", port)).await;
    // the request's own Host header is set by `get` to the SNI; send another one on top
    let body = get(
        s,
        "localhost",
        &mitm.cert_pem,
        "/",
        &[("host", "evil.test")],
    )
    .await
    .unwrap();
    assert!(!body.contains("evil.test"), "{}", body);
    assert!(
        body.contains(&format!("host: localhost:{}", port)),
        "{}",
        body
    );
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test]
async fn a_ca_that_cannot_be_read_stops_the_proxy() {
    let cfg = Config::from_json(
        r#"{"ca": {"cert": "/nonexistent/ca.pem", "key": "/nonexistent/ca.key"},
            "inject": [{"host": "api.example.com"}]}"#,
    )
    .unwrap();
    assert!(Proxy::new(cfg).is_err());
}
