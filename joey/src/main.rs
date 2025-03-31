use clap::Parser;
use cli::{Cli, CloneParams, Commands::Clone};
use progress::IndicatifProgress;
use std::path::Path;
mod cli;
mod progress;

fn main() -> anyhow::Result<()> {
    let args = Cli::parse();

    match args {
        Cli {
            command: Clone(CloneParams { repo, dir }),
        } => {
            let dir = dir.as_deref().unwrap_or(".");
            let path = Path::new(dir);
            let url = gix::url::parse(repo.as_str().into())?;
            let mut clone = gix::prepare_clone(url, path)?;
            let mut prep_clone_progress = IndicatifProgress::new();
            let (mut prepare_checkout, _) = clone
                .fetch_then_checkout(&mut prep_clone_progress, &gix::interrupt::IS_INTERRUPTED)?;

            let mut main_worktree_progress = IndicatifProgress::new();
            let (repo, _) = prepare_checkout
                .main_worktree(&mut main_worktree_progress, &gix::interrupt::IS_INTERRUPTED)?;

            let _ = repo
                .find_default_remote(gix::remote::Direction::Fetch)
                .expect("always present after clone")?;
        }
    };

    Ok(())
}
