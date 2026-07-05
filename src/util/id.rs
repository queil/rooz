use lazy_static::lazy_static;
use rand::{RngExt, distr::Alphanumeric, rng};
use regex::Regex;

lazy_static! {
    static ref ID_REGEX: Regex = Regex::new(r"[^a-zA-Z0-9-]").unwrap();
}

pub fn random_suffix(prefix: &str) -> String {
    let suffix: String = rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    format!("{}-{}", prefix, suffix)
}

pub fn sanitize(dirty: &str) -> String {
    ID_REGEX
        .replace_all(&dirty, "-")
        .to_ascii_lowercase()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn alphanumeric_and_hyphens_pass_through() {
        assert_eq!(sanitize("my-volume-1"), "my-volume-1");
    }

    #[test]
    fn special_chars_replaced_by_hyphen() {
        assert_eq!(sanitize("~/.cargo/registry"), "---cargo-registry");
    }

    #[test]
    fn uppercase_lowercased() {
        assert_eq!(sanitize("MyVolume"), "myvolume");
    }

    #[test]
    fn underscore_collision_pinned() {
        // ~/a.txt and ~/a_txt both produce "--a-txt" — pinned known wart
        assert_eq!(sanitize("~/a.txt"), sanitize("~/a_txt"));
        assert_eq!(sanitize("~/a.txt"), "--a-txt");
    }
}
