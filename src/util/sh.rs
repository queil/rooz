/// Wraps a value in POSIX single quotes so a shell treats it as exactly one
/// literal argument, whatever it contains. The only character that needs
/// special handling is `'` itself: close the quote, emit an escaped quote,
/// reopen.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::quote;
    use std::process::Command;

    fn echo(arg: &str) -> String {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", quote(arg)))
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn plain_value_round_trips() {
        assert_eq!(
            quote("https://github.com/queil/rooz"),
            "'https://github.com/queil/rooz'"
        );
    }

    #[test]
    fn single_quote_is_escaped() {
        assert_eq!(quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn shell_sees_metacharacters_as_literals() {
        for hostile in [
            "x'; cat /tmp/.ssh/id_ed25519 > /tmp/pwned; #",
            "https://x.git && cat /tmp/.ssh/id_ed25519",
            "$(id -u)",
            "`id -u`",
            "a|b;c&d>e<f",
            "line\nbreak",
            "back\\slash",
        ] {
            assert_eq!(echo(hostile), hostile, "not neutralized: {}", hostile);
        }
    }
}
