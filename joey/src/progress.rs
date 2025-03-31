use gix::progress::{Id, MessageLevel, Step, StepShared, Unit};
use gix::{Count, NestedProgress, Progress};
use indicatif::{self, MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

pub struct IndicatifProgress {
    multi_progress: Arc<MultiProgress>,
    progress_bar: Option<ProgressBar>,
    children: HashMap<Id, ProgressBar>,
    id: Id,
    name: Option<String>,
}

impl IndicatifProgress {
    pub fn new() -> Self {
        Self {
            multi_progress: Arc::new(MultiProgress::new()),
            progress_bar: None,
            children: HashMap::new(),
            id: gix::progress::UNKNOWN,
            name: None,
        }
    }
}

impl Count for IndicatifProgress {
    fn set(&self, step: usize) {
        if let Some(pb) = &self.progress_bar {
            pb.set_position(step as u64);
        }
    }

    fn step(&self) -> usize {
        self.progress_bar
            .as_ref()
            .map(|pb| pb.position() as usize)
            .unwrap_or(0)
    }

    fn inc_by(&self, step: usize) {
        if let Some(pb) = &self.progress_bar {
            pb.inc(step as u64);
        }
    }

    fn counter(&self) -> StepShared {
        Arc::new(AtomicUsize::default())
    }
}

impl Progress for IndicatifProgress {
    fn init(&mut self, max: Option<usize>, unit: Option<Unit>) {
        let pb = self
            .multi_progress
            .add(ProgressBar::new(max.unwrap_or(0) as u64));

        let style = match unit {
            Some(_) => ProgressStyle::default_bar()
                .template("⠁ {msg}: [{bar:20}] {bytes}/{total_bytes}")
                .unwrap()
                .progress_chars("█░"),
            _ => ProgressStyle::default_bar()
                .template("⠁ {msg}: [{bar:20}] {pos}/{len}")
                .unwrap()
                .progress_chars("█░"),
        };

        pb.set_style(style);
        if let Some(name) = &self.name {
            pb.set_message(name.clone());
        }
        self.progress_bar = Some(pb);
    }

    fn set_max(&mut self, max: Option<Step>) -> Option<Step> {
        if let Some(pb) = &self.progress_bar {
            let old_max = pb.length().map(|l| l as usize);
            pb.set_length(max.unwrap_or(0) as u64);
            old_max
        } else {
            None
        }
    }

    fn set_name(&mut self, name: String) {
        self.name = Some(name.clone());
        if let Some(pb) = &self.progress_bar {
            pb.set_message(name);
        }
    }

    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn id(&self) -> Id {
        self.id
    }

    fn message(&self, level: MessageLevel, message: String) {
        if let Some(pb) = &self.progress_bar {
            match level {
                MessageLevel::Info => pb.println(message),
                MessageLevel::Failure => pb.println(format!("Error: {}", message)),
                _ => pb.println(message),
            }
        }
    }
}

impl NestedProgress for IndicatifProgress {
    type SubProgress = Self;

    fn add_child(&mut self, name: impl Into<String>) -> Self {
        let id = gix::progress::Id::from([
            rand::random::<u8>(),
            rand::random::<u8>(),
            rand::random::<u8>(),
            rand::random::<u8>(),
        ]);
        self.add_child_with_id(name, id)
    }

    fn add_child_with_id(&mut self, name: impl Into<String>, id: Id) -> Self {
        let name_str = name.into();

        let child_pb = self.multi_progress.add(ProgressBar::new(0));
        child_pb.set_style(
            ProgressStyle::default_bar()
                .template("⠁ {msg}: [{bar:20}] {pos}/{len}")
                .unwrap()
                .progress_chars("█░"),
        );
        child_pb.set_message(name_str.clone());

        self.children.insert(id, child_pb.clone());

        Self {
            multi_progress: self.multi_progress.clone(),
            progress_bar: Some(child_pb),
            children: HashMap::new(),
            id,
            name: Some(name_str),
        }
    }
}

impl Drop for IndicatifProgress {
    fn drop(&mut self) {
        if let Some(pb) = &self.progress_bar {
            pb.finish_and_clear();
        }

        for (_, pb) in self.children.drain() {
            pb.finish_and_clear();
        }
    }
}
