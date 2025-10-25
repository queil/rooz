use lazy_static::lazy_static;
use rand::{Rng, distr::Alphanumeric, rng};
use regex::Regex;

lazy_static! {
    static ref ID_REGEX: Regex = Regex::new(r"[^a-zA-Z0-9_.-]").unwrap();
}

pub fn random_suffix(prefix: &str) -> String {
    let suffix: String = rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    format!("{}-{}", prefix, suffix)
}

pub fn to_safe_id(dirty: &str) -> String {
    ID_REGEX
        .replace_all(&dirty, "-")
        .to_ascii_lowercase()
        .to_string()
}
