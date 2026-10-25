use super::hosts;
use hyper::http::{HeaderName, HeaderValue, Method};
use serde::Deserialize;
use std::collections::BTreeMap;

// What the proxy lets through. Rooz writes it; anything the proxy cannot make sense of stops it
// from starting rather than being read some other way.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: String,
    // hosts reached through as they are: the TLS session is the client's and the server's
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default = "default_ports")]
    pub ports: Vec<u16>,
    // hosts whose TLS the proxy terminates, to apply header rules before passing requests on
    #[serde(default)]
    pub inject: Vec<InjectHost>,
    // the CA the proxy issues inject hosts' certificates from; the clients trust its certificate
    pub ca: Option<CaFiles>,
    // PEM files of roots trusted for upstream servers, on top of the built-in web roots
    #[serde(default)]
    pub upstream_roots: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaFiles {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectHost {
    pub host: String,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

// The first rule a request matches applies; a request that matches none goes on unchanged.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    // any method when empty
    #[serde(default)]
    pub methods: Vec<String>,
    // any path when empty; `/v1/*` is a prefix, anything else the exact path
    #[serde(default)]
    pub paths: Vec<String>,
    // only requests that carry this header (a marker the client sets) match
    pub when_header: Option<String>,
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

fn default_listen() -> String {
    "0.0.0.0:9124".into()
}

fn default_ports() -> Vec<u16> {
    vec![443]
}

impl Config {
    pub fn from_json(body: &str) -> Result<Self, String> {
        let cfg: Config = serde_json::from_str(body).map_err(|e| format!("config: {}", e))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        for p in &self.allow {
            if !hosts::valid_pattern(p) {
                return Err(format!(
                    "allow: '{}' is not a host name or '*.' followed by one",
                    p
                ));
            }
        }
        if self.ports.is_empty() {
            return Err("ports: at least one port".into());
        }
        if !self.inject.is_empty() && self.ca.is_none() {
            return Err("inject needs a ca to issue the hosts' certificates from".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for h in &self.inject {
            if h.host.starts_with("*.") || !hosts::valid_pattern(&h.host) {
                return Err(format!(
                    "inject: '{}' is not a host name (no wildcards: each host is intercepted on its own)",
                    h.host
                ));
            }
            if !seen.insert(h.host.clone()) {
                return Err(format!("inject: '{}' is listed twice", h.host));
            }
            if self.allow.iter().any(|p| hosts::matches(p, &h.host)) {
                return Err(format!(
                    "'{}' is in allow and inject: an inject host is intercepted, so it cannot also pass through",
                    h.host
                ));
            }
            for r in &h.rules {
                for m in &r.methods {
                    if m != &m.to_ascii_uppercase() || Method::from_bytes(m.as_bytes()).is_err() {
                        return Err(format!("inject {}: '{}' is not a method", h.host, m));
                    }
                }
                for p in &r.paths {
                    let body = p.strip_suffix('*').unwrap_or(p);
                    if !p.starts_with('/') || body.contains('*') {
                        return Err(format!(
                            "inject {}: path '{}' must start with '/' and may only end in '*'",
                            h.host, p
                        ));
                    }
                }
                let names = r
                    .set
                    .keys()
                    .chain(r.remove.iter())
                    .chain(r.when_header.iter());
                for n in names {
                    HeaderName::from_bytes(n.as_bytes())
                        .map_err(|_| format!("inject {}: '{}' is not a header name", h.host, n))?;
                }
                for v in r.set.values() {
                    HeaderValue::from_str(v)
                        .map_err(|_| format!("inject {}: a header value is not valid", h.host))?;
                }
            }
        }
        Ok(())
    }

    pub fn allows(&self, host: &str, port: u16) -> bool {
        self.ports.contains(&port)
            && (self.inject_host(host).is_some()
                || self.allow.iter().any(|p| hosts::matches(p, host)))
    }

    pub fn inject_host(&self, host: &str) -> Option<&InjectHost> {
        self.inject.iter().find(|h| h.host == host)
    }
}

impl Rule {
    pub fn matches(&self, method: &str, path: &str, has: impl Fn(&str) -> bool) -> bool {
        (self.methods.is_empty() || self.methods.iter().any(|m| m == method))
            && (self.paths.is_empty()
                || self.paths.iter().any(|p| match p.strip_suffix('*') {
                    Some(prefix) => path.starts_with(prefix),
                    None => path == p,
                }))
            && self.when_header.as_deref().is_none_or(has)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: &str) -> Result<Config, String> {
        Config::from_json(json)
    }

    #[test]
    fn defaults_are_https_only_and_nothing_allowed() {
        let c = cfg("{}").unwrap();
        assert_eq!(c.ports, vec![443]);
        assert!(!c.allows("github.com", 443));
    }

    #[test]
    fn a_config_that_cannot_be_read_one_way_is_refused() {
        for bad in [
            r#"{"allow": ["https://github.com"]}"#,
            r#"{"allow": ["github.com"], "unknown": 1}"#,
            r#"{"ports": []}"#,
            r#"{"inject": [{"host": "api.example.com"}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "*.example.com"}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example"}, {"host": "a.example"}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "allow": ["*.example"], "inject": [{"host": "a.example"}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example", "rules": [{"methods": ["post"]}]}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example", "rules": [{"paths": ["v1/*"]}]}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example", "rules": [{"paths": ["/*/x"]}]}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example", "rules": [{"set": {"bad header": "x"}}]}]}"#,
            r#"{"ca": {"cert": "c", "key": "k"}, "inject": [{"host": "a.example", "rules": [{"set": {"x": "a\nb"}}]}]}"#,
        ] {
            assert!(cfg(bad).is_err(), "accepted: {}", bad);
        }
    }

    #[test]
    fn a_port_or_host_outside_the_lists_is_not_allowed() {
        let c = cfg(r#"{"allow": ["*.github.com", "crates.io"], "ports": [443, 8443]}"#).unwrap();
        assert!(c.allows("api.github.com", 443));
        assert!(c.allows("crates.io", 8443));
        assert!(!c.allows("crates.io", 80));
        assert!(!c.allows("github.com", 443));
        assert!(!c.allows("example.com", 443));
    }

    #[test]
    fn a_rule_matches_on_method_path_and_marker() {
        let r: Rule = serde_json::from_str(
            r#"{"methods": ["POST"], "paths": ["/v1/*", "/health"], "when_header": "x-marker"}"#,
        )
        .unwrap();
        let marked = |n: &str| n == "x-marker";
        let unmarked = |_: &str| false;
        assert!(r.matches("POST", "/v1/messages", marked));
        assert!(r.matches("POST", "/health", marked));
        assert!(!r.matches("GET", "/v1/messages", marked));
        assert!(!r.matches("POST", "/healthz", marked));
        assert!(!r.matches("POST", "/v1/messages", unmarked));
    }
}
