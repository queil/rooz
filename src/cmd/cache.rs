use std::collections::{BTreeSet, HashMap};

use bollard::query_parameters::{ListContainersOptions, ListVolumesOptions, RemoveVolumeOptions};
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

use crate::{
    api::{Api, image::human_bytes},
    cmd::list::{human_age, now_secs},
    model::{
        types::AnyError,
        volume::{cache_scope, cache_volume_name},
    },
    util::{
        id::sanitize,
        labels::{CACHE_ROLE, Labels, WORKSPACE_KEY},
    },
};

#[derive(Debug, Serialize)]
struct Cache {
    name: String,
    size: Option<i64>,
    created: Option<String>,
    workspaces: Vec<String>,
    mounted: bool,
}

#[derive(Tabled)]
struct CacheView {
    #[tabled(rename = "CACHE")]
    name: String,
    #[tabled(rename = "SIZE")]
    size: String,
    #[tabled(rename = "AGE")]
    age: String,
    #[tabled(rename = "MOUNTED BY")]
    workspaces: String,
}

fn scope_prefix(scope: &str) -> String {
    format!("rooz_{}_{}_", sanitize(CACHE_ROLE), sanitize(scope))
}

// A cache is addressed by its full volume name, by the name without the scope prefix, or by
// the path it caches ('~/.cargo').
fn resolve<'a>(arg: &str, scope: &str, known: &'a [Cache]) -> Option<&'a Cache> {
    let prefix = scope_prefix(scope);
    let candidates = [
        arg.to_string(),
        format!("{}{}", prefix, arg),
        cache_volume_name(scope, arg),
    ];
    known
        .iter()
        .find(|c| candidates.iter().any(|cand| *cand == c.name))
}

impl<'a> Api<'a> {
    async fn caches(&self, all_scopes: bool) -> Result<Vec<Cache>, AnyError> {
        let prefix = scope_prefix(&cache_scope());

        let volumes = self
            .client
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(Labels::from(&[Labels::role(CACHE_ROLE)]).into()),
                ..Default::default()
            }))
            .await?
            .volumes
            .unwrap_or_default();

        // size comes from the engine's disk usage report; engines without one show no size
        let sizes: HashMap<String, i64> = match self.client.df(None).await {
            Ok(df) => df
                .volume_usage
                .and_then(|u| u.items)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| {
                    Some((
                        v.get("Name")?.as_str()?.to_string(),
                        v.get("UsageData")?.get("Size")?.as_i64()?,
                    ))
                })
                .filter(|(_, size)| *size >= 0)
                .collect(),
            Err(e) => {
                log::debug!("Could not read volume sizes: {}", e);
                HashMap::new()
            }
        };

        // every container, stopped or not: the engine refuses to remove a volume any of them
        // references
        let mut mounted_by = HashMap::<String, BTreeSet<String>>::new();
        for c in self
            .client
            .list_containers(Some(ListContainersOptions {
                all: true,
                ..Default::default()
            }))
            .await?
        {
            let owner = c
                .labels
                .as_ref()
                .and_then(|l| l.get(WORKSPACE_KEY).cloned())
                .unwrap_or_else(|| "?".to_string());
            for m in c.mounts.unwrap_or_default() {
                if let Some(name) = m.name {
                    mounted_by.entry(name).or_default().insert(owner.clone());
                }
            }
        }

        let mut caches = volumes
            .into_iter()
            .filter(|v| all_scopes || v.name.starts_with(&prefix))
            .map(|v| {
                let workspaces = mounted_by
                    .get(&v.name)
                    .map(|s| s.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                Cache {
                    size: sizes.get(&v.name).copied(),
                    created: v.created_at.clone(),
                    mounted: !workspaces.is_empty(),
                    workspaces,
                    name: v.name,
                }
            })
            .collect::<Vec<_>>();
        caches.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(caches)
    }

    pub async fn cache_ls(&self, all_scopes: bool, json: bool) -> Result<(), AnyError> {
        let caches = self.caches(all_scopes).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&caches)?);
            return Ok(());
        }
        let prefix = scope_prefix(&cache_scope());
        let now = now_secs();
        let views = caches.iter().map(|c| CacheView {
            name: if all_scopes {
                c.name.clone()
            } else {
                c.name.strip_prefix(&prefix).unwrap_or(&c.name).to_string()
            },
            size: c.size.map(human_bytes).unwrap_or_else(|| "-".to_string()),
            age: human_age(c.created.as_deref().and_then(parse_rfc3339_secs), now),
            workspaces: c.workspaces.join(", "),
        });
        println!("{}", Table::new(views).with(Style::blank()));
        Ok(())
    }

    pub async fn cache_rm(&self, names: &[String]) -> Result<(), AnyError> {
        let scope = cache_scope();
        let caches = self.caches(false).await?;

        // check everything first so a bad name removes nothing
        let mut targets = Vec::new();
        for n in names {
            let c = resolve(n, &scope, &caches).ok_or(format!(
                "No cache '{}' in scope '{}'. Check 'rooz cache ls'.",
                n, scope
            ))?;
            if c.mounted {
                return Err(format!(
                    "Cache '{}' is mounted by: {}. Remove those workspaces (or update them without \
                     the cache) first.",
                    n,
                    c.workspaces.join(", ")
                )
                .into());
            }
            targets.push(c.name.clone());
        }
        for name in targets {
            self.remove_cache(&name).await?;
        }
        Ok(())
    }

    pub async fn cache_prune(&self, dry_run: bool) -> Result<(), AnyError> {
        let unused = self
            .caches(false)
            .await?
            .into_iter()
            .filter(|c| !c.mounted)
            .collect::<Vec<_>>();
        let mut freed = 0;
        for c in &unused {
            freed += c.size.unwrap_or(0);
            if dry_run {
                println!("Would remove {}", c.name);
            } else {
                self.remove_cache(&c.name).await?;
            }
        }
        println!(
            "{} {} cache(s), {}",
            if dry_run { "Would remove" } else { "Removed" },
            unused.len(),
            human_bytes(freed)
        );
        Ok(())
    }

    async fn remove_cache(&self, name: &str) -> Result<(), AnyError> {
        self.client
            .remove_volume(name, Some(RemoveVolumeOptions::default()))
            .await
            .map_err(|e| format!("Could not remove cache '{}': {}", name, e))?;
        println!("Removed {}", name);
        Ok(())
    }
}

// Volume CreatedAt is RFC 3339 ("2026-10-16T10:41:32Z"); engines may add an offset or fraction
fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(name: &str, mounted: bool) -> Cache {
        Cache {
            name: name.into(),
            size: None,
            created: None,
            workspaces: vec![],
            mounted,
        }
    }

    #[test]
    fn caches_resolve_by_full_name_short_name_or_path() {
        let known = vec![cache("rooz_cache_uid-1000_---cargo", false)];
        for arg in ["rooz_cache_uid-1000_---cargo", "---cargo", "~/.cargo"] {
            assert!(resolve(arg, "uid-1000", &known).is_some(), "{}", arg);
        }
        assert!(resolve("~/.nuget", "uid-1000", &known).is_none());
    }

    #[test]
    fn rfc3339_parses_to_unix_seconds() {
        assert_eq!(parse_rfc3339_secs("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(parse_rfc3339_secs("2026-10-16T10:41:32Z"), Some(1792147292));
        assert_eq!(
            parse_rfc3339_secs("2026-10-16T12:41:32.5+02:00"),
            Some(1792147292)
        );
        assert_eq!(parse_rfc3339_secs("garbage"), None);
    }
}
