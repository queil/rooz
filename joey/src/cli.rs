use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "")]
pub struct CloneParams{
    pub repo: String,
    pub dir: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Clone(CloneParams),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}
