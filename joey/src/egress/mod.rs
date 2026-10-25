// `joey egress-proxy`: the only way out for the containers that peer with it. A CONNECT is
// decided on its host and port before anything is resolved or dialled; an allowed host is
// reached as it is, with the TLS server name checked against the host asked for; an inject host
// is intercepted so header rules can add what the client must not hold, such as a token.
pub mod config;
pub mod hosts;
pub mod mint;
pub mod proxy;
pub mod sni;

pub use config::Config;
pub use proxy::{Proxy, serve};

use std::process::ExitCode;

pub fn main(args: &[String]) -> ExitCode {
    let path = match args {
        [flag, path] if flag == "--config" => path,
        _ => {
            eprintln!("usage: joey egress-proxy --config <file>");
            return ExitCode::from(2);
        }
    };
    // a config that cannot be read stops the proxy: it never runs open
    let proxy = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path, e))
        .and_then(|body| Config::from_json(&body))
        .and_then(Proxy::new);
    let proxy = match proxy {
        Ok(p) => p,
        Err(e) => {
            eprintln!("joey egress-proxy: {}", e);
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("joey egress-proxy: {}", e);
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&proxy.config().listen).await?;
        println!("joey egress-proxy listening on {}", listener.local_addr()?);
        serve(listener, proxy).await
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("joey egress-proxy: {}", e);
            ExitCode::FAILURE
        }
    }
}
