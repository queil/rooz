use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

use crate::backend::GitApi;
use crate::labels::Labels;
use crate::types::{AnyError, GitCloneSpec};
use crate::{
    container, id, ssh,
    types::{ContainerResult, RoozCfg, RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec},
};

fn get_clone_dir(root_dir: &str, git_ssh_url: &str) -> String {
    let clone_work_dir = git_ssh_url
        .split(&['/'])
        .last()
        .unwrap_or("repo")
        .replace(".git", "")
        .to_string();

    log::debug!("Clone dir: {}", &clone_work_dir);

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    work_dir
}

impl<'a> GitApi<'a> {
    async fn git_volume(
        &self,
        target_path: &str,
        workspace_key: &str,
    ) -> Result<(Mount, RoozVolume), AnyError> {
        let git_vol = RoozVolume {
            path: target_path.into(),
            sharing: RoozVolumeSharing::Exclusive {
                key: workspace_key.into(),
            },
            role: RoozVolumeRole::Git,
        };

        let vol_name = git_vol.safe_volume_name();

        self.api
            .volume
            .ensure_volume(&vol_name, &git_vol.role, git_vol.key(), false)
            .await?;

        let git_vol_mount = Mount {
            typ: Some(VOLUME),
            source: Some(vol_name.into()),
            target: Some((git_vol.path).to_string()),
            read_only: Some(false),
            ..Default::default()
        };

        Ok((git_vol_mount, git_vol.clone()))
    }

    pub async fn clone_repo(
        &self,
        image: &str,
        uid: &str,
        url: &str,
        workspace_key: &str,
        working_dir: &str,
    ) -> Result<(Option<RoozCfg>, GitCloneSpec), AnyError> {
        let tmp_working_dir = "/tmp/git";
        let clone_dir = format!("{}", &tmp_working_dir);

        let clone_cmd = container::inject(
            format!(
                    r#"export GIT_SSH_COMMAND="ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts"
                    ls "{}/.git" > /dev/null 2>&1 || git clone {} {}"#,
                &clone_dir, &url, &clone_dir
            )
            .as_ref(),
            "clone.sh",
        );

        let (tmp_git_vol_mount, _) = self.git_volume(tmp_working_dir, workspace_key).await?;
        let labels = Labels::new(Some(&workspace_key), Some("git"));

        let run_spec = RunSpec {
            reason: "git-clone",
            image,
            uid: &uid,
            work_dir: None,
            container_name: &id::random_suffix("rooz-git"),
            workspace_key,
            mounts: Some(vec![tmp_git_vol_mount.clone(), ssh::mount("/tmp/.ssh")]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels: (&labels).into(),
            ..Default::default()
        };

        let container_result = self.api.container.create(run_spec).await?;
        self.api.container.start(container_result.id()).await?;
        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
            self.api.exec.ensure_user(container_id).await?;
            self.api.exec.chown(&container_id, uid, &clone_dir).await?;

            self.api
                .exec
                .tty(
                    "git-clone",
                    &container_id,
                    true,
                    None,
                    None,
                    Some(clone_cmd.iter().map(String::as_str).collect()),
                )
                .await?;
        };

        let rooz_cfg = self
            .api
            .exec
            .output(
                "rooz-toml",
                &container_id,
                None,
                Some(vec![
                    "cat",
                    format!("{}/{}", clone_dir, ".rooz.toml").as_ref(),
                ]),
            )
            .await?;

        log::debug!("Repo config result: {}", &rooz_cfg);

        self.api.container.remove(&container_id, true).await?;

        let cfg = RoozCfg::from_string(rooz_cfg).ok();

        let clone_dir = get_clone_dir(working_dir, url);
        let (mount, rooz_vol) = self.git_volume(&clone_dir, workspace_key).await?;

        let work_spec = GitCloneSpec {
            dir: clone_dir,
            mount,
            volume: rooz_vol,
        };

        match cfg {
            Some(cfg) => Ok((Some(cfg), work_spec)),
            None => Ok((None, work_spec)),
        }
    }
}
