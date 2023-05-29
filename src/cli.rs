use clap::{Parser, Subcommand};

const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";

#[derive(Parser, Debug)]
#[command(about = "Prunes all rooz resources")]
pub struct PruneParams {}

#[derive(Parser, Debug)]
#[command(about = "Lists workspaces", alias="ls")]
pub struct ListParams {

}

#[derive(Subcommand, Debug)]
pub enum SystemCommands {
    Prune(PruneParams),
}

#[derive(Parser, Debug)]
#[command(about = "System subcommands")]
pub struct System {
    #[command(subcommand)]
    pub command: SystemCommands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    New(NewParams),
    Enter (EnterParams),
    List(ListParams),
    System(System),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct WorkParams {
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

#[derive(Parser, Debug)]
pub struct NewParams {
    pub name: String,
    #[command(flatten)]
    pub work: WorkParams,
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct EnterParams {
    pub name: String,
    #[arg(short, long, default_value = "bash", env = "ROOZ_SHELL")]
    pub shell: String,
    #[arg(short, long)]
    pub work_dir: Option<String>,
}


#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}
