use progress::IndicatifProgress;
use std::path::Path;
mod progress;

fn main() -> anyhow::Result<()> {
    let url = "ssh://git@github.com/queil/image";

    let path = Path::new("/Users/queil/gh/joey-test");

    let url = gix::url::parse(url.into())?;

    let mut prep_clone_progress = IndicatifProgress::new();

    let mut clone = gix::prepare_clone(url, path)?;
    let (mut prepare_checkout, _) =
        clone.fetch_then_checkout(&mut prep_clone_progress, &gix::interrupt::IS_INTERRUPTED)?;
    println!(
        "Checking out into {:?} ...",
        prepare_checkout.repo().work_dir().expect("should be there")
    );

    let mut main_worktree_progress = IndicatifProgress::new();

    let (repo, _) = prepare_checkout
        .main_worktree(&mut main_worktree_progress, &gix::interrupt::IS_INTERRUPTED)?;
    println!(
        "Repo cloned into {:?}",
        repo.work_dir().expect("directory pre-created")
    );

    let remote = repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .expect("always present after clone")?;

    println!(
        "Default remote: {} -> {}",
        remote
            .name()
            .expect("default remote is always named")
            .as_bstr(),
        remote
            .url(gix::remote::Direction::Fetch)
            .expect("should be the remote URL")
            .to_bstring(),
    );

    Ok(())
}
