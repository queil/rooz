use bollard::models::MountTypeEnum::VOLUME;
use bollard::{service::Mount, Docker};
use serde::Deserialize;

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
    url: &str,
    target_path: &str,
) -> Result<Mount, Box<dyn std::error::Error + 'static>> {
    let git_vol = RoozVolume {
        path: target_path.into(),
        sharing: RoozVolumeSharing::Exclusive {
            key: id::to_safe_id(url)?,
        },
        role: RoozVolumeRole::Git,
    };

    let vol_name = git_vol.safe_volume_name()?;

    volume::ensure_volume(
        docker,
        &vol_name,
        &git_vol.role.as_str(),
        git_vol.group_key(),
    )
    .await;

    let git_vol_mount = Mount {
        typ: Some(VOLUME),
        source: Some(vol_name.clone()),
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

        let git_vol_mount = git_volume(docker, &url, working_dir).await?;

        let run_spec = RunSpec {
            reason: "git-clone",
            image,
            image_id,
            user: Some(&uid),
            work_dir: None,
            container_name: &id::random_suffix("rooz-git"),
            mounts: Some(vec![git_vol_mount.clone(), ssh::mount("/tmp/.ssh")]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
        };

        let container_result = container::run(&docker, run_spec).await?;

        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
            container::exec_tty(
                "git-clone",
                &docker,
                &container_id,
                false,
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

        container::force_remove(docker, &container_id).await?;

        match RoozCfg::deserialize(toml::de::Deserializer::new(&rooz_cfg)).ok() {
            Some(cfg) => Ok((Some(cfg), Some(url))),
            None => Ok((None, Some(url))),
        }
    } else {
        Ok((None, None))
    }
}
