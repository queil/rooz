const DEFAULT_WIDTH: usize = 100;
const MIN_WIDTH: usize = 40;
const INDENT: &str = "  ";

pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(DEFAULT_WIDTH)
        .clamp(MIN_WIDTH, DEFAULT_WIDTH)
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::<String>::new();
    for paragraph in text.split("\n") {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

// Splits an error message into a one-sentence headline and a wrapped, indented body.
pub fn split_error(message: &str, width: usize) -> (String, String) {
    let message = message.trim();
    let (headline, rest) = match message.split_once(". ") {
        Some((head, rest)) => (format!("{}.", head), rest.trim()),
        None => (message.to_string(), ""),
    };
    let body = wrap(rest, width.saturating_sub(INDENT.len()))
        .into_iter()
        .map(|l| match l.is_empty() {
            true => l,
            false => format!("{}{}", INDENT, l),
        })
        .collect::<Vec<_>>()
        .join("\n");
    (headline, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headline_is_the_first_sentence_and_the_rest_wraps_indented() {
        let (head, body) = split_error(
            "This configuration requests privileged containers: dkr. A privileged container has \
             full access to the host. If you trust them, name them: ROOZ_ALLOW_PRIVILEGED=dkr.",
            50,
        );
        assert_eq!(
            head,
            "This configuration requests privileged containers: dkr."
        );
        assert!(body.lines().count() > 1);
        assert!(body.lines().all(|l| l.starts_with(INDENT) && l.len() <= 50));
        assert!(body.contains("ROOZ_ALLOW_PRIVILEGED=dkr."));
    }

    #[test]
    fn single_sentence_has_no_body() {
        assert_eq!(
            split_error("Workspace not found", 80),
            ("Workspace not found".into(), "".into())
        );
    }

    #[test]
    fn long_words_are_never_split() {
        let url = "x".repeat(120);
        let (_, body) = split_error(&format!("Head. See {}", url), 60);
        assert!(body.contains(&url));
    }
}
