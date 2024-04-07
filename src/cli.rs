use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser, Debug)]
#[command(about = "Prunes all rooz resources")]
pub struct PruneParams {}

#[derive(Parser, Debug)]
#[command(about = "Initializes rooz system")]
pub struct InitParams {
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
#[command(about = "Initializes rooz system")]
pub struct CompletionParams {
    pub shell: Shell,
}

#[derive(Subcommand, Debug)]
pub enum SystemCommands {
    Prune(PruneParams),
    Init(InitParams),
    Completion(CompletionParams),
}

#[derive(Parser, Debug)]
#[command(about = "System subcommands")]
pub struct System {
    #[command(subcommand)]
    pub command: SystemCommands,
}

#[derive(Parser, Debug)]
#[command(about = "Establishes a connection to a remote host")]
pub struct RemoteParams {
    #[arg(
        long,
        short,
        env = "ROOZ_REMOTE_SSH_URL",
        help = "Remote host's SSH url"
    )]
    pub ssh_url: String,
    #[arg(
        long,
        short,
        default_value = "23750",
        env = "ROOZ_REMOTE_LOCAL_PORT",
        help = "A local port forwarded to the remote docker socket"
    )]
    pub local_port: i16,
}

#[derive(Clone, Parser, Debug)]
pub struct WorkspacePersistence {
    pub name: String,
    #[arg(
        short,
        long,
        help = "Replace an existing workspace with a new empty one. WARNING: potential data loss ahead"
    )]
    pub replace: bool,
    #[arg(
        short,
        long,
        conflicts_with = "replace",
        requires = "config",
        help = "Recreates workspace containers with the given configuration"
    )]
    pub apply: bool,
}

#[derive(Clone, Parser, Debug)]
pub struct WorkParams {
    #[arg(short, long, alias = "git")]
    pub git_ssh_url: Option<String>,
    #[arg(long, hide = true, env = "ROOZ_IMAGE")]
    pub env_image: Option<String>,
    #[arg(short, long)]
    pub image: Option<String>,
    #[arg(long)]
    pub pull_image: bool,
    #[arg(long, hide = true, env = "ROOZ_USER")]
    pub env_user: Option<String>,
    #[arg(long, hide = true, env = "ROOZ_SHELL")]
    pub env_shell: Option<String>,
    #[arg(short, long)]
    pub user: Option<String>,
    #[arg(long, hide = true, env = "ROOZ_CACHES", use_value_delimiter = true)]
    pub env_caches: Option<Vec<String>>,
    #[arg(
        short,
        long,
        use_value_delimiter = true,
        help = "Enables defining global shared caches"
    )]
    pub caches: Option<Vec<String>>,
    #[arg(short, long)]
    pub privileged: Option<bool>,
}

#[derive(Parser, Debug)]
#[command(about = "Creates a new workspace (container + volumes)")]
pub struct NewParams {
    #[command(flatten)]
    pub persistence: WorkspacePersistence,
    #[command(flatten)]
    pub work: WorkParams,
    #[arg(
        long,
        help = "Configures the new workspace from .rooz.toml at the given path"
    )]
    pub config: Option<String>,
}

#[derive(Parser, Debug)]
#[command(about = "Enters an ephemeral workspace")]
pub struct TmpParams {
    #[command(flatten)]
    pub work: WorkParams,
    #[arg(short, long)]
    pub root: bool,
    #[arg(short, long, default_value = "bash", env = "ROOZ_SHELL")]
    pub shell: String,
}

#[derive(Parser, Debug)]
#[command(
    about = "Opens an interactive shell to a workspace's container",
    alias = "jump"
)]
pub struct EnterParams {
    pub name: String,
    #[arg(short, long)]
    pub shell: Option<String>,
    #[arg(short, long)]
    pub root: bool,
    #[arg(short, long)]
    pub work_dir: Option<String>,
    #[arg(short, long)]
    pub container: Option<String>,
}

#[derive(Parser, Debug)]
#[command(about = "Lists workspaces", alias = "ls")]
pub struct ListParams {}

#[derive(Parser, Debug)]
#[command(about = "Removes a workspace", alias = "rm")]
pub struct RemoveParams {
    #[arg(required_unless_present = "all")]
    pub name: Option<String>,
    #[arg(short, long)]
    pub force: bool,
    #[arg(short, long, conflicts_with = "name")]
    pub all: bool,
}

#[derive(Parser, Debug)]
#[command(about = "Stops a workspace")]
pub struct StopParams {
    #[arg(required_unless_present = "all")]
    pub name: Option<String>,
    #[arg(short, long, conflicts_with = "name")]
    pub all: bool,
}

#[derive(Parser, Debug)]
#[command(about = "Describes a workspace")]
pub struct DescribeParams {
    #[arg()]
    pub name: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    New(NewParams),
    Enter(EnterParams),
    Tmp(TmpParams),
    List(ListParams),
    Remove(RemoveParams),
    Describe(DescribeParams),
    Stop(StopParams),
    Remote(RemoteParams),
    System(System),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}
