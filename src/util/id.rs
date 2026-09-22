use crate::model::types::AnyError;
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
    // lower-cased so the generated name is already in the canonical form that
    // volume names are derived in
    format!("{}-{}", prefix, suffix.to_ascii_lowercase())
}

// Workspaces are looked up by their raw name (container/volume labels) but every
// volume name is derived through `sanitize`. Accepting names that differ between the
// two lets `rooz new Victim` slip past the "workspace already exists" check while
// still resolving to workspace `victim`'s volumes, so only canonical names are allowed.
pub fn validate_workspace_name(name: &str) -> Result<(), AnyError> {
    if name.is_empty() {
        return Err("workspace name must not be empty".into());
    }
    let canonical = sanitize(name);
    if name != canonical {
        return Err(format!(
            "invalid workspace name '{}': only lowercase letters, digits and '-' are allowed. Did you mean '{}'?",
            name, canonical
        )
        .into());
    }
    Ok(())
}

pub fn sanitize(dirty: &str) -> String {
    ID_REGEX
        .replace_all(&dirty, "-")
        .to_ascii_lowercase()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{random_suffix, sanitize, validate_workspace_name};

    #[test]
    fn canonical_names_are_accepted() {
        for n in ["victim", "my-ws-1", "tmp-ab3xyz1"] {
            assert!(validate_workspace_name(n).is_ok(), "rejected: {}", n);
        }
    }

    #[test]
    fn case_variant_of_an_existing_workspace_is_rejected() {
        // `rooz new Victim` derives exactly workspace `victim`'s volume names
        let err = validate_workspace_name("Victim").unwrap_err().to_string();
        assert!(err.contains("'victim'"), "no suggestion given: {}", err);
    }

    #[test]
    fn non_canonical_names_are_rejected() {
        for n in ["", "My.Ws", "ws/../other", "ws name", "WS"] {
            assert!(validate_workspace_name(n).is_err(), "accepted: {:?}", n);
        }
    }

    #[test]
    fn random_suffix_is_canonical() {
        for _ in 0..50 {
            let name = random_suffix("tmp");
            assert!(
                validate_workspace_name(&name).is_ok(),
                "not canonical: {}",
                name
            );
        }
    }

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
