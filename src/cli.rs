use clap::Parser;

const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
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
    #[arg(
        long,
        conflicts_with = "prune",
        help = "Prunes all rooz containers and volumes apart from the ssh-key vol"
    )]
    pub prune_all: bool,
    #[arg(short, long)]
    pub privileged: bool,
}
