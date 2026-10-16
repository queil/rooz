use tabled::{Table, Tabled, settings::Style};

use crate::{
    api::ConfigApi,
    cli::{ConfigFormat, ConfigPart},
    config::{
        config::{ConfigPath, ConfigType, FileFormat, RoozCfg},
        resolved,
        runtime::{RuntimeConfig, SECRET_MASK},
    },
    model::types::AnyError,
    util::labels::{self, Labels},
};

const EXTENDS_SEPARATOR: &str = "---";

impl<'a> ConfigApi<'a> {
    pub async fn show(
        &self,
        workspace_key: &str,
        part: ConfigPart,
        output: Option<ConfigFormat>,
        resolved: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::workspace_config(workspace_key);

        let volume = self
            .api
            .volume
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        if resolved {
            let origin = volume.labels.get(labels::CONFIG_ORIGIN).ok_or(
                "Workspace has no config file to resolve: it was created from flags only.",
            )?;
            return self.show_resolved(workspace_key, origin).await;
        }

        let content: Option<String> = match part {
            ConfigPart::Origin => Some(
                volume
                    .labels
                    .get(labels::CONFIG_ORIGIN)
                    .unwrap()
                    .to_string(),
            ),
            ConfigPart::Body => {
                let new_format = output.map(|c| match c {
                    ConfigFormat::Yaml => FileFormat::Yaml,
                });

                let body = self.read(workspace_key, &ConfigType::Body).await?;

                let body = if let Some(format) = new_format {
                    let origin_path = volume.labels.get(labels::CONFIG_ORIGIN).unwrap();
                    let original_format = FileFormat::from_path(&origin_path)?;
                    let cfg = RoozCfg::from_string(&body, original_format)?;
                    cfg.to_string(format)?
                } else {
                    body
                };

                let bases_body = self.read(workspace_key, &ConfigType::Bases).await?;
                if bases_body.is_empty() {
                    Some(body)
                } else {
                    Some(format!("{}\n{}\n{}", body, EXTENDS_SEPARATOR, bases_body))
                }
            }
            ConfigPart::Runtime => {
                let runtime_config = self.read(workspace_key, &ConfigType::Runtime).await?;
                match output {
                    Some(ConfigFormat::Yaml) => {
                        let cfg = RuntimeConfig::from_string(runtime_config.to_string())?;
                        Some(serde_yaml::to_string(&cfg)?)
                    }
                    _ => Some(runtime_config.to_string()),
                }
            }
        };

        println!("{}", content.unwrap_or("N/A".to_string()));

        Ok(())
    }

    // The stored config files merged the way a run merges them, each setting naming the layer
    // it came from. Templates are not expanded and secrets are never printed.
    async fn show_resolved(&self, workspace_key: &str, origin: &str) -> Result<(), AnyError> {
        let main_name = match ConfigPath::from_str(origin)? {
            ConfigPath::File { path } => std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(path),
            ConfigPath::Git { .. } => origin.to_string(),
        };
        let format = FileFormat::from_path(origin)?;

        let mut layers = vec![("default".to_string(), RoozCfg::default())];

        let bases = self.read(workspace_key, &ConfigType::Bases).await?;
        if !bases.trim().is_empty() {
            for chunk in bases.split("\n---\n") {
                let name = chunk
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("# "))
                    .unwrap_or("base")
                    .to_string();
                layers.push((name, RoozCfg::from_string(chunk, format)?));
            }
        }

        let body = self.read(workspace_key, &ConfigType::Body).await?;
        layers.push((main_name, RoozCfg::from_string(&body, format)?));

        let rows = resolved::resolve(&layers)?
            .into_iter()
            .map(|r| ResolvedView {
                value: display_value(&r.path, &r.value),
                path: r.path,
                from: r.from,
            });

        println!("{}", Table::new(rows).with(Style::blank()));
        Ok(())
    }
}

#[derive(Tabled)]
struct ResolvedView {
    #[tabled(rename = "SETTING")]
    path: String,
    #[tabled(rename = "VALUE")]
    value: String,
    #[tabled(rename = "FROM")]
    from: String,
}

fn display_value(path: &str, value: &str) -> String {
    if path.starts_with("secrets.") {
        return SECRET_MASK.to_string();
    }
    match value.lines().next() {
        Some(first) if value.lines().count() > 1 => format!("{} …", first),
        Some(first) if first.chars().count() > 60 => {
            format!("{} …", first.chars().take(60).collect::<String>())
        }
        Some(first) => first.to_string(),
        None => String::new(),
    }
}
