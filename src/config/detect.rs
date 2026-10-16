use std::path::Path;

use crate::model::types::AnyError;

// Floating tags where the ecosystem lets a newer toolchain build older projects, so `rooz
// update` (which pulls) keeps them current without a rooz release. .NET is the exception: the
// SDK image carries only its own runtime, so the version follows the project's target framework.
const RUST_IMAGE: &str = "docker.io/library/rust:1";
const NODE_IMAGE: &str = "docker.io/library/node:lts";
const GO_IMAGE: &str = "docker.io/library/golang:1";
const PYTHON_IMAGE: &str = "docker.io/library/python:3";
const DOTNET_IMAGE: &str = "mcr.microsoft.com/dotnet/sdk";
// used only when no project file states a target framework
const DOTNET_FALLBACK: &str = "10.0";

#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    pub stack: &'static str,
    pub marker: String,
    pub image: String,
    pub caches: Vec<&'static str>,
}

fn has(dir: &Path, name: &str) -> bool {
    dir.join(name).is_file()
}

fn files_with_ext(dir: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .collect();
    found.sort();
    found
}

fn project_files(dir: &Path, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    out.extend(files_with_ext(dir, &["csproj", "fsproj", "vbproj"]));
    if depth == 0 {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let skip = name.starts_with('.')
            || ["bin", "obj", "node_modules", "target"].contains(&name.as_str());
        if entry.path().is_dir() && !skip {
            project_files(&entry.path(), depth - 1, out);
        }
    }
}

// The highest `netX.Y` among the TargetFramework(s) of the project files in the directory and
// two levels below it (`src/App/App.csproj` is the common layout).
fn dotnet_version(dir: &Path) -> Result<Option<String>, AnyError> {
    let mut projects = Vec::new();
    project_files(dir, 2, &mut projects);

    let mut best: Option<(u32, u32)> = None;
    for project in projects {
        let text = std::fs::read_to_string(&project)?;
        let Ok(doc) = roxmltree::Document::parse(&text) else {
            continue;
        };
        for node in doc
            .descendants()
            .filter(|n| n.has_tag_name("TargetFramework") || n.has_tag_name("TargetFrameworks"))
        {
            for tfm in node.text().unwrap_or_default().split(';') {
                // "net8.0", "net8.0-windows"; netstandard, netcoreapp and netfx are not SDK lines
                let Some(rest) = tfm.trim().strip_prefix("net") else {
                    continue;
                };
                let ver = rest.split('-').next().unwrap_or_default();
                let mut parts = ver.split('.');
                if let (Some(Ok(major)), Some(Ok(minor))) = (
                    parts.next().map(str::parse::<u32>),
                    parts.next().map(str::parse::<u32>),
                ) {
                    if major >= 5 && best.map_or(true, |b| (major, minor) > b) {
                        best = Some((major, minor));
                    }
                }
            }
        }
    }
    Ok(best.map(|(major, minor)| format!("{}.{}", major, minor)))
}

/// The stacks whose marker files sit in `dir`, in a fixed order, with an image and the cache
/// paths that stack's tooling fills.
pub fn detect(dir: &Path) -> Result<Vec<Detected>, AnyError> {
    let mut found = Vec::new();

    if has(dir, "Cargo.toml") {
        found.push(Detected {
            stack: "rust",
            marker: "Cargo.toml".into(),
            image: RUST_IMAGE.into(),
            // CARGO_HOME of the official image
            caches: vec!["/usr/local/cargo/registry", "/usr/local/cargo/git"],
        });
    }

    let dotnet_marker = ["global.json"]
        .iter()
        .map(|s| s.to_string())
        .chain(
            files_with_ext(dir, &["sln", "slnx", "csproj", "fsproj", "vbproj"])
                .into_iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
        )
        .find(|m| has(dir, m));
    if let Some(marker) = dotnet_marker {
        let version = dotnet_version(dir)?.unwrap_or_else(|| DOTNET_FALLBACK.to_string());
        found.push(Detected {
            stack: "dotnet",
            marker,
            image: format!("{}:{}", DOTNET_IMAGE, version),
            caches: vec!["~/.nuget/packages"],
        });
    }

    if has(dir, "package.json") {
        let cache = if has(dir, "pnpm-lock.yaml") {
            "~/.local/share/pnpm/store"
        } else if has(dir, "yarn.lock") {
            "~/.cache/yarn"
        } else {
            "~/.npm"
        };
        found.push(Detected {
            stack: "node",
            marker: "package.json".into(),
            image: NODE_IMAGE.into(),
            caches: vec![cache],
        });
    }

    if has(dir, "go.mod") {
        found.push(Detected {
            stack: "go",
            marker: "go.mod".into(),
            image: GO_IMAGE.into(),
            // GOPATH of the official image, and the default build cache under HOME
            caches: vec!["/go/pkg/mod", "~/.cache/go-build"],
        });
    }

    if let Some(marker) = ["pyproject.toml", "requirements.txt", "Pipfile"]
        .iter()
        .find(|m| has(dir, m))
    {
        found.push(Detected {
            stack: "python",
            marker: marker.to_string(),
            image: PYTHON_IMAGE.into(),
            caches: vec![if has(dir, "uv.lock") {
                "~/.cache/uv"
            } else {
                "~/.cache/pip"
            }],
        });
    }

    Ok(found)
}

