use std::fmt;

use bollard::errors::Error::DockerResponseServerError;
use bollard_stubs::query_parameters::InspectNetworkOptions;

use crate::{
    api::{VolumeApi, WorkspaceApi},
    config::{config::ResolvedStep, runtime::RuntimeConfig},
    constants,
    model::{types::AnyError, volume::RoozVolume},
    util::labels::Labels,
};

#[derive(Debug, Clone)]
pub enum PlanMode {
    New,
    Update { purge: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanEntry {
    pub action: &'static str,
    pub kind: &'static str,
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub workspace: String,
    pub entries: Vec<PlanEntry>,
}

impl Plan {
    fn push(&mut self, action: &'static str, kind: &'static str, name: &str, detail: String) {
        self.entries.push(PlanEntry {
            action,
            kind,
            name: name.to_string(),
            detail,
        });
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for e in &self.entries {
            write!(f, "{:<8} {:<9} {}", e.action, e.kind, e.name)?;
            if !e.detail.is_empty() {
                write!(f, "  {}", e.detail)?;
            }
            writeln!(f)?;
        }
        write!(f, "\nDry run: nothing was changed.")
    }
}

fn steps_summary(steps: &[ResolvedStep]) -> String {
    steps
        .iter()
        .map(|s| {
            let mut flags = Vec::new();
            if s.root {
                flags.push("root");
            }
            if s.skip_if.is_some() {
                flags.push("skip_if");
            }
            if flags.is_empty() {
                s.name.clone()
            } else {
                format!("{}[{}]", s.name, flags.join(","))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl<'a> WorkspaceApi<'a> {
    async fn network_exists(&self, name: &str) -> Result<bool, AnyError> {
        match self
            .api
            .client
            .inspect_network(name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(_) => Ok(true),
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    // What a run of 'new' (or 'update') would do, from the engine's current state. Nothing is
    // created or removed; the engine is only asked. Values that may carry secrets (env, install
    // scripts, the repository URL) are reported by name or masked.
    pub async fn plan(
        &self,
        workspace_key: &str,
        cfg: &RuntimeConfig,
        secrets: &[String],
        mode: &PlanMode,
        pull_image: bool,
    ) -> Result<Plan, AnyError> {
        let cfg_masked = cfg.mask_secrets(secrets);
        let mut plan = Plan {
            workspace: workspace_key.to_string(),
            entries: Vec::new(),
        };

        let mut purged = Vec::<String>::new();
        if let PlanMode::Update { purge } = mode {
            let ws_labels = Labels::from(&[Labels::workspace(workspace_key)]);
            for c in self.api.container.get_all(&ws_labels).await? {
                let name = c
                    .names
                    .and_then(|n| n.into_iter().next())
                    .unwrap_or_default();
                plan.push(
                    "remove",
                    "container",
                    name.trim_start_matches('/'),
                    "".into(),
                );
            }
            if *purge {
                for v in self.api.volume.get_all(&ws_labels).await? {
                    // the stored config is kept: the rebuild reads it
                    if v.labels.get(crate::util::labels::ROLE).map(String::as_str)
                        == Some(crate::util::labels::WORKSPACE_CONFIG_ROLE)
                    {
                        continue;
                    }
                    plan.push("remove", "volume", &v.name, "purge".into());
                    purged.push(v.name);
                }
            }
        }

        let present = self.api.image.exists(&cfg.image).await?;
        plan.push(
            if pull_image || !present {
                "pull"
            } else {
                "present"
            },
            "image",
            &cfg_masked.image,
            "".into(),
        );

        let mut volumes =
            VolumeApi::create_volume_specs(workspace_key, &cfg.data, &cfg.all_mounts(), true)?
                .into_values()
                .map(|v| v.volume)
                .collect::<Vec<_>>();
        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        volumes.dedup_by(|a, b| a.name == b.name);
        for spec in &volumes {
            let exists = !purged.contains(&spec.name) && self.api.volume.exists_for(spec).await?;
            plan.push(
                if exists { "reuse" } else { "create" },
                "volume",
                &spec.name,
                "".into(),
            );
        }

        for (path, shown) in cfg.caches.iter().zip(&cfg_masked.caches) {
            let exists = self
                .api
                .volume
                .exists_for(&RoozVolume::cache(path).to_spec())
                .await?;
            // the volume name is derived from the path, so it is derived from the masked one
            plan.push(
                if exists { "reuse" } else { "create" },
                "cache",
                &RoozVolume::cache(shown).safe_volume_name(),
                shown.clone(),
            );
        }

        let (pair_keys, peer_keys) = RuntimeConfig::workspace_networks(&cfg.sidecars);
        let mut networks = vec![constants::egress_network(workspace_key)];
        networks.extend(
            pair_keys
                .iter()
                .map(|s| constants::pair_network(workspace_key, s)),
        );
        networks.extend(
            peer_keys
                .iter()
                .map(|(a, b)| constants::peer_network(workspace_key, a, b)),
        );
        for n in &networks {
            let exists = self.network_exists(n).await?;
            plan.push(
                if exists { "reuse" } else { "create" },
                "network",
                n,
                "".into(),
            );
        }

        let mut sidecars = cfg_masked.sidecars.iter().collect::<Vec<_>>();
        sidecars.sort_by(|a, b| a.0.cmp(b.0));
        for (name, s) in sidecars {
            let steps = s.install.as_ref().map(|i| i.resolved()).unwrap_or_default();
            let mut detail = vec![format!("image={}", s.image)];
            if let Some(uid) = s.uid {
                detail.push(format!("uid={}", uid));
            }
            if s.privileged {
                detail.push("privileged".into());
            }
            if s.vpn {
                detail.push("vpn".into());
            }
            if s.egress {
                detail.push("egress".into());
            }
            if !steps.is_empty() {
                detail.push(format!("install: {}", steps_summary(&steps)));
            }
            plan.push("create", "sidecar", name, detail.join(" "));
        }

        let mut detail = vec![
            format!("image={}", cfg_masked.image),
            format!("user={} uid={}", cfg_masked.user, cfg.uid),
        ];
        if cfg.privileged {
            detail.push("privileged".into());
        }
        let mut ports = cfg
            .ports
            .iter()
            .map(|(container, host)| match host {
                Some(host) => format!("{}:{}", host, container),
                None => container.clone(),
            })
            .collect::<Vec<_>>();
        ports.sort();
        if !ports.is_empty() {
            detail.push(format!("ports={}", ports.join(",")));
        }
        let mut env = cfg.env.keys().cloned().collect::<Vec<_>>();
        env.sort();
        if !env.is_empty() {
            detail.push(format!("env={}", env.join(",")));
        }
        plan.push("create", "work", workspace_key, detail.join(" "));

        let steps = cfg
            .install
            .as_ref()
            .map(|i| i.resolved())
            .unwrap_or_default();
        for step in &steps {
            let mut flags = Vec::new();
            if step.root {
                flags.push("root");
            }
            if step.skip_if.is_some() {
                flags.push("skip_if");
            }
            plan.push("run", "install", &step.name, flags.join(","));
        }

        if let Some(url) = &cfg_masked.git_ssh_url {
            plan.push("clone", "repo", url, "".into());
        }
        for url in &cfg_masked.extra_repos {
            plan.push("clone", "repo", url, "extra".into());
        }

        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_renders_aligned_lines_and_a_footer() {
        let mut plan = Plan {
            workspace: "ws".into(),
            entries: Vec::new(),
        };
        plan.push("create", "volume", "rooz_ws_work", "".into());
        plan.push("run", "install", "00-a", "root,skip_if".into());
        assert_eq!(
            plan.to_string(),
            "create   volume    rooz_ws_work\nrun      install   00-a  root,skip_if\n\nDry run: nothing was changed."
        );
    }

    #[test]
    fn steps_summary_flags_root_and_skip_if() {
        let step = |name: &str, skip_if: Option<&str>, root: bool| ResolvedStep {
            name: name.into(),
            run: "x".into(),
            skip_if: skip_if.map(String::from),
            root,
        };
        assert_eq!(
            steps_summary(&[step("00-a", None, false), step("10-b", Some("c"), true)]),
            "00-a 10-b[root,skip_if]"
        );
    }
}
