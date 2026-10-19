use colored::{Color, ColoredString, Colorize};

const HOST_PALETTE: [Color; 6] = [
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Green,
    Color::Blue,
    Color::BrightRed,
];

fn phase_color(label: &str) -> Color {
    match label {
        "image" => Color::Blue,
        "volumes" => Color::Magenta,
        "clone" => Color::Yellow,
        "container" => Color::Cyan,
        "install" => Color::Green,
        _ => Color::White,
    }
}

/// `[label]` in the colour of its phase.
pub fn tag(label: &str) -> String {
    format!("[{}]", label).color(phase_color(label)).to_string()
}

/// `[install:{container}]` or `[install:{container}:{step}]`; the container keeps a stable colour.
pub fn install_tag(container: &str, step: Option<&str>) -> String {
    let green = |s: &str| s.color(phase_color("install")).to_string();
    let mut out = format!(
        "{}{}",
        green("[install:"),
        container.color(host_color(container)).bold()
    );
    if let Some(step) = step {
        out.push_str(&format!("{}{}", green(":"), step.bold()));
    }
    out.push_str(&green("]"));
    out
}

fn host_color(name: &str) -> Color {
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    HOST_PALETTE[h as usize % HOST_PALETTE.len()]
}

/// A host name in a colour derived from the name, so a host keeps its colour across lines and runs.
pub fn host(name: &str) -> ColoredString {
    name.color(host_color(name)).bold()
}

/// `[host]` with the host's colour.
pub fn host_tag(name: &str) -> ColoredString {
    format!("[{}]", name).color(host_color(name)).bold()
}

/// One terse progress line per phase, on stderr so stdout stays clean for callers.
pub fn phase(label: &str, msg: impl std::fmt::Display) {
    eprintln!("{} {}", tag(label), msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_colour_is_stable_per_name() {
        colored::control::set_override(true);
        assert!(host_tag("dkr").to_string().contains("[dkr]"));
        assert_eq!(host_tag("dkr").to_string(), host_tag("dkr").to_string());
    }

    #[test]
    fn install_tag_plain_text() {
        colored::control::set_override(false);
        assert_eq!(install_tag("claude", None), "[install:claude]");
        assert_eq!(
            install_tag("claude", Some("proxy-cert")),
            "[install:claude:proxy-cert]"
        );
    }
}
