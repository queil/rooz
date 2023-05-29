use clap::{Parser, Subcommand};

const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";

#[derive(Parser, Debug)]
#[command(about = "Prunes all rooz resources")]
pub struct Prune {}

#[derive(Parser, Debug)]
#[command(about = "Lists workspaces", alias="ls")]
pub struct List {

}

#[derive(Subcommand, Debug)]
pub enum SystemCommands {
    Prune(Prune),
}

#[derive(Parser, Debug)]
#[command(about = "System subcommands")]
pub struct System {
    #[command(subcommand)]
    pub command: SystemCommands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Work { name: Option<String> },
    List(List),
    System(System),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    pub git_ssh_url: Option<String>,
    #[arg(short, long, default_value = DEFAULT_IMAGE, env = "ROOZ_IMAGE")]
    pub image: String,
    #[arg(long)]
    pub pull_image: bool,
    #[arg(short, long, default_value = "bash", env = "ROOZ_SHELL")]
    pub shell: String,
    #[arg(short, long, default_value = "rooz_user", env = "ROOZ_USER")]
    pub user: String,
    #[arg(
        short,
        long,
        env = "ROOZ_CACHES",
        use_value_delimiter = true,
        help = "Enables defining global shared caches"
    )]
    pub caches: Option<Vec<String>>,
    #[arg(
        long,
        help = "Prunes containers and volumes scoped to the provided git repository"
    )]
    pub prune: bool,
    #[arg(short, long)]
    pub privileged: bool,
}
