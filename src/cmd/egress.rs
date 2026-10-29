use std::{
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
};

use bollard::query_parameters::LogsOptions;
use colored::{ColoredString, Colorize};
use futures::{Stream, StreamExt};
use indexmap::IndexMap;
use serde::Deserialize;

use crate::{
    api::Api,
    model::types::AnyError,
    util::labels::{self, Labels},
};

// One decision of a sidecar's egress proxy, as joey logs it: a line of JSON
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Event {
    pub decision: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub rule: Option<usize>,
    pub reason: Option<String>,
    pub ua: Option<String>,
}

const PROXY_SUFFIX: &str = "-egress";

impl Event {
    pub fn parse(line: &str) -> Option<Event> {
        serde_json::from_str(line.trim()).ok()
    }

    // refused, or let through only because the proxy was learning
    fn denied(&self) -> bool {
        matches!(self.decision.as_str(), "deny" | "learn")
    }

    // a host the policy's `allow` would have to list
    fn wanted_host(&self) -> Option<&str> {
        let refused = self.decision == "deny" && self.reason.as_deref() == Some("not allowed");
        (refused || self.decision == "learn")
            .then_some(self.host.as_deref())
            .flatten()
    }

    // where it went: a request inside an intercepted tunnel, a host and port, or a request line
    fn target(&self) -> String {
        match (&self.host, self.port, &self.method, &self.path) {
            (Some(h), _, Some(m), Some(p)) => format!("{} {} {}", h, m, p),
            (Some(h), Some(port), _, _) => format!("{}:{}", h, port),
            (Some(h), None, _, _) => h.clone(),
            (None, _, m, p) => format!(
                "{} {}",
                m.as_deref().unwrap_or("-"),
                p.as_deref().unwrap_or("-")
            ),
        }
    }

    fn detail(&self) -> String {
        match (self.rule, &self.reason) {
            (Some(rule), _) => format!("rule {}", rule),
            (None, Some(reason)) => reason.clone(),
            (None, None) => String::new(),
        }
    }

    fn ua(&self) -> &str {
        self.ua.as_deref().unwrap_or("-")
    }
}

const NO_CREDENTIALS: &str = "no proxy credentials";
const ANSWERED: &str = "auth";

// Some clients (.NET's HttpClient) send the proxy credential only once challenged: a CONNECT
// without it, refused, then the same again with it. A refusal for a host and client that also got
// through is that challenge, answered - not something the policy refused.
fn through(e: &Event) -> Option<(String, String)> {
    matches!(e.decision.as_str(), "pass" | "inject" | "learn")
        .then(|| Some((e.host.clone()?, e.ua().to_string())))
        .flatten()
}

fn challenge(e: &Event) -> Option<(String, String)> {
    (e.decision == "deny" && e.reason.as_deref() == Some(NO_CREDENTIALS))
        .then(|| Some((e.host.clone()?, e.ua().to_string())))
        .flatten()
}

fn mark_answered(e: &mut Event) {
    e.decision = ANSWERED.to_string();
    e.reason = Some("challenge answered".to_string());
}

pub fn answer_challenges(events: &mut [Event]) {
    let got_through = events.iter().filter_map(through).collect::<BTreeSet<_>>();
    for e in events.iter_mut() {
        if challenge(e).is_some_and(|c| got_through.contains(&c)) {
            mark_answered(e);
        }
    }
}

fn rank(decision: &str) -> u8 {
    match decision {
        "deny" => 0,
        "learn" => 1,
        "fail" => 2,
        "inject" => 3,
        "pass" => 4,
        _ => 5,
    }
}

fn colored(decision: &str) -> ColoredString {
    let padded = format!("{:<6}", decision);
    match decision {
        "deny" => padded.red().bold(),
        "learn" => padded.yellow().bold(),
        "fail" => padded.magenta(),
        "inject" => padded.cyan(),
        "pass" => padded.green(),
        ANSWERED => padded.dimmed(),
        _ => padded.normal(),
    }
}

