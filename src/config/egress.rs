use crate::{
    config::config::{DataValue, MountSource, RoozCfg, RoozSidecar},
    model::types::AnyError,
};
use handlebars::Handlebars;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// `egress:` on a sidecar: `true` or `false` as ever, or a policy - the sidecar gets a proxy of its
// own and reaches only what the policy lists, through it.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum SidecarEgress {
    Open(bool),
    Proxied(EgressPolicy),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicy {
    // hosts reached through as they are; `*.example.com` for the hosts under example.com
    #[serde(default)]
    pub allow: Vec<String>,
    // 443 unless set
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<u16>>,
    // hosts the proxy intercepts to apply header rules, for credentials the sidecar must not hold
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inject: Vec<EgressInject>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EgressInject {
    pub host: String,
    #[serde(default)]
    pub rules: Vec<EgressRule>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EgressRule {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_header: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub set: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

pub const PROXY_PORT: u16 = 9124;
const PROXY_CONFIG: &str = "/etc/joey/egress.json";
const PROXY_CA_CERT: &str = "/etc/joey/ca.pem";
const PROXY_CA_KEY: &str = "/etc/joey/ca.key";
// where Debian, Ubuntu and Alpine's update-ca-certificates picks a CA up
pub const AGENT_CA_CERT: &str = "/usr/local/share/ca-certificates/rooz-egress.crt";
// the proxy needs no account; this is the uid its files are written for
const PROXY_UID: i32 = 65532;

impl SidecarEgress {
    pub fn is_open(&self) -> bool {
        matches!(self, SidecarEgress::Open(true))
    }

    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        let render = |s: &String| -> Result<String, AnyError> { Ok(reg.render_template(s, vars)?) };
        Ok(match self {
            SidecarEgress::Open(b) => SidecarEgress::Open(*b),
            SidecarEgress::Proxied(p) => SidecarEgress::Proxied(EgressPolicy {
                allow: p.allow.iter().map(render).collect::<Result<_, _>>()?,
                ports: p.ports.clone(),
                inject: p
                    .inject
                    .iter()
                    .map(|i| {
                        Ok(EgressInject {
                            host: render(&i.host)?,
                            rules: i
                                .rules
                                .iter()
                                .map(|r| {
                                    Ok(EgressRule {
                                        methods: r.methods.clone(),
                                        paths: r.paths.clone(),
                                        when_header: r.when_header.clone(),
                                        set: r
                                            .set
                                            .iter()
                                            .map(|(k, v)| Ok((k.clone(), render(v)?)))
                                            .collect::<Result<_, AnyError>>()?,
                                        remove: r.remove.clone(),
                                    })
                                })
                                .collect::<Result<_, AnyError>>()?,
                        })
                    })
                    .collect::<Result<_, AnyError>>()?,
            }),
        })
    }
}

pub fn proxy_name(sidecar: &str) -> String {
    format!("{}-egress", sidecar)
}

// The credential in the sidecar's proxy URL, and the `Proxy-Authorization` value the proxy checks.
// The proxy holds what the sidecar must not, and the work container reaches it too (it shares a
// network with every sidecar), so reaching its port must not be enough to use it.
fn proxy_token() -> (String, String) {
    use base64::{Engine as _, engine::general_purpose};
    use rand::{RngExt, distr::Alphanumeric, rng};
    let token: String = rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let header = format!(
        "Basic {}",
        general_purpose::STANDARD.encode(format!("rooz:{}", token))
    );
    (token, header)
}

fn inline(content: String, once: bool) -> MountSource {
    MountSource::InlineDataValue(DataValue::InlineContent {
        content,
        executable: None,
        once: once.then_some(true),
    })
}

// What joey reads
fn proxy_config(policy: &EgressPolicy, with_ca: bool, auth: &str) -> String {
    let mut cfg = serde_json::json!({
        "listen": format!("0.0.0.0:{}", PROXY_PORT),
        "allow": policy.allow,
        "ports": policy.ports.clone().unwrap_or_else(|| vec![443]),
        "inject": policy.inject,
        "auth": auth,
    });
    if with_ca {
        cfg["ca"] = serde_json::json!({"cert": PROXY_CA_CERT, "key": PROXY_CA_KEY});
    }
    serde_json::to_string_pretty(&cfg).expect("a json value serializes")
}

pub struct Ca {
    pub cert: String,
    pub key: String,
}

// One CA per workspace, for the inject hosts' certificates. Its files are written once, so the CA
// a sidecar's image was built to trust stays the CA in use across updates.
pub fn generate_ca() -> Result<Ca, AnyError> {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
        .distinguished_name
        .push(DnType::CommonName, "rooz egress proxy");
    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok(Ca {
        cert: cert.pem(),
        key: key.serialize_pem(),
    })
}

// A sidecar with an egress policy becomes two: itself, cut off from the internet and pointed at
// its proxy, and the proxy, the only one of the two with a way out. Returns what was generated
// that must not be persisted in clear (the CA's key).
pub fn expand(cfg: &mut RoozCfg, image: &str) -> Result<Vec<String>, AnyError> {
    expand_with(cfg, image, generate_ca)
}

pub fn expand_with(
    cfg: &mut RoozCfg,
    image: &str,
    make_ca: impl Fn() -> Result<Ca, AnyError>,
) -> Result<Vec<String>, AnyError> {
    let Some(sidecars) = cfg.sidecars.as_mut() else {
        return Ok(vec![]);
    };
    let policies = sidecars
        .iter()
        .filter_map(|(name, s)| match &s.egress {
            Some(SidecarEgress::Proxied(p)) => Some((name.clone(), p.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    if policies.is_empty() {
        return Ok(vec![]);
    }
    let ca = match policies.iter().any(|(_, p)| !p.inject.is_empty()) {
        true => Some(make_ca()?),
        false => None,
    };
    // generated, never to be stored in clear: each proxy's client credential, and the CA's key
    let mut generated = Vec::new();

    for (name, policy) in policies {
        let proxy = proxy_name(&name);
        if sidecars.contains_key(&proxy) {
            return Err(format!(
                "sidecar '{}': its egress policy needs a proxy named '{}', which is another sidecar",
                name, proxy
            )
            .into());
        }
        let with_ca = !policy.inject.is_empty();
        let agent = sidecars.get_mut(&name).expect("listed above");
        agent.egress = Some(SidecarEgress::Open(false));
        let mut peers = agent.peers.clone().unwrap_or_default();
        // what it reaches without the proxy: itself and its peers
        let no_proxy = ["localhost", "127.0.0.1", "::1"]
            .iter()
            .map(|s| s.to_string())
            .chain(peers.iter().cloned())
            .collect::<Vec<_>>()
            .join(",");
        peers.push(proxy.clone());
        agent.peers = Some(peers);
        let (token, auth) = proxy_token();
        let url = format!("http://rooz:{}@{}:{}", token, proxy, PROXY_PORT);
        generated.push(token);
        generated.push(auth.clone());
        let env = agent.env.get_or_insert_with(IndexMap::new);
        // the sidecar's own settings win
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            env.entry(key.to_string()).or_insert_with(|| url.clone());
        }
        for key in ["NO_PROXY", "no_proxy"] {
            env.entry(key.to_string())
                .or_insert_with(|| no_proxy.clone());
        }
        let mut proxy_mounts = IndexMap::new();
        proxy_mounts.insert(
            PROXY_CONFIG.to_string(),
            inline(proxy_config(&policy, with_ca, &auth), false),
        );
        if let (true, Some(ca)) = (with_ca, &ca) {
            agent
                .mounts
                .get_or_insert_with(IndexMap::new)
                .insert(AGENT_CA_CERT.to_string(), inline(ca.cert.clone(), true));
            env.entry("NODE_EXTRA_CA_CERTS".to_string())
                .or_insert_with(|| AGENT_CA_CERT.to_string());
            proxy_mounts.insert(PROXY_CA_CERT.to_string(), inline(ca.cert.clone(), true));
            proxy_mounts.insert(PROXY_CA_KEY.to_string(), inline(ca.key.clone(), true));
        }
        sidecars.insert(
            proxy,
            RoozSidecar {
                image: Some(image.to_string()),
                env: None,
                command: Some(vec!["/joey".into()]),
                args: Some(vec![
                    "egress-proxy".into(),
                    "--config".into(),
                    PROXY_CONFIG.into(),
                ]),
                mounts: Some(proxy_mounts),
                ports: None,
                privileged: None,
                vpn: None,
                init: None,
                install: None,
                work_dir: None,
                uid: Some(PROXY_UID),
                egress: Some(SidecarEgress::Open(true)),
                shell: None,
                peers: None,
            },
        );
    }
    if let Some(ca) = ca {
        generated.push(ca.key);
    }
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> RoozCfg {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn fake_ca() -> Result<Ca, AnyError> {
        Ok(Ca {
            cert: "CERT".into(),
            key: "KEY".into(),
        })
    }

    fn content(m: &MountSource) -> &str {
        match m {
            MountSource::InlineDataValue(DataValue::InlineContent { content, .. }) => content,
            _ => panic!("not inline content"),
        }
    }

    #[test]
    fn egress_true_and_false_read_as_before() {
        let c = cfg(
            "sidecars:\n  a:\n    image: x\n    egress: true\n  b:\n    image: x\n    egress: false\n",
        );
        let s = c.sidecars.unwrap();
        assert_eq!(s["a"].egress, Some(SidecarEgress::Open(true)));
        assert_eq!(s["b"].egress, Some(SidecarEgress::Open(false)));
    }

    #[test]
    fn a_policy_cuts_the_sidecar_off_and_gives_it_a_proxy() {
        let mut c = cfg(
            "sidecars:\n  agent:\n    image: x\n    peers: [dkr]\n    env:\n      NO_PROXY: mine\n    egress:\n      allow: [github.com]\n  dkr:\n    image: y\n",
        );
        let secrets = expand_with(&mut c, "joey-image", fake_ca).unwrap();
        assert_eq!(
            secrets.len(),
            2,
            "the proxy credential, as token and header, is masked"
        );
        let (token, auth) = (&secrets[0], &secrets[1]);
        let s = c.sidecars.unwrap();
        let agent = &s["agent"];
        assert_eq!(agent.egress, Some(SidecarEgress::Open(false)));
        assert_eq!(
            agent.peers.as_ref().unwrap(),
            &vec!["dkr".to_string(), "agent-egress".into()]
        );
        let env = agent.env.as_ref().unwrap();
        let url = format!("http://rooz:{}@agent-egress:9124", token);
        assert_eq!(env["HTTP_PROXY"], url);
        assert_eq!(env["HTTPS_PROXY"], url);
        assert_eq!(token.len(), 32);
        assert_eq!(env["NO_PROXY"], "mine", "the sidecar's own setting wins");
        assert_eq!(env["no_proxy"], "localhost,127.0.0.1,::1,dkr");
        assert!(!env.contains_key("NODE_EXTRA_CA_CERTS"));

        let proxy = &s["agent-egress"];
        assert_eq!(proxy.image.as_deref(), Some("joey-image"));
        assert_eq!(proxy.egress, Some(SidecarEgress::Open(true)));
        let mounts = proxy.mounts.as_ref().unwrap();
        assert_eq!(mounts.len(), 1);
        let json: serde_json::Value = serde_json::from_str(content(&mounts[PROXY_CONFIG])).unwrap();
        assert_eq!(json["allow"], serde_json::json!(["github.com"]));
        assert_eq!(
            json["auth"],
            serde_json::json!(auth),
            "the proxy checks the sidecar's credential"
        );
        assert!(auth.starts_with("Basic "));
        assert_eq!(json["ports"], serde_json::json!([443]));
        assert!(json.get("ca").is_none());
    }

    #[test]
    fn inject_brings_a_ca_whose_key_only_the_proxy_gets() {
        let mut c = cfg(
            "sidecars:\n  agent:\n    image: x\n    egress:\n      allow: [github.com]\n      inject:\n        - host: api.anthropic.com\n          rules:\n            - when_header: x-cc-req\n              set: {authorization: Bearer t}\n              remove: [x-cc-req]\n",
        );
        let secrets = expand_with(&mut c, "joey-image", fake_ca).unwrap();
        assert_eq!(
            secrets.len(),
            3,
            "the proxy credential and the CA key are masked"
        );
        assert_eq!(secrets[2], "KEY", "the key is masked when stored");
        let s = c.sidecars.unwrap();
        let agent_mounts = s["agent"].mounts.as_ref().unwrap();
        assert_eq!(content(&agent_mounts[AGENT_CA_CERT]), "CERT");
        assert!(
            agent_mounts.values().all(|m| content(m) != "KEY"),
            "the sidecar got the CA key"
        );
        assert_eq!(
            s["agent"].env.as_ref().unwrap()["NODE_EXTRA_CA_CERTS"],
            AGENT_CA_CERT
        );
        let proxy_mounts = s["agent-egress"].mounts.as_ref().unwrap();
        assert_eq!(content(&proxy_mounts[PROXY_CA_KEY]), "KEY");
        // written once: the CA a built image trusts stays the CA in use
        assert!(matches!(
            &proxy_mounts[PROXY_CA_KEY],
            MountSource::InlineDataValue(DataValue::InlineContent {
                once: Some(true),
                ..
            })
        ));
        let json: serde_json::Value =
            serde_json::from_str(content(&proxy_mounts[PROXY_CONFIG])).unwrap();
        assert_eq!(json["inject"][0]["host"], "api.anthropic.com");
        assert_eq!(
            json["inject"][0]["rules"][0]["set"]["authorization"],
            "Bearer t"
        );
        assert_eq!(json["ca"]["key"], PROXY_CA_KEY);
    }

    #[test]
    fn a_proxy_name_taken_by_another_sidecar_is_refused() {
        let mut c = cfg(
            "sidecars:\n  agent:\n    image: x\n    egress:\n      allow: [a.b]\n  agent-egress:\n    image: y\n",
        );
        assert!(expand_with(&mut c, "j", fake_ca).is_err());
    }

    #[test]
    fn a_generated_ca_is_a_ca() {
        let ca = generate_ca().unwrap();
        assert!(ca.cert.contains("BEGIN CERTIFICATE"));
        assert!(ca.key.contains("PRIVATE KEY"));
    }
}
