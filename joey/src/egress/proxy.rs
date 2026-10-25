use super::{
    config::{Config, InjectHost},
    hosts,
    mint::Mint,
    sni::{self, Hello},
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode,
    body::Incoming,
    header::{HOST, HeaderName, HeaderValue},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const HEAD_LIMIT: usize = 8 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

type Body = BoxBody<Bytes, hyper::Error>;

#[derive(Clone)]
pub struct Proxy {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    mint: Option<Mint>,
    upstream: Arc<rustls::ClientConfig>,
}

impl Proxy {
    pub fn new(config: Config) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mint = match &config.ca {
            Some(ca) => {
                let read =
                    |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{}: {}", p, e));
                Some(Mint::load(&read(&ca.cert)?, &read(&ca.key)?)?)
            }
            None => None,
        };
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for path in &config.upstream_roots {
            let certs = CertificateDer::pem_file_iter(path)
                .map_err(|e| format!("upstream_roots {}: {}", path, e))?;
            for cert in certs {
                let cert = cert.map_err(|e| format!("upstream_roots {}: {}", path, e))?;
                roots
                    .add(cert)
                    .map_err(|e| format!("upstream_roots {}: {}", path, e))?;
            }
        }
        let mut upstream = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        upstream.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                mint,
                upstream: Arc::new(upstream),
            }),
        })
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    async fn handle(&self, mut client: TcpStream) -> io::Result<()> {
        let Ok(head) = timeout(CLIENT_TIMEOUT, read_head(&mut client)).await else {
            return Ok(());
        };
        let (head, rest) = head?;
        let Some((method, target)) = request_line(&head) else {
            return respond(&mut client, 400, "bad request").await;
        };
        if method != "CONNECT" {
            log(&format!(
                "deny {} {}: only CONNECT is proxied",
                method, target
            ));
            return respond(&mut client, 403, "only CONNECT is proxied").await;
        }
        let Some((host, port)) = split_target(&target) else {
            return respond(&mut client, 400, "bad CONNECT target").await;
        };
        let host = hosts::normalize(&host);
        // decided before anything is resolved or dialled: a host outside the lists is never
        // even looked up
        if !self.inner.config.allows(&host, port) {
            log(&format!("deny {}:{}: not allowed", host, port));
            return respond(&mut client, 403, "not allowed").await;
        }
        client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await?;
        let client = Prefixed::new(rest, client);
        match self.inner.config.inject_host(&host).cloned() {
            Some(rules) => self.intercept(client, &host, port, rules).await,
            None => self.pass(client, &host, port).await,
        }
    }

    // The client's TLS goes through untouched, to the host the CONNECT named and only if the
    // ClientHello asks for that same host: a permitted CONNECT cannot carry another server name.
    async fn pass(&self, mut client: Prefixed<TcpStream>, host: &str, port: u16) -> io::Result<()> {
        let Ok(hello) = timeout(CLIENT_TIMEOUT, read_hello(&mut client)).await else {
            return Ok(());
        };
        let (bytes, name) = match hello? {
            Ok(h) => h,
            Err(why) => {
                log(&format!("deny {}:{}: {}", host, port, why));
                return Ok(());
            }
        };
        match name.map(|n| hosts::normalize(&n)) {
            Some(n) if n == host => {}
            Some(n) => {
                log(&format!(
                    "deny {}:{}: server name '{}' is another host",
                    host, port, n
                ));
                return Ok(());
            }
            None => {
                log(&format!("deny {}:{}: no server name", host, port));
                return Ok(());
            }
        }
        let mut upstream = match timeout(DIAL_TIMEOUT, TcpStream::connect((host, port))).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                log(&format!("fail {}:{}: {}", host, port, e));
                return Ok(());
            }
            Err(_) => {
                log(&format!("fail {}:{}: connect timed out", host, port));
                return Ok(());
            }
        };
        log(&format!("pass {}:{}", host, port));
        upstream.write_all(&bytes).await?;
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        Ok(())
    }

    // The proxy is the server the client talks to, with a certificate from the CA the client
    // trusts, so it can apply header rules before passing each request on over verified TLS.
    async fn intercept(
        &self,
        client: Prefixed<TcpStream>,
        host: &str,
        port: u16,
        rules: InjectHost,
    ) -> io::Result<()> {
        let mint = self
            .inner
            .mint
            .as_ref()
            .ok_or_else(|| io::Error::other("inject without a ca"))?;
        let key = mint.for_host(host).map_err(io::Error::other)?;
        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(key)));
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let tls = match TlsAcceptor::from(Arc::new(tls)).accept(client).await {
            Ok(s) => s,
            Err(e) => {
                log(&format!("fail {}:{}: client TLS: {}", host, port, e));
                return Ok(());
            }
        };
        let upstream = self.inner.upstream.clone();
        let host = host.to_string();
        let service = service_fn(move |req| {
            forward(req, host.clone(), port, rules.clone(), upstream.clone())
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tls), service)
            .await;
        Ok(())
    }
}

pub async fn serve(listener: TcpListener, proxy: Proxy) -> io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let proxy = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy.handle(stream).await {
                log(&format!("fail {}: {}", peer, e));
            }
        });
    }
}

fn log(line: &str) {
    println!("{}", line);
}

