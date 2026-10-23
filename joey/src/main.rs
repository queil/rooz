use std::process::ExitCode;

fn version() -> String {
    format!("joey {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "version") => {
            println!("{}", version());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: joey --version");
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
