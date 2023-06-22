use bollard::network::CreateNetworkOptions;

use crate::{
    backend::Api,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    labels::{self, Labels},
    types::{RoozCfg, RunSpec, WorkSpec},
};

impl<'a> Api<'a> {
    pub async fn new(
        &self,
        spec: &WorkParams,
        config: Option<RoozCfg>,
        persistence: Option<WorkspacePersistence>,
    ) -> Result<String, Box<dyn std::error::Error + 'static>> {
        let ephemeral = persistence.is_none();

        let orig_shell = &spec.shell;
        let orig_user = &spec.user;
        let orig_uid = constants::DEFAULT_UID.to_string();
        let orig_image = &spec.image;
        let (workspace_key, force, enter) = match persistence {
            Some(p) => (p.name.to_string(), p.force, p.enter),
            None => (crate::id::random_suffix("tmp"), false, true),
        };

        let labels = Labels::new(Some(&workspace_key), Some(labels::ROLE_WORK));
        if force {
            self.remove_workspace(&workspace_key, true).await?;
        }

        let labels_sidecar = Labels::new(Some(&workspace_key), Some(labels::ROLE_SIDECAR));

        let network = if let Some(RoozCfg {
            sidecars: Some(_), ..
        }) = &config
        {
            let network_options = CreateNetworkOptions::<&str> {
                name: &workspace_key,
                check_duplicate: true,
                labels: (&labels).into(),

                ..Default::default()
            };

            self.client.create_network(network_options).await?;
            Some(workspace_key.as_ref())
        } else {
            None
        };

        if let Some(RoozCfg {
            sidecars: Some(sidecars),
            ..
        }) = &config
        {
            for (name, s) in sidecars {
                log::debug!("Process sidecar: {}", name);
                self.ensure_image(&s.image, spec.pull_image).await?;
                let container_name = format!("{}-{}", workspace_key, name);
                self.create_container(
                    RunSpec {
                        container_name: &container_name,
                        image: &s.image,
                        force_recreate: force,
                        workspace_key: &workspace_key,
                        labels: (&labels_sidecar).into(),
                        env: s.env.clone(),
                        network,
                        network_aliases: Some(vec![name.into()]),
                        ..Default::default()
                    },
                )
                .await?;
            }
        }

        self.ensure_image(&orig_image, spec.pull_image).await?;
        self.ensure_image(constants::DEFAULT_IMAGE, spec.pull_image).await?;

        let home_dir = format!("/home/{}", &orig_user);
        let work_dir = format!("{}/work", &home_dir);

        let work_spec = WorkSpec {
            image: &orig_image,
            shell: &orig_shell,
            uid: &orig_uid,
            user: &orig_user,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            labels: (&labels).into(),
            ephemeral,
            git_vol_mount: None,
            caches: spec.caches.clone(),
            privileged: spec.privileged,
            force_recreate: force,
            network,
        };

        match &spec.git_ssh_url {
            None => {
                let ws = self.create(&work_spec).await?;
                let volumes = ws.volumes;
                if enter {
                    self.enter(
                        &workspace_key,
                        Some(&work_spec.container_working_dir),
                        Some(&home_dir),
                        &work_spec.shell.as_ref(),
                        None,
                        volumes,
                        &orig_uid,
                        ephemeral,
                    )
                    .await?;
                }
                return Ok(ws.container_id);
            }
            Some(url) => {
                match self
                    .clone_repo(
                        constants::DEFAULT_IMAGE,
                        &orig_uid,
                        url,
                        &workspace_key,
                        &work_dir,
                    )
                    .await?
                {
                    (
                        Some(RoozCfg {
                            image: Some(img),
                            shell,
                            caches: repo_caches,
                            ..
                        }),
                        git_spec,
                    ) => {
                        log::debug!("Image config read from .rooz.toml in the cloned repo");
                        self.ensure_image(&img, spec.pull_image).await?;
                        let sh = shell.or(Some(orig_shell.to_string())).unwrap();
                        let caches = spec.caches.clone();
                        let mut all_caches = vec![];
                        if let Some(caches) = caches {
                            all_caches.extend(caches);
                        }
                        if let Some(caches) = repo_caches {
                            all_caches.extend(caches);
                        };

                        all_caches.dedup();

                        let work_spec = WorkSpec {
                            image: &img,
                            shell: &sh,
                            container_working_dir: &git_spec.dir,
                            git_vol_mount: Some(git_spec.mount),
                            caches: Some(all_caches),
                            ..work_spec
                        };

                        let ws = self.create(&work_spec).await?;
                        let mut volumes = ws.volumes;
                        volumes.push(git_spec.volume);
                        if enter {
                            self.enter(
                                &workspace_key,
                                Some(&git_spec.dir),
                                Some(&home_dir),
                                &sh,
                                None,
                                volumes,
                                &orig_uid,
                                ephemeral,
                            )
                            .await?;
                        }
                        return Ok(ws.container_id);
                    }
                    (None, git_spec) => {
                        let work_spec = WorkSpec {
                            container_working_dir: &git_spec.dir,
                            git_vol_mount: Some(git_spec.mount),
                            ..work_spec
                        };
                        let ws = self.create(&work_spec).await?;
                        let mut volumes = ws.volumes;
                        volumes.push(git_spec.volume);

                        if enter {
                            self.enter(
                                &workspace_key,
                                Some(&git_spec.dir),
                                Some(&home_dir),
                                &work_spec.shell,
                                None,
                                volumes,
                                &orig_uid,
                                ephemeral,
                            )
                            .await?;
                        }
                        return Ok(ws.container_id);
                    }
                    s => {
                        println!("{:?}", s);
                        unreachable!("Unreachable");
                    }
                }
            }
        };
    }
}