// what the proxy logged came from clients: only printable ASCII reaches the terminal, so nothing
// in it ends a line, reorders one or acts as an escape sequence
fn shown(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == ' ' || c.is_ascii_graphic() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[derive(Default)]
struct Group {
    count: usize,
    uas: BTreeSet<String>,
}

// Decisions alike - the same decision, target and rule or reason - counted together
pub fn summary(events: &[Event]) -> Vec<(String, String, String, usize, Vec<String>)> {
    let mut groups = BTreeMap::<(u8, String, String, String), Group>::new();
    for e in events {
        let g = groups
            .entry((
                rank(&e.decision),
                e.decision.clone(),
                e.target(),
                e.detail(),
            ))
            .or_default();
        g.count += 1;
        g.uas.insert(e.ua().to_string());
    }
    let mut rows = groups
        .into_iter()
        .map(|((rank, decision, target, detail), g)| {
            (
                rank,
                (
                    decision,
                    target,
                    detail,
                    g.count,
                    g.uas.into_iter().collect(),
                ),
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then(b.3.cmp(&a.3)).then(a.1.cmp(&b.1)));
    rows.into_iter().map(|(_, row)| row).collect()
}

// The hosts and ports a sidecar was refused or let through only by learning, as the policy that
// would allow them
pub fn suggestion(proxies: &[(String, Vec<Event>)]) -> Option<String> {
    let mut sidecars = IndexMap::new();
    for (proxy, events) in proxies {
        let hosts = events
            .iter()
            .filter_map(Event::wanted_host)
            .map(shown)
            .collect::<BTreeSet<_>>();
        if hosts.is_empty() {
            continue;
        }
        let ports = events
            .iter()
            .filter(|e| e.wanted_host().is_some())
            .filter_map(|e| e.port)
            .collect::<BTreeSet<_>>();
        let mut egress = IndexMap::new();
        egress.insert(
            "allow",
            serde_yaml::to_value(hosts.into_iter().collect::<Vec<_>>()).ok()?,
        );
        if ports.iter().any(|p| *p != 443) {
            let mut ports = ports;
            ports.insert(443);
            egress.insert(
                "ports",
                serde_yaml::to_value(ports.into_iter().collect::<Vec<_>>()).ok()?,
            );
        }
        let agent = proxy
            .strip_suffix(PROXY_SUFFIX)
            .unwrap_or(proxy)
            .to_string();
        sidecars.insert(agent, IndexMap::from([("egress", egress)]));
    }
    if sidecars.is_empty() {
        return None;
    }
    serde_yaml::to_string(&IndexMap::from([("sidecars", sidecars)])).ok()
}

type Lines<'s> = Pin<Box<dyn Stream<Item = Result<String, AnyError>> + Send + 's>>;

impl<'a> Api<'a> {
    // A workspace's egress proxies, by container name: the sidecars running `joey egress-proxy`
    async fn egress_proxies(
        &self,
        workspace_key: &str,
        sidecar: Option<&str>,
    ) -> Result<Vec<(String, String)>, AnyError> {
        let mut proxies = self
            .container
            .get_all(&Labels::from(&[Labels::workspace(workspace_key)]))
            .await?
            .into_iter()
            .filter(|c| {
                c.command
                    .as_deref()
                    .is_some_and(|cmd| cmd.contains("egress-proxy"))
            })
            .filter_map(|c| {
                let name = c.labels.as_ref()?.get(labels::CONTAINER)?.clone();
                Some((name, c.id?))
            })
            .filter(|(name, _)| {
                sidecar.is_none_or(|s| *name == s || *name == format!("{}{}", s, PROXY_SUFFIX))
            })
            .collect::<Vec<_>>();
        proxies.sort();
        if proxies.is_empty() {
            return Err(match sidecar {
                Some(s) => format!(
                    "Sidecar '{}' of workspace '{}' has no egress proxy: only a sidecar with an egress policy ('egress: {{allow: [...]}}') has one.",
                    s, workspace_key
                ),
                None => format!(
                    "Workspace '{}' has no egress proxy: a sidecar gets one with an egress policy ('egress: {{allow: [...]}}').",
                    workspace_key
                ),
            }
            .into());
        }
        Ok(proxies)
    }

    // A container's output, line by line
    fn log_lines<'s>(&'s self, id: &'s str, follow: bool, tail: &str) -> Lines<'s> {
        let stream = self
            .client
            .logs(
                id,
                Some(LogsOptions {
                    follow,
                    stdout: true,
                    stderr: true,
                    tail: tail.to_string(),
                    ..Default::default()
                }),
            )
            .scan(Vec::<u8>::new(), |buf, item| {
                let lines: Vec<Result<String, AnyError>> = match item {
                    Ok(chunk) => {
                        buf.extend_from_slice(&chunk.into_bytes());
                        let mut lines = vec![];
                        while let Some(end) = buf.iter().position(|b| *b == b'\n') {
                            let line = buf.drain(..=end).collect::<Vec<_>>();
                            lines.push(Ok(String::from_utf8_lossy(&line).trim_end().to_string()));
                        }
                        lines
                    }
                    Err(e) => vec![Err(e.into())],
                };
                futures::future::ready(Some(futures::stream::iter(lines)))
            })
            .flatten();
        Box::pin(stream)
    }

    pub async fn egress_log(
        &self,
        workspace_key: &str,
        sidecar: Option<&str>,
        denied: bool,
        suggest: bool,
        follow: bool,
        tail: Option<&str>,
    ) -> Result<(), AnyError> {
        let proxies = self.egress_proxies(workspace_key, sidecar).await?;
        let tail = tail.unwrap_or("all");
        let width = proxies.iter().map(|(n, _)| n.len()).max().unwrap_or(0);

        if follow {
            let streams = proxies.iter().map(|(name, id)| {
                self.log_lines(id, true, tail)
                    .map(move |line| line.map(|l| (name.as_str(), l)))
                    .boxed()
            });
            let mut all = futures::stream::select_all(streams);
            // as it happens, a challenge is answered once that client has got through
            let mut got_through = BTreeSet::new();
            while let Some(item) = all.next().await {
                let (name, line) = item?;
                let Some(mut e) = Event::parse(&line) else {
                    continue;
                };
                if let Some(t) = through(&e) {
                    got_through.insert((name.to_string(), t));
                }
                if challenge(&e).is_some_and(|c| got_through.contains(&(name.to_string(), c))) {
                    mark_answered(&mut e);
                }
                if denied && !e.denied() {
                    continue;
                }
                println!(
                    "{:<width$}  {}  {}  {}  {}",
                    name.dimmed(),
                    colored(&e.decision),
                    shown(&e.target()).bold(),
                    shown(&e.detail()),
                    format!("ua: {}", shown(e.ua())).dimmed(),
                    width = width
                );
            }
            return Ok(());
        }

        let mut read = vec![];
        for (name, id) in &proxies {
            let mut events = vec![];
            let mut unread = 0;
            let mut lines = self.log_lines(id, false, tail);
            while let Some(line) = lines.next().await {
                let line = line?;
                match Event::parse(&line) {
                    Some(e) => events.push(e),
                    None if line.trim().is_empty() || line.starts_with("joey ") => {}
                    None => unread += 1,
                }
            }
            if unread > 0 {
                eprintln!(
                    "{}: {} lines are not decisions rooz can read - a joey older than 0.175.0 logs text; 'rooz update {}' recreates the proxy",
                    name, unread, workspace_key
                );
            }
            answer_challenges(&mut events);
            if denied {
                events.retain(Event::denied);
            }
            read.push((name.clone(), events));
        }

        if suggest {
            match suggestion(&read) {
                Some(yaml) => {
                    eprintln!(
                        "{}",
                        "# hosts refused or let through only by learning - review before adding them"
                            .dimmed()
                    );
                    print!("{}", yaml);
                }
                None => eprintln!("Nothing was refused."),
            }
            return Ok(());
        }

        for (name, events) in &read {
            println!("{}", name.bold());
            let rows = summary(events);
            if rows.is_empty() {
                println!("  {}", "no decisions logged".dimmed());
                continue;
            }
            let target_width = rows
                .iter()
                .map(|r| shown(&r.1).chars().count())
                .max()
                .unwrap_or(0);
            let detail_width = rows
                .iter()
                .map(|r| shown(&r.2).chars().count())
                .max()
                .unwrap_or(0);
            for (decision, target, detail, count, uas) in rows {
                let uas = match uas.len() {
                    0..=2 => uas.join(", "),
                    n => format!("{}, {} +{}", uas[0], uas[1], n - 2),
                };
                println!(
                    "  {}  {}  {:>4}×  {:<dw$}  {}",
                    colored(&decision),
                    format!("{:<w$}", shown(&target), w = target_width).bold(),
                    count,
                    shown(&detail),
                    format!("ua: {}", shown(&uas)).dimmed(),
                    dw = detail_width,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(lines: &[&str]) -> Vec<Event> {
        lines.iter().filter_map(|l| Event::parse(l)).collect()
    }

    #[test]
    fn decisions_alike_are_counted_together_refusals_first() {
        let rows = summary(&events(&[
            r#"{"decision":"pass","host":"github.com","port":443,"ua":"curl/8"}"#,
            r#"{"decision":"deny","host":"x.example","port":443,"reason":"not allowed","ua":"a"}"#,
            r#"{"decision":"deny","host":"x.example","port":443,"reason":"not allowed","ua":"b"}"#,
            r#"{"decision":"inject","host":"api.example","port":443,"method":"POST","path":"/v1/m","rule":1}"#,
            r#"{"decision":"deny","method":"CONNECT","host":"x.example","port":443,"reason":"no proxy credentials"}"#,
            "joey egress-proxy listening on 0.0.0.0:9124",
            "deny old text line",
        ]));
        let shape = rows
            .iter()
            .map(|(d, t, r, n, uas)| format!("{} {} [{}] {} {}", d, t, r, n, uas.join(",")))
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            [
                "deny x.example:443 [not allowed] 2 a,b",
                "deny x.example:443 [no proxy credentials] 1 -",
                "inject api.example POST /v1/m [rule 1] 1 -",
                "pass github.com:443 [] 1 curl/8",
            ]
        );
    }

    #[test]
    fn a_credential_challenge_the_client_answered_is_not_a_refusal() {
        let mut e = events(&[
            r#"{"decision":"deny","method":"CONNECT","host":"nuget.pkg.github.com","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
            r#"{"decision":"inject","host":"nuget.pkg.github.com","port":443,"method":"GET","path":"/o/index.json","rule":1,"ua":"NuGet"}"#,
            r#"{"decision":"deny","method":"CONNECT","host":"nuget.pkg.github.com","port":443,"reason":"no proxy credentials","ua":"curl/8"}"#,
            r#"{"decision":"deny","method":"CONNECT","host":"api.nuget.org","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
        ]);
        answer_challenges(&mut e);
        let shape = summary(&e)
            .iter()
            .map(|(d, t, r, n, uas)| format!("{} {} [{}] {} {}", d, t, r, n, uas.join(",")))
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            [
                "deny api.nuget.org:443 [no proxy credentials] 1 NuGet",
                "deny nuget.pkg.github.com:443 [no proxy credentials] 1 curl/8",
                "inject nuget.pkg.github.com GET /o/index.json [rule 1] 1 NuGet",
                "auth nuget.pkg.github.com:443 [challenge answered] 1 NuGet",
            ]
        );
        assert!(e.iter().filter(|e| e.denied()).count() == 2);
    }

    #[test]
    fn the_suggestion_lists_what_the_policy_refused_or_only_learned() {
        let proxies = vec![
            (
                "claude-egress".to_string(),
                events(&[
                    r#"{"decision":"deny","host":"b.example","port":443,"reason":"not allowed"}"#,
                    r#"{"decision":"learn","host":"a.example","port":8443}"#,
                    r#"{"decision":"deny","host":"c.example","port":443,"reason":"no proxy credentials"}"#,
                    r#"{"decision":"pass","host":"github.com","port":443}"#,
                ]),
            ),
            (
                "dkr-egress".to_string(),
                events(&[r#"{"decision":"pass","host":"ghcr.io","port":443}"#]),
            ),
        ];
        assert_eq!(
            suggestion(&proxies).unwrap(),
            "sidecars:\n  claude:\n    egress:\n      allow:\n      - a.example\n      - b.example\n      ports:\n      - 443\n      - 8443\n"
        );
        assert_eq!(suggestion(&proxies[1..]), None);
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        assert_eq!(shown("a\u{1b}[31mb\nc\u{202e}d"), "a?[31mb?c?d");
    }
}
