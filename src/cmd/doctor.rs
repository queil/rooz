use bollard::{Docker, query_parameters::ListNetworksOptions};
use colored::Colorize;

use crate::{
    constants,
    model::{types::AnyError, volume::RoozVolume},
    util::{
        backend::{ContainerBackend, ContainerEngine, check_version_floor, root_cause},
        identity, ssh,
    },
};

// Engines hand out network subnets from a fixed pool; docker's default holds about 31
// (172.17-31.0.0/16 and 192.168.0.0/16 split into /20s). Podman's is far larger.
const DOCKER_POOL_SUBNETS: usize = 31;
const POOL_WARN_AT: usize = 25;

enum Level {
    Info,
    Ok,
    Warn,
    Fail,
}

struct Report {
    failed: bool,
}

impl Report {
    fn line(&mut self, level: Level, what: &str, detail: impl std::fmt::Display) {
        let mark = match level {
            Level::Info => "info".dimmed(),
            Level::Ok => "ok".green(),
            Level::Warn => "warn".yellow(),
            Level::Fail => {
                self.failed = true;
                "FAIL".red().bold()
            }
        };
        println!("{:>6}  {:<16} {}", mark, what, detail);
    }
}

/// Checks the machine and the engine rooz runs against; returns whether everything it needs
/// is in place. Needs no working engine connection - a missing one is what it reports.
pub async fn run(
    connection: Result<Docker, bollard::errors::Error>,
    target: &str,
) -> Result<bool, AnyError> {
    let mut r = Report { failed: false };
    r.line(Level::Info, "endpoint", target);

    let docker = match connection {
        Ok(d) => d,
        Err(e) => {
            r.line(
                Level::Fail,
                "engine",
                format!(
                    "cannot connect: {}. Set DOCKER_HOST or start the engine.",
                    root_cause(&e)
                ),
            );
            local_checks(&mut r);
            return Ok(!r.failed);
        }
    };

    let (version, info) = tokio::join!(docker.version(), docker.info());
    let (version, info) = match (version, info) {
        (Ok(v), Ok(i)) => (v, i),
        (v, i) => {
            let cause = v
                .err()
                .or(i.err())
                .map(|e| root_cause(&e))
                .unwrap_or_default();
            r.line(
                Level::Fail,
                "engine",
                format!(
                    "not reachable: {}. Check DOCKER_HOST, that the engine is running, and that \
                     your user may use its socket.",
                    cause
                ),
            );
            local_checks(&mut r);
            return Ok(!r.failed);
        }
    };

    let backend = ContainerBackend::resolve(&version, &info).await?;
    let rootless = info
        .security_options
        .as_ref()
        .is_some_and(|o| o.iter().any(|s| s.contains("rootless")));
    r.line(
        Level::Ok,
        "engine",
        format!(
            "{}, API {}{}",
            match backend.engine {
                ContainerEngine::Unknown => "docker (or compatible)".to_string(),
                ref e => format!("{:?}", e),
            },
            version.api_version.clone().unwrap_or_default(),
            if rootless { ", rootless" } else { "" }
        ),
    );
    match check_version_floor(&version, &backend) {
        Ok(()) => r.line(Level::Ok, "engine version", "meets rooz's minimum"),
        Err(e) => r.line(Level::Fail, "engine version", e),
    }

    let sys_vol = RoozVolume::system_config(constants::SYSTEM_CONFIG_MOUNT).safe_volume_name();
    for (what, name) in [
        ("system config", sys_vol.as_str()),
        ("ssh key", ssh::VOLUME_NAME),
    ] {
        match docker.inspect_volume(name).await {
            Ok(_) => r.line(Level::Ok, what, format!("volume '{}' present", name)),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => r.line(
                Level::Fail,
                what,
                format!("volume '{}' missing. Run 'rooz system init'.", name),
            ),
            Err(e) => r.line(
                Level::Fail,
                what,
                format!("could not inspect '{}': {}", name, e),
            ),
        }
    }

    local_checks(&mut r);

    if !matches!(backend.engine, ContainerEngine::Podman) {
        match docker.list_networks(None::<ListNetworksOptions>).await {
            Ok(networks) => {
                let used = networks
                    .iter()
                    .filter(|n| {
                        n.ipam
                            .as_ref()
                            .and_then(|i| i.config.as_ref())
                            .is_some_and(|c| !c.is_empty())
                    })
                    .count();
                let detail = format!(
                    "{} of about {} default subnets in use",
                    used, DOCKER_POOL_SUBNETS
                );
                let hint = " Each workspace with sidecars takes one. Remove unused workspaces \
                            ('rooz rm') or widen 'default-address-pools' in the daemon config.";
                if used >= DOCKER_POOL_SUBNETS {
                    r.line(Level::Fail, "networks", format!("{}.{}", detail, hint));
                } else if used >= POOL_WARN_AT {
                    r.line(Level::Warn, "networks", format!("{}.{}", detail, hint));
                } else {
                    r.line(Level::Ok, "networks", detail);
                }
            }
            Err(e) => r.line(Level::Warn, "networks", format!("could not list: {}", e)),
        }
    }

    Ok(!r.failed)
}

fn local_checks(r: &mut Report) {
    match identity::key_path() {
        Ok(path) => match identity::read() {
            Ok(Some(_)) => r.line(Level::Ok, "age identity", format!("{}", path.display())),
            Ok(None) => r.line(
                Level::Fail,
                "age identity",
                format!("none at {}. Run 'rooz system init'.", path.display()),
            ),
            Err(e) => r.line(Level::Fail, "age identity", e),
        },
        Err(e) => r.line(Level::Fail, "age identity", e),
    }
}
