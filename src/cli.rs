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
    #[arg(
        long,
        help = "Initializes rooz with the provided age identity rather than generating a new one"
    )]
    pub age_identity: Option<String>,
}

#[derive(Parser, Debug)]
#[command(about = "Outputs shell completion scripts")]
pub struct CompletionParams {
    pub shell: Shell,
}

#[derive(Subcommand, Debug)]
pub enum SystemCommands {
    Prune(PruneParams),
    Init(InitParams),
    Completion(CompletionParams),
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    Template(TemplateConfigParams),
    Edit(EditConfigParams),
    Show(ShowConfigParams),
}

#[derive(Parser, Debug)]
#[command(about = "System subcommands")]
pub struct System {
    #[command(subcommand)]
    pub command: SystemCommands,
}

#[derive(Parser, Debug)]
#[command(about = "Config subcommands")]
pub struct Config {
    #[command(subcommand)]
    pub command: ConfigCommands,
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
    #[arg(env = "DOCKER_HOST", hide = true)]
    pub local_docker_host: String,
}

#[derive(Clone, Parser, Debug)]
pub struct WorkEnvParams {
    #[arg(
        long = "env_image",
        name = "env_image",
        hide = true,
        env = "ROOZ_IMAGE"
    )]
    pub image: Option<String>,
    #[arg(long = "env_user", name = "env_user", hide = true, env = "ROOZ_USER")]
    pub user: Option<String>,
    #[arg(
        long = "env_shell",
        name = "env_shell",
        hide = true,
        env = "ROOZ_SHELL"
    )]
    pub shell: Option<String>,
    #[arg(
        long = "env_caches",
        name = "env_caches",
        hide = true,
        env = "ROOZ_CACHES",
        use_value_delimiter = true
    )]
    pub caches: Option<Vec<String>>,
}

impl Default for WorkEnvParams {
    fn default() -> Self {
        Self {
            image: Default::default(),
            user: Default::default(),
            shell: Default::default(),
            caches: Default::default(),
        }
    }
}

#[derive(Clone, Parser, Debug)]
pub struct WorkParams {
    #[arg(short, long, alias = "git")]
    pub git_ssh_url: Option<String>,
    #[arg(short, long)]
    pub image: Option<String>,
    #[arg(long)]
    pub home_from_image: Option<String>,
    #[arg(long)]
    pub pull_image: bool,
    #[arg(short, long)]
    pub user: Option<String>,
    #[arg(long)]
    pub uid: Option<u32>,
    #[arg(
        short,
        long,
        use_value_delimiter = true,
        help = "Enables defining global shared caches"
    )]
    pub caches: Option<Vec<String>>,
    #[arg(short, long)]
    pub privileged: Option<bool>,
    #[arg(
        long,
        default_value = "true",
        help = "Starts the workspace immediately"
    )]
    pub start: Option<bool>,
    #[command(flatten)]
    pub env: WorkEnvParams,
}

impl Default for WorkParams {
    fn default() -> Self {
        Self {
            git_ssh_url: Default::default(),
            image: Default::default(),
            home_from_image: Default::default(),
            pull_image: Default::default(),
            user: Default::default(),
            caches: Default::default(),
            privileged: Default::default(),
            start: Default::default(),
            env: Default::default(),
            uid: Default::default(),
        }
    }
}

#[derive(Parser, Debug)]
#[command(about = "Creates a new workspace (container + volumes)")]
pub struct NewParams {
    pub name: String,
    #[command(flatten)]
    pub work: WorkParams,
    #[arg(
        long,
        help = "Configures the new workspace from a config file given by the path.",
        alias = "config"
    )]
    pub config_path: Option<String>,
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
#[command(about = "Starts a workspace")]
pub struct StartParams {
    pub name: String,
}

#[derive(Parser, Debug)]
#[command(about = "Restarts a workspace's main container")]
pub struct RestartParams {
    pub name: String,
    #[arg(long, default_value = "false", help = "")]
    pub all_containers: Option<bool>,
}

#[derive(Parser, Debug)]
#[command(about = "Lists workspaces", alias = "ls")]
pub struct ListParams {}

#[derive(Parser, Debug)]
#[command(about = "Removes a workspace", alias = "rm")]
pub struct RemoveParams {
    #[arg(required_unless_present = "all")]
    pub name: Option<String>,
    #[arg(short, long, help = "Kill running containers")]
    pub force: bool,
    #[arg(short, long, conflicts_with = "name", help = "Remove all workspaces")]
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

#[derive(Parser, Debug, Clone, clap::ValueEnum)]
pub enum ConfigPart {
    Origin,
    Body,
    Runtime,
}

#[derive(Parser, Debug, Clone, clap::ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Yaml,
}

#[derive(Parser, Debug)]
#[command(about = "Shows a workspace's configuration")]
pub struct ShowConfigParams {
    #[arg()]
    pub name: String,
    #[arg(long, short, value_enum, default_value = "body")]
    pub part: ConfigPart,
    #[arg(long, short)]
    pub output: Option<ConfigFormat>,
}

#[derive(Parser, Debug)]
#[command(about = "Outputs a workspace configuration template")]
pub struct TemplateConfigParams {
    #[arg(long, short)]
    pub format: ConfigFormat,
}

#[derive(Parser, Debug)]
#[command(about = "Edits a local configuration file")]
pub struct EditConfigParams {
    pub config_path: String,
}

#[derive(Parser, Debug)]
#[command(about = "Updates a workspace created from a config file")]
pub struct UpdateParams {
    #[arg()]
    pub name: String,
    #[command(flatten)]
    pub env: WorkEnvParams,
    #[arg(
        long,
        short,
        help = "If set allows to tweak the existing config and apply the edited version."
    )]
    pub tweak: bool,
    #[arg(
        long,
        conflicts_with = "tweak",
        help = "If set it removes the workspace volumes. WARNING: potential data loss ahead"
    )]
    pub purge: bool,
    #[arg(long, help = "If set it skips pulling new images")]
    pub no_pull: bool,
}

#[derive(Parser, Debug)]
#[command(
    about = "Attaches VsCode to a workspace. (requires VsCode installed and 'code' in $PATH)"
)]
pub struct CodeParams {
    #[arg()]
    pub name: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    New(NewParams),
    Enter(EnterParams),
    Code(CodeParams),
    Start(StartParams),
    Stop(StopParams),
    Restart(RestartParams),
    Remove(RemoveParams),
    Update(UpdateParams),
    List(ListParams),
    Config(Config),
    Tmp(TmpParams),
    Remote(RemoteParams),
    System(System),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}
