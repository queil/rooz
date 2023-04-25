use clap::{Parser, Subcommand};

const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";

#[derive(Parser, Debug)]
#[command(about = "Lists workspaces", alias="ls")]
pub struct List {

}

#[derive(Parser, Debug)]
#[command(about = "Prunes workspaces")]
pub struct Prune {
    #[arg(
        help = "Limits the pruning scope to resources related to the given key only"
    )]
    //TODO: it'll be required once the default/generic workspace gets disallowed, so it'll be either --all or key
    pub git_ssh_url: Option<String>,
    #[arg(
        long,
        conflicts_with = "git_ssh_url",
        help = "Prune all rooz containers and volumes apart from the ssh-key vol"
    )]
    pub all: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Work {
        name: Option<String>,
    },
    Prune(Prune),
    List(List),
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
    #[arg(short, long)]
    pub privileged: bool,
}
