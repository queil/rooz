// Install progress of containers installing side by side. On a terminal each container gets a
// pane: a header with its current step and elapsed time, then the last lines of its output,
// cut to the terminal width. A finished container collapses to one summary line; a failed one
// leaves its failing step's whole output behind. Anywhere else, and with ROOZ_PROGRESS=plain,
// every line is printed as it comes, tagged with its container and step.

use crate::util::progress;
use colored::Colorize;
use crossterm::{QueueableCommand, cursor, terminal};
use indexmap::IndexMap;
use std::collections::VecDeque;
use std::io::{IsTerminal, Write, stdout};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PROGRESS_ENV: &str = "ROOZ_PROGRESS";
const MAX_PANE_LINES: usize = 5;
// a step's output is kept for its failure report, but not without limit: a verbose or runaway
// step must not grow the process's memory with everything it printed
const MAX_STEP_LINES: usize = 1000;
const REDRAW_EVERY: Duration = Duration::from_millis(80);

struct Pane {
    step: Option<String>,
    started: Instant,
    steps_done: usize,
    tail: VecDeque<String>,
    step_output: Vec<String>,
}

struct View {
    panes: IndexMap<String, Pane>,
    drawn: u16,
    last_draw: Option<Instant>,
}

static VIEW: Mutex<Option<View>> = Mutex::new(None);

fn wanted() -> bool {
    stdout().is_terminal() && std::env::var(PROGRESS_ENV).map_or(true, |v| v.trim() != "plain")
}

/// Starts the live view if stdout is a terminal. Pair with `end`.
pub fn begin() {
    if !wanted() {
        return;
    }
    *VIEW.lock().unwrap() = Some(View {
        panes: IndexMap::new(),
        drawn: 0,
        last_draw: None,
    });
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match VIEW.lock().unwrap().as_mut() {
                Some(view) => view.draw(),
                None => break,
            }
        }
    });
}

/// Ends the live view. Containers still running were cut short by another one's failure.
pub fn end() {
    if let Some(mut view) = VIEW.lock().unwrap().take() {
        let names = view.panes.keys().cloned().collect::<Vec<_>>();
        for name in names {
            let pane = view.panes.shift_remove(&name).unwrap();
            let line = format!(
                "{} {} · stopped{} · {}",
                "–".dimmed(),
                progress::host(&name),
                pane.step.map(|s| format!(" at {}", s)).unwrap_or_default(),
                elapsed(pane.started)
            );
            view.print_above(&[line]);
        }
        view.clear();
    }
}

pub fn is_live() -> bool {
    VIEW.lock().unwrap().is_some()
}

/// A line that belongs above the panes, e.g. a phase line or an image pull.
pub fn println(line: impl std::fmt::Display) {
    match VIEW.lock().unwrap().as_mut() {
        Some(view) => view.print_above(&[line.to_string()]),
        None => println!("{}", line),
    }
}

pub fn install_begin(container: &str, steps: &[&str]) {
    match VIEW.lock().unwrap().as_mut() {
        Some(view) => {
            view.panes.insert(
                container.to_string(),
                Pane {
                    step: None,
                    started: Instant::now(),
                    steps_done: 0,
                    tail: VecDeque::new(),
                    step_output: Vec::new(),
                },
            );
            view.draw();
        }
        None => println!(
            "{} steps: {}",
            progress::install_tag(container, None),
            steps.join(", ")
        ),
    }
}

// the step whose skip_if check runs: shown in its pane, while plain output waits for the outcome
pub fn install_checking(container: &str, step: &str) {
    with_pane(
        container,
        |pane| {
            pane.step = Some(step.to_string());
            pane.step_output.clear();
        },
        || (),
    );
}

pub fn install_step(container: &str, step: &str) {
    with_pane(
        container,
        |pane| {
            pane.step = Some(step.to_string());
            pane.step_output.clear();
        },
        || println!("{}", progress::install_tag(container, Some(step))),
    );
}

pub fn install_skipped(container: &str, step: &str) {
    with_pane(
        container,
        |pane| {
            pane.steps_done += 1;
            pane.push(format!("{}: skipped, skip_if succeeded", step));
        },
        || {
            println!(
                "{} skipped: skip_if succeeded",
                progress::install_tag(container, Some(step))
            )
        },
    );
}

pub fn install_step_done(container: &str) {
    with_pane(container, |pane| pane.steps_done += 1, || ());
}

pub fn install_output(container: &str, step: &str, line: &str) {
    with_pane(
        container,
        |pane| pane.push(clean(line)),
        || println!("{} {}", progress::install_tag(container, Some(step)), line),
    );
}

pub fn install_done(container: &str) {
    if let Some(view) = VIEW.lock().unwrap().as_mut()
        && let Some(pane) = view.panes.shift_remove(container)
    {
        let line = format!(
            "{} {} · {} step{} · {}",
            "✓".green(),
            progress::host(container),
            pane.steps_done,
            if pane.steps_done == 1 { "" } else { "s" },
            elapsed(pane.started)
        );
        view.print_above(&[line]);
    }
}

