use bollard::models::MountTypeEnum::{TMPFS, VOLUME};
use bollard::{service::Mount, Docker};

use crate::labels::Labels;
use crate::{
    container, id, ssh,
    types::{ContainerResult, RoozCfg, RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec},
    volume,
};

//TODO: return volume from clone_repo and make this private
pub fn get_clone_dir(root_dir: &str, git_ssh_url: Option<String>) -> String {
    let clone_work_dir = match git_ssh_url {
        Some(url) => url
            .split(&['/'])
            .last()
            .unwrap_or("repo")
            .replace(".git", "")
            .to_string(),
        None => "".into(),
    };

    log::debug!("Clone dir: {}", &clone_work_dir);

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    work_dir
}

//TODO: return volume from clone_repo and make this private
pub async fn git_volume(
    docker: &Docker,
    target_path: &str,
    workspace_key: &str,
    ephemeral: bool,
) -> Result<Mount, Box<dyn std::error::Error + 'static>> {
    let git_vol = RoozVolume {
        path: target_path.into(),
        sharing: RoozVolumeSharing::Exclusive {
            key: workspace_key.into(),
        },
        role: RoozVolumeRole::Git,
    };

    let vol_name = git_vol.safe_volume_name()?;

    if !ephemeral {
        volume::ensure_volume(
            docker,
            &vol_name,
            &git_vol.role.as_str(),
            git_vol.key(),
            false,
        )
        .await?;
    }

    let git_vol_mount = Mount {
        typ: Some(if ephemeral { TMPFS } else { VOLUME }),
        source: if ephemeral {
            None
        } else {
            Some(vol_name.into())
        },
        target: Some(git_vol.path.into()),
        read_only: Some(false),
        ..Default::default()
    };

    Ok(git_vol_mount)
}

pub async fn clone_repo(
    docker: &Docker,
    image: &str,
    image_id: &str,
    uid: &str,
    git_ssh_url: Option<String>,
    workspace_key: &str,
    ephemeral: bool,
) -> Result<(Option<RoozCfg>, Option<String>), Box<dyn std::error::Error + 'static>> {
    if let Some(url) = git_ssh_url.clone() {
        let working_dir = "/tmp/git";
        let clone_dir = format!("{}", &working_dir);

        let clone_cmd = container::inject(
            format!(
                    r#"export GIT_SSH_COMMAND="ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts"
                    ls "{}/.git" > /dev/null 2>&1 || git clone {} {}"#,
                &clone_dir, &url, &clone_dir
            )
            .as_ref(),
            "clone.sh",
        );

        let git_vol_mount = git_volume(docker, working_dir, workspace_key, ephemeral).await?;
        let labels = Labels::new(Some(&workspace_key), Some("git"));

        let run_spec = RunSpec {
            reason: "git-clone",
            image,
            image_id,
            user: Some(&uid),
            work_dir: None,
            container_name: &id::random_suffix("rooz-git"),
            workspace_key,
            mounts: Some(vec![git_vol_mount.clone(), ssh::mount("/tmp/.ssh")]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels: (&labels).into(),
            ..Default::default()
        };

        let container_result = container::create(&docker, run_spec).await?;
        container::start(docker, container_result.id()).await?;
        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
            container::exec_tty(
                "git-clone",
                &docker,
                &container_id,
                true,
                None,
                None,
                Some(clone_cmd.iter().map(String::as_str).collect()),
            )
            .await?;
        };

        let rooz_cfg = container::exec_output(
            "rooz-toml",
            &docker,
            &container_id,
            None,
            Some(vec![
                "cat",
                format!("{}/{}", clone_dir, ".rooz.toml").as_ref(),
            ]),
        )
        .await?;

        log::debug!("Repo config result: {}", &rooz_cfg);

        container::remove(docker, &container_id, true).await?;

        let cfg = RoozCfg::from_string(rooz_cfg).ok();
        match cfg {
            Some(cfg) => Ok((Some(cfg), Some(url))),
            None => Ok((None, Some(url))),
        }
    } else {
        Ok((None, None))
    }
}
