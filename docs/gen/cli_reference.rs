//! Prints the CLI reference page for the docs site: every command's `--help`, in tree order.
//!
//!   cargo run --example cli-reference > docs/src/reference/cli.md

use clap::{Command, CommandFactory};
use rooz::cli::Cli;

fn hide_env_values(cmd: Command) -> Command {
    // the page is built on CI, whose environment must not end up in it
    cmd.mut_args(|a| a.hide_env_values(true))
        .mut_subcommands(hide_env_values)
}

fn render(cmd: &mut Command, path: &str, out: &mut String) {
    out.push_str(&format!("## `{path}`\n\n```text\n"));
    out.push_str(cmd.render_long_help().to_string().trim_end());
    out.push_str("\n```\n\n");
    for sub in cmd.get_subcommands_mut() {
        if sub.get_name() == "help" || sub.is_hide_set() {
            continue;
        }
        let path = format!("{path} {}", sub.get_name());
        render(sub, &path, out);
    }
}

fn main() {
    let mut cmd = hide_env_values(Cli::command().bin_name("rooz"));
    cmd.build();
    let mut out = format!(
        "# CLI reference\n\nGenerated from `rooz {}`.\n\n",
        cmd.get_version().unwrap_or_default()
    );
    render(&mut cmd, "rooz", &mut out);
    print!("{}", out.trim_end());
    println!();
}
