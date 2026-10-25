use std::process::ExitCode;

fn version() -> String {
    format!("joey {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "version") => {
            println!("{}", version());
            ExitCode::SUCCESS
        }
        Some("egress-proxy") => joey::egress::main(&args[1..]),
        _ => {
            eprintln!("usage: joey --version | joey egress-proxy --config <file>");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn the_version_names_the_binary_and_the_crate() {
        assert_eq!(version(), format!("joey {}", env!("CARGO_PKG_VERSION")));
    }
}