#[derive(Debug)]
struct Fixed(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for Fixed {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

async fn forward(
    mut req: Request<Incoming>,
    host: String,
    port: u16,
    inject: InjectHost,
    upstream: Arc<rustls::ClientConfig>,
) -> Result<Response<Body>, hyper::Error> {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let rule = inject
        .rules
        .iter()
        .position(|r| r.matches(&method, &path, |n| req.headers().contains_key(n)));
    match rule {
        Some(i) => {
            let r = &inject.rules[i];
            for name in &r.remove {
                req.headers_mut().remove(name.as_str());
            }
            for (name, value) in &r.set {
                // both checked when the config was read
                if let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    req.headers_mut().insert(n, v);
                }
            }
            log(&format!(
                "inject {} {} {} (rule {})",
                host,
                method,
                path,
                i + 1
            ));
        }
        None => log(&format!("pass {} {} {}", host, method, path)),
    }
    // The request goes to the host the tunnel was opened for, and says so: a Host header naming
    // another site behind the same address would carry whatever was just injected to it.
    let authority = if port == 443 {
        host.clone()
    } else {
        format!("{}:{}", host, port)
    };
    match HeaderValue::from_str(&authority) {
        Ok(v) => {
            req.headers_mut().insert(HOST, v);
        }
        Err(_) => return Ok(text(StatusCode::BAD_REQUEST, "bad host")),
    }
    match send(req, &host, port, upstream).await {
        Ok(resp) => Ok(resp.map(|b| b.boxed())),
        Err(e) => {
            log(&format!("fail {}:{}: upstream: {}", host, port, e));
            Ok(text(StatusCode::BAD_GATEWAY, "upstream failed"))
        }
    }
}

async fn send(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    upstream: Arc<rustls::ClientConfig>,
) -> Result<Response<Incoming>, Box<dyn std::error::Error + Send + Sync>> {
    let tcp = timeout(DIAL_TIMEOUT, TcpStream::connect((host, port))).await??;
    let name = ServerName::try_from(host.to_string())?;
    let tls = TlsConnector::from(upstream).connect(name, tcp).await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender.send_request(req).await?)
}

fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut resp = Response::new(
        Full::new(Bytes::from_static(body.as_bytes()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *resp.status_mut() = status;
    resp
}

async fn respond(client: &mut TcpStream, code: u16, body: &str) -> io::Result<()> {
    let reason = match code {
        400 => "Bad Request",
        403 => "Forbidden",
        _ => "Error",
    };
    let msg = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        reason,
        body.len(),
        body
    );
    client.write_all(msg.as_bytes()).await?;
    client.shutdown().await
}

// The request head and whatever the client sent after it.
async fn read_head(client: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), rest));
        }
        if buf.len() > HEAD_LIMIT {
            return Err(io::Error::other("request head too large"));
        }
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "closed before a request",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn request_line(head: &str) -> Option<(String, String)> {
    let mut parts = head.lines().next()?.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    parts
        .next()?
        .starts_with("HTTP/1.")
        .then_some((method, target))
}

// `host:port`, or `[v6]:port`; a port is required
pub fn split_target(target: &str) -> Option<(String, u16)> {
    let (host, port) = match target.strip_prefix('[') {
        Some(v6) => {
            let (h, p) = v6.split_once("]:")?;
            (h, p)
        }
        None => target.rsplit_once(':')?,
    };
    if host.is_empty() || host.contains(['/', '@', ' ']) {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

// The ClientHello's bytes, all of them to pass on, and the server name in it.
async fn read_hello(
    client: &mut Prefixed<TcpStream>,
) -> io::Result<Result<(Vec<u8>, Option<String>), &'static str>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match sni::parse(&buf) {
            Hello::Complete(name) => return Ok(Ok((buf, name))),
            Hello::Invalid => return Ok(Err("not a TLS ClientHello")),
            Hello::Incomplete => {}
        }
        if buf.len() > sni::MAX_HELLO {
            return Ok(Err("ClientHello too large"));
        }
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Ok(Err("closed before a ClientHello"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

// A stream that first yields bytes already read from it.
pub struct Prefixed<S> {
    prefix: Vec<u8>,
    at: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            at: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.at < me.prefix.len() {
            let n = buf.remaining().min(me.prefix.len() - me.at);
            buf.put_slice(&me.prefix[me.at..me.at + n]);
            me.at += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connect_target_needs_a_host_and_a_port() {
        assert_eq!(
            split_target("github.com:443"),
            Some(("github.com".into(), 443))
        );
        assert_eq!(split_target("[::1]:8443"), Some(("::1".into(), 8443)));
        for bad in [
            "github.com",
            ":443",
            "github.com:x",
            "a/b:443",
            "u@h:443",
            "[::1]",
        ] {
            assert_eq!(split_target(bad), None, "{}", bad);
        }
    }

    #[test]
    fn a_request_line_is_method_target_and_http1() {
        assert_eq!(
            request_line("CONNECT a:443 HTTP/1.1\r\n\r\n"),
            Some(("CONNECT".into(), "a:443".into()))
        );
        assert_eq!(request_line("CONNECT a:443\r\n\r\n"), None);
        assert_eq!(request_line("garbage\r\n\r\n"), None);
    }
}