/// A starter config. With detected stacks the first one chooses the image, and the caches of all
/// of them are listed.
pub fn template(detected: &[Detected]) -> String {
    let mut out = String::from("# rooz workspace config\n");

    match detected.split_first() {
        Some((primary, rest)) => {
            out.push_str(&format!(
                "# detected: {} ({})\n",
                primary.stack, primary.marker
            ));
            for other in rest {
                out.push_str(&format!(
                    "# also found: {} ({}) - this image does not carry its toolchain; use an image \
                     that has both, or run it in a sidecar\n",
                    other.stack, other.marker
                ));
            }
            out.push_str(&format!("image: {}\n", primary.image));
            out.push_str(
                "# caches start out root-owned: run 'rooz enter <workspace> --chown' once\n",
            );
            out.push_str("caches:\n");
            let mut seen = Vec::new();
            for cache in detected.iter().flat_map(|d| d.caches.iter()) {
                if !seen.contains(cache) {
                    seen.push(cache);
                    out.push_str(&format!("  - {}\n", cache));
                }
            }
        }
        None => {
            out.push_str("# image: docker.io/library/alpine:latest\n# caches:\n#   - ~/.cache\n");
        }
    }

    out.push_str(
        "# user: rooz_user\n\
         # shell: [bash]\n\
         # env:\n\
         #   KEY: value\n\
         # install:\n\
         #   10-tools:\n\
         #     run: echo installing\n\
         #     skip_if: command -v mytool\n\
         # ports: [\"8080:8080\"]\n\
         # sidecars: {}\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::{FileFormat, RoozCfg};

    fn dir_with(files: &[(&str, &str)]) -> tempdir::Dir {
        tempdir::Dir::with(files)
    }

    // std only: a scratch directory removed on drop
    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn with(files: &[(&str, &str)]) -> Dir {
                let path = std::env::temp_dir().join(format!(
                    "rooz-detect-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
                let _ = std::fs::remove_dir_all(&path);
                for (name, body) in files {
                    let file = path.join(name);
                    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                    std::fs::write(file, body).unwrap();
                }
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn dotnet_image_follows_the_highest_target_framework() {
        let d = dir_with(&[
            ("app.sln", ""),
            (
                "src/a/a.csproj",
                "<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>",
            ),
            (
                "src/b/b.csproj",
                "<Project><PropertyGroup><TargetFrameworks>netstandard2.0;net10.0-windows</TargetFrameworks></PropertyGroup></Project>",
            ),
        ]);
        let found = detect(&d.0).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].stack, "dotnet");
        assert_eq!(found[0].marker, "app.sln");
        assert_eq!(found[0].image, "mcr.microsoft.com/dotnet/sdk:10.0");
    }

    #[test]
    fn dotnet_without_a_target_framework_uses_the_fallback() {
        let d = dir_with(&[("global.json", "{}")]);
        assert_eq!(
            detect(&d.0).unwrap()[0].image,
            format!("{}:{}", DOTNET_IMAGE, DOTNET_FALLBACK)
        );
    }

    #[test]
    fn each_stack_is_found_by_its_marker() {
        for (file, stack) in [
            ("Cargo.toml", "rust"),
            ("package.json", "node"),
            ("go.mod", "go"),
            ("requirements.txt", "python"),
        ] {
            let d = dir_with(&[(file, "")]);
            let found = detect(&d.0).unwrap();
            assert_eq!(found.len(), 1, "{}", file);
            assert_eq!(found[0].stack, stack);
        }
        assert!(
            detect(&dir_with(&[("README.md", "")]).0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn node_cache_follows_the_lockfile() {
        let d = dir_with(&[("package.json", "{}"), ("yarn.lock", "")]);
        assert_eq!(detect(&d.0).unwrap()[0].caches, vec!["~/.cache/yarn"]);
    }

    #[test]
    fn templates_parse_as_configs_with_and_without_detection() {
        let d = dir_with(&[("Cargo.toml", ""), ("package.json", "{}")]);
        let detected = detect(&d.0).unwrap();
        let text = template(&detected);
        let cfg = RoozCfg::from_string(&text, FileFormat::Yaml).unwrap();
        assert_eq!(cfg.image.as_deref(), Some(RUST_IMAGE));
        assert!(cfg.caches.unwrap().contains(&"~/.npm".to_string()));
        assert!(text.contains("also found: node"));

        RoozCfg::from_string(&template(&[]), FileFormat::Yaml).unwrap();
    }
}
