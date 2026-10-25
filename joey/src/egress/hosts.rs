// Host names as the proxy compares them: lower case, no trailing dot.
pub fn normalize(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

// `example.com` is that host alone; `*.example.com` any host under it, not the apex.
pub fn matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(&format!(".{}", suffix)),
        None => host == pattern,
    }
}

// A pattern is a host name, optionally behind one leading `*.`: no scheme, port, path or other
// wildcard, so it can only ever mean what it reads as.
pub fn valid_pattern(pattern: &str) -> bool {
    let name = pattern.strip_prefix("*.").unwrap_or(pattern);
    !name.is_empty()
        && name == normalize(name)
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_pattern_is_that_host_alone() {
        assert!(matches("github.com", "github.com"));
        assert!(!matches("github.com", "api.github.com"));
        assert!(!matches("github.com", "evilgithub.com"));
    }

    #[test]
    fn a_wildcard_is_the_hosts_under_it() {
        assert!(matches("*.github.com", "api.github.com"));
        assert!(matches("*.github.com", "a.b.github.com"));
        assert!(!matches("*.github.com", "github.com"));
        assert!(!matches("*.github.com", "evilgithub.com"));
        assert!(!matches("*.github.com", ".github.com"));
    }

    #[test]
    fn host_names_compare_without_case_or_a_trailing_dot() {
        assert_eq!(normalize("API.GitHub.com."), "api.github.com");
    }

    #[test]
    fn a_pattern_cannot_hide_a_scheme_port_path_or_other_wildcard() {
        for ok in ["github.com", "*.github.com", "a-b.example", "localhost"] {
            assert!(valid_pattern(ok), "{}", ok);
        }
        for bad in [
            "",
            "*",
            "*.",
            "https://github.com",
            "github.com:443",
            "github.com/x",
            "a.*.com",
            "GitHub.com",
            "-a.com",
            "a..com",
            "*github.com",
        ] {
            assert!(!valid_pattern(bad), "{}", bad);
        }
    }
}