pub fn install_failed(container: &str) {
    if let Some(view) = VIEW.lock().unwrap().as_mut()
        && let Some(pane) = view.panes.shift_remove(container)
    {
        let step = pane.step.unwrap_or_default();
        let mut lines = vec![format!(
            "{} {} · {} failed · {}",
            "✗".red(),
            progress::host(container),
            step,
            elapsed(pane.started)
        )];
        if pane.step_output.len() >= MAX_STEP_LINES {
            lines.push(format!(
                "{} … earlier output omitted",
                progress::install_tag(container, Some(&step))
            ));
        }
        lines.extend(
            pane.step_output
                .iter()
                .map(|l| format!("{} {}", progress::install_tag(container, Some(&step)), l)),
        );
        view.print_above(&lines);
    }
}

fn with_pane(container: &str, live: impl FnOnce(&mut Pane), plain: impl FnOnce()) {
    match VIEW.lock().unwrap().as_mut() {
        Some(view) => {
            if let Some(pane) = view.panes.get_mut(container) {
                live(pane);
            }
            view.draw_throttled();
        }
        None => plain(),
    }
}

impl Pane {
    fn push(&mut self, line: String) {
        self.step_output.push(line.clone());
        // trimmed in batches, so the cost stays amortised over the lines pushed
        if self.step_output.len() >= MAX_STEP_LINES * 2 {
            let excess = self.step_output.len() - MAX_STEP_LINES;
            self.step_output.drain(..excess);
        }
        self.tail.push_back(line);
        while self.tail.len() > MAX_PANE_LINES {
            self.tail.pop_front();
        }
    }
}

impl View {
    fn lines(&self) -> Vec<String> {
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let width = width.saturating_sub(1) as usize;
        let panes = self.panes.len().max(1);
        // headers always fit; output lines share what is left of the screen
        let per_pane = (height as usize)
            .saturating_sub(2 + panes)
            .checked_div(panes)
            .unwrap_or(0)
            .min(MAX_PANE_LINES);
        let mut lines = Vec::new();
        for (name, pane) in &self.panes {
            let left = format!(
                "── {} · {} ",
                name,
                pane.step.as_deref().unwrap_or("starting")
            );
            let right = format!(" {}", elapsed(pane.started));
            let fill = width.saturating_sub(left.chars().count() + right.chars().count());
            lines.push(format!(
                "── {} · {} {}{}",
                progress::host(name),
                pane.step.as_deref().unwrap_or("starting"),
                "─".repeat(fill).dimmed(),
                right.dimmed()
            ));
            let skip = pane.tail.len().saturating_sub(per_pane);
            for line in pane.tail.iter().skip(skip) {
                lines.push(
                    format!("  {}", truncate(line, width.saturating_sub(2)))
                        .dimmed()
                        .to_string(),
                );
            }
        }
        lines
    }

    fn clear(&mut self) {
        let mut out = stdout().lock();
        if self.drawn > 0 {
            let _ = out.queue(cursor::MoveUp(self.drawn));
        }
        let _ = out.queue(cursor::MoveToColumn(0));
        let _ = out.queue(terminal::Clear(terminal::ClearType::FromCursorDown));
        let _ = out.flush();
        self.drawn = 0;
    }

    fn draw(&mut self) {
        let lines = self.lines();
        self.clear();
        let mut out = stdout().lock();
        for line in &lines {
            let _ = writeln!(out, "{}", line);
        }
        let _ = out.flush();
        self.drawn = lines.len() as u16;
        self.last_draw = Some(Instant::now());
    }

    fn draw_throttled(&mut self) {
        if self.last_draw.is_none_or(|t| t.elapsed() >= REDRAW_EVERY) {
            self.draw();
        }
    }

    fn print_above(&mut self, lines: &[String]) {
        self.clear();
        {
            let mut out = stdout().lock();
            for line in lines {
                let _ = writeln!(out, "{}", line);
            }
        }
        self.draw();
    }
}

fn elapsed(since: Instant) -> String {
    format!("{}s", since.elapsed().as_secs())
}

// Output as a terminal would leave it: what follows the last carriage return, without escape
// sequences or other control characters that would move the cursor out of the pane.
fn clean(line: &str) -> String {
    let line = line.trim_end_matches(['\n', '\r']);
    let line = line.rsplit('\r').next().unwrap_or(line);
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => (),
            c => out.push(c),
        }
    }
    out
}

fn truncate(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        line.to_string()
    } else {
        let mut cut = line
            .chars()
            .take(width.saturating_sub(1))
            .collect::<String>();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_cleaned_to_what_a_terminal_shows() {
        assert_eq!(clean("10%\r50%\r100% done\n"), "100% done");
        assert_eq!(clean("\x1b[32mok\x1b[0m"), "ok");
        assert_eq!(clean("a\tb\x07"), "a    b");
    }

    #[test]
    fn a_step_keeps_a_bounded_amount_of_output() {
        let mut pane = Pane {
            step: None,
            started: Instant::now(),
            steps_done: 0,
            tail: VecDeque::new(),
            step_output: Vec::new(),
        };
        for i in 0..MAX_STEP_LINES * 10 {
            pane.push(format!("line {}", i));
        }
        assert!(pane.step_output.len() < MAX_STEP_LINES * 2);
        assert!(pane.step_output.len() >= MAX_STEP_LINES);
        let last = format!("line {}", MAX_STEP_LINES * 10 - 1);
        assert_eq!(pane.step_output.last(), Some(&last));
        assert_eq!(pane.tail.len(), MAX_PANE_LINES);
    }

    #[test]
    fn long_lines_are_cut_to_the_width() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
