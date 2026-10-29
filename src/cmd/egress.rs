use std::{
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
};

use bollard::query_parameters::LogsOptions;
use colored::{ColoredString, Colorize};
use futures::{Stream, StreamExt};
use indexmap::IndexMap;
use jiff::{SignedDuration, Timestamp};
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
    // the client's address as the proxy saw it on the network
    pub peer: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub rule: Option<usize>,
    pub reason: Option<String>,
    pub ua: Option<String>,
    // when the container runtime received the line, not part of joey's JSON
    #[serde(skip)]
    pub at: Option<Timestamp>,
}

const PROXY_SUFFIX: &str = "-egress";

impl Event {
    pub fn parse(line: &str) -> Option<Event> {
        serde_json::from_str(line.trim()).ok()
    }

    // a log line as the runtime gives it with timestamps: the time it received it, then the line
    pub fn read(line: &str) -> Option<Event> {
        let (at, line) = stamped(line);
        Event::parse(line).map(|e| Event { at, ..e })
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
// how soon after its challenge a client gets through with the credential: .NET asks again at once
const ANSWER_WITHIN: SignedDuration = SignedDuration::from_secs(5);

// The runtime's timestamp a line starts with, and the rest of it
fn stamped(line: &str) -> (Option<Timestamp>, &str) {
    match line.split_once(' ') {
        Some((at, rest)) => match at.parse::<Timestamp>() {
            Ok(at) => (Some(at), rest),
            Err(_) => (None, line),
        },
        None => (None, line),
    }
}

// Some clients (.NET's HttpClient) send the proxy credential only once challenged: a CONNECT
// without it, refused, then the same again with it. A refusal for a missing credential is that
// challenge, answered, when the same client address - the network's, not the client's own claim -
// gets through to the same host within moments after it, each success answering one refusal: a
// client that never sends the credential is not hidden by another one that does. A wrong
// credential is never one.
fn through(e: &Event) -> Option<(String, String)> {
    matches!(e.decision.as_str(), "pass" | "inject" | "learn")
        .then(|| Some((e.peer.clone()?, e.host.clone()?)))
        .flatten()
}

fn challenge(e: &Event) -> Option<(String, String)> {
    (e.decision == "deny" && e.reason.as_deref() == Some(NO_CREDENTIALS))
        .then(|| Some((e.peer.clone()?, e.host.clone()?)))
        .flatten()
}

pub fn answer_challenges(events: &mut [Event]) {
    // when what got through later and answered nothing yet did, walking back from the end: latest first
    let mut later = BTreeMap::<(String, String), Vec<Timestamp>>::new();
    for e in events.iter_mut().rev() {
        let Some(at) = e.at else {
            continue;
        };
        if let Some(t) = through(e) {
            later.entry(t).or_default().push(at);
        } else if let Some(times) = challenge(e).and_then(|c| later.get_mut(&c)) {
            // too long after this challenge is too long after any before it
            times.retain(|t| t.duration_since(at) <= ANSWER_WITHIN);
            if times.pop().is_some() {
                e.decision = ANSWERED.to_string();
                e.reason = Some("challenge answered".to_string());
            }
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

    // A container's output, line by line, each after the time the runtime received it
    fn log_lines<'s>(&'s self, id: &'s str, follow: bool, tail: &str) -> Lines<'s> {
        let stream = self
            .client
            .logs(
                id,
                Some(LogsOptions {
                    follow,
                    stdout: true,
                    stderr: true,
                    timestamps: true,
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
            // as it happens, what follows a challenge is not known yet: it shows as the refusal it is
            while let Some(item) = all.next().await {
                let (name, line) = item?;
                let Some(e) = Event::read(&line) else {
                    continue;
                };
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
                let (at, text) = stamped(&line);
                match Event::parse(text) {
                    Some(e) => events.push(Event { at, ..e }),
                    None if text.trim().is_empty() || text.starts_with("joey ") => {}
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
        lines.iter().filter_map(|l| Event::read(l)).collect()
    }

    fn decisions(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| format!("{} {}", e.decision, e.detail()))
            .collect()
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
    fn a_challenge_is_answered_only_by_the_same_address_getting_through_after_it() {
        let mut e = events(&[
            // answered: the same address gets through to the host afterwards
            r#"2026-10-29T10:00:00.100Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"nuget.pkg.github.com","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
            r#"2026-10-29T10:00:00.200Z {"decision":"inject","peer":"10.0.0.2","host":"nuget.pkg.github.com","port":443,"method":"GET","path":"/o/index.json","rule":1,"ua":"NuGet"}"#,
            // another address copying the User-Agent stays refused
            r#"2026-10-29T10:00:00.300Z {"decision":"deny","peer":"10.0.0.3","method":"CONNECT","host":"nuget.pkg.github.com","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
            // a wrong credential stays refused
            r#"2026-10-29T10:00:00.400Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"nuget.pkg.github.com","port":443,"reason":"wrong proxy credentials","ua":"NuGet"}"#,
            // a success only before a refusal does not answer it
            r#"2026-10-29T10:00:00.500Z {"decision":"pass","peer":"10.0.0.2","host":"api.nuget.org","port":443,"ua":"NuGet"}"#,
            r#"2026-10-29T10:00:00.600Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"api.nuget.org","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
            // nor does one without an address (an older joey)
            r#"2026-10-29T10:00:00.700Z {"decision":"deny","method":"CONNECT","host":"x.example","port":443,"reason":"no proxy credentials","ua":"NuGet"}"#,
            r#"2026-10-29T10:00:00.800Z {"decision":"pass","host":"x.example","port":443,"ua":"NuGet"}"#,
            // nor one without the time it was logged
            r#"{"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"y.example","port":443,"reason":"no proxy credentials"}"#,
            r#"{"decision":"pass","peer":"10.0.0.2","host":"y.example","port":443}"#,
        ]);
        answer_challenges(&mut e);
        assert_eq!(
            decisions(&e),
            [
                "auth challenge answered",
                "inject rule 1",
                "deny no proxy credentials",
                "deny wrong proxy credentials",
                "pass ",
                "deny no proxy credentials",
                "deny no proxy credentials",
                "pass ",
                "deny no proxy credentials",
                "pass ",
            ]
        );
        assert_eq!(e.iter().filter(|e| e.denied()).count(), 5);
    }

    #[test]
    fn a_client_that_never_sends_the_credential_is_not_hidden_by_one_that_does() {
        // npm without the credential, then curl with it from the same container
        let mut e = events(&[
            r#"2026-10-29T10:00:00.000Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"registry.npmjs.org","port":443,"reason":"no proxy credentials"}"#,
            r#"2026-10-29T10:00:01.000Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"registry.npmjs.org","port":443,"reason":"no proxy credentials"}"#,
            r#"2026-10-29T10:00:02.000Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"registry.npmjs.org","port":443,"reason":"no proxy credentials"}"#,
            // one success answers one challenge, the latest before it
            r#"2026-10-29T10:00:03.000Z {"decision":"pass","peer":"10.0.0.2","host":"registry.npmjs.org","port":443,"ua":"curl/8"}"#,
            // and none long before it
            r#"2026-10-29T10:01:00.000Z {"decision":"deny","peer":"10.0.0.2","method":"CONNECT","host":"registry.npmjs.org","port":443,"reason":"no proxy credentials"}"#,
            r#"2026-10-29T10:05:00.000Z {"decision":"pass","peer":"10.0.0.2","host":"registry.npmjs.org","port":443,"ua":"curl/8"}"#,
        ]);
        answer_challenges(&mut e);
        assert_eq!(
            decisions(&e),
            [
                "deny no proxy credentials",
                "deny no proxy credentials",
                "auth challenge answered",
                "pass ",
                "deny no proxy credentials",
                "pass ",
            ]
        );
    }

    #[test]
    fn a_line_starts_with_the_time_the_runtime_received_it() {
        let e = Event::read(r#"2026-10-29T10:00:00.123456789Z {"decision":"pass"}"#).unwrap();
        assert_eq!(
            e.at,
            Some("2026-10-29T10:00:00.123456789Z".parse().unwrap())
        );
        assert_eq!(Event::read(r#"{"decision":"pass"}"#).unwrap().at, None);
        assert_eq!(
            stamped("joey egress-proxy listening").1,
            "joey egress-proxy listening"
        );
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
