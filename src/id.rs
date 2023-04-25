use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use regex::Regex;

pub fn random_suffix(prefix: &str) -> String {
    let suffix: String = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    format!("{}-{}", prefix, suffix)
}

pub fn to_safe_id(dirty: &str) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let re = Regex::new(r"[^a-zA-Z0-9_.-]").unwrap();
    Ok(re.replace_all(&dirty, "-").to_ascii_lowercase().to_string())
}
