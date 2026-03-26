#![allow(dead_code)]

use std::{io::IsTerminal, time::Duration};

use cpm_core::fetcher::{DownloadProgress, DownloadProgressHandle};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMode {
    Rich,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Install,
    Remove,
    Update,
}

impl OperationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
            Self::Update => "update",
        }
    }

    fn present_participle(self) -> &'static str {
        match self {
            Self::Install => "Installing",
            Self::Remove => "Removing",
            Self::Update => "Updating",
        }
    }

    fn past_tense(self) -> &'static str {
        match self {
            Self::Install => "Installed",
            Self::Remove => "Removed",
            Self::Update => "Updated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl OperationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug)]
pub struct ProgressReporter {
    mode: ProgressMode,
    rich: Option<MultiProgress>,
}

impl ProgressReporter {
    pub fn auto() -> Self {
        Self::with_mode(if env_allows_rich_output() {
            ProgressMode::Rich
        } else {
            ProgressMode::Plain
        })
    }

    pub fn with_mode(mode: ProgressMode) -> Self {
        Self {
            mode,
            rich: matches!(mode, ProgressMode::Rich).then(MultiProgress::new),
        }
    }

    pub fn mode(&self) -> ProgressMode {
        self.mode
    }

    pub fn begin_operation(
        &self,
        kind: OperationKind,
        subject: impl Into<String>,
    ) -> OperationHandle {
        let subject = subject.into();
        let status = OperationStatus::Pending;

        match (&self.mode, &self.rich) {
            (ProgressMode::Rich, Some(multi)) => {
                let bar = multi.add(ProgressBar::new_spinner());
                bar.set_style(rich_spinner_style());
                bar.enable_steady_tick(Duration::from_millis(80));
                bar.set_prefix(kind.as_str().to_owned());
                bar.set_message(render_rich_message(kind, status, &subject));

                OperationHandle {
                    kind,
                    subject,
                    status,
                    rich: Some(bar),
                }
            }
            _ => {
                eprintln!("{}", plain_status_line(kind, &subject, status));

                OperationHandle {
                    kind,
                    subject,
                    status,
                    rich: None,
                }
            }
        }
    }

    fn begin_download(
        &self,
        url: &str,
        total_bytes: Option<u64>,
    ) -> Box<dyn DownloadProgressHandle> {
        let label = download_label(url);
        match (&self.mode, &self.rich, total_bytes) {
            (ProgressMode::Rich, Some(multi), Some(total)) => {
                let bar = multi.add(ProgressBar::new(total));
                bar.set_style(rich_download_style());
                bar.set_prefix("download".to_owned());
                bar.set_message(label.clone());
                Box::new(RichDownloadHandle {
                    bar,
                    clear_on_finish: true,
                    subject: label,
                })
            }
            (ProgressMode::Rich, Some(multi), None) => {
                let bar = multi.add(ProgressBar::new_spinner());
                bar.set_style(rich_spinner_style());
                bar.enable_steady_tick(Duration::from_millis(80));
                bar.set_prefix("download".to_owned());
                bar.set_message(label.clone());
                Box::new(RichDownloadHandle {
                    bar,
                    clear_on_finish: true,
                    subject: label,
                })
            }
            _ => {
                eprintln!(
                    "{}",
                    plain_download_line("started", &label, 0, total_bytes, None)
                );
                Box::new(PlainDownloadHandle {
                    subject: label,
                    total_bytes,
                    downloaded_bytes: 0,
                })
            }
        }
    }
}

impl DownloadProgress for ProgressReporter {
    fn begin(&self, url: &str, total_bytes: Option<u64>) -> Box<dyn DownloadProgressHandle> {
        self.begin_download(url, total_bytes)
    }
}

#[derive(Debug)]
pub struct OperationHandle {
    kind: OperationKind,
    subject: String,
    status: OperationStatus,
    rich: Option<ProgressBar>,
}

#[derive(Debug)]
struct RichDownloadHandle {
    bar: ProgressBar,
    clear_on_finish: bool,
    subject: String,
}

impl DownloadProgressHandle for RichDownloadHandle {
    fn advance(&mut self, delta: u64) {
        self.bar.inc(delta);
    }

    fn finish(&mut self) {
        if self.clear_on_finish {
            self.bar.finish_and_clear();
        } else {
            self.bar.finish();
        }
    }

    fn fail(&mut self) {
        self.bar.abandon_with_message(format!("✖ {}", self.subject));
    }
}

#[derive(Debug)]
struct PlainDownloadHandle {
    subject: String,
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
}

impl DownloadProgressHandle for PlainDownloadHandle {
    fn advance(&mut self, delta: u64) {
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(delta);
    }

    fn finish(&mut self) {
        eprintln!(
            "{}",
            plain_download_line(
                "finished",
                &self.subject,
                self.downloaded_bytes,
                self.total_bytes,
                Some("succeeded"),
            )
        );
    }

    fn fail(&mut self) {
        eprintln!(
            "{}",
            plain_download_line(
                "finished",
                &self.subject,
                self.downloaded_bytes,
                self.total_bytes,
                Some("failed"),
            )
        );
    }
}

impl OperationHandle {
    pub fn status(&self) -> OperationStatus {
        self.status
    }

    pub fn set_status(&mut self, status: OperationStatus) {
        self.status = status;

        match self.rich.as_ref() {
            Some(bar) => {
                bar.set_prefix(self.kind.as_str().to_owned());
                bar.set_message(render_rich_message(self.kind, status, &self.subject));
                if matches!(
                    status,
                    OperationStatus::Succeeded | OperationStatus::Failed | OperationStatus::Skipped
                ) {
                    bar.finish_with_message(render_rich_message(self.kind, status, &self.subject));
                }
            }
            None => {
                eprintln!("{}", plain_status_line(self.kind, &self.subject, status));
            }
        }
    }

    pub fn finish(mut self, status: OperationStatus) {
        self.set_status(status);
    }
}

pub fn should_use_rich_output(is_terminal: bool, no_color: bool, ci: bool) -> bool {
    is_terminal && !no_color && !ci
}

pub fn plain_status_line(kind: OperationKind, subject: &str, status: OperationStatus) -> String {
    format!(
        "ts={} cpm-progress operation={} subject={} status={}",
        unix_timestamp(),
        kind.as_str(),
        quote_field(subject),
        status.as_str(),
    )
}

fn render_rich_message(kind: OperationKind, status: OperationStatus, subject: &str) -> String {
    match status {
        OperationStatus::Pending => format!("• {} {subject}", kind.present_participle()),
        OperationStatus::Running => format!("→ {} {subject}", kind.present_participle()),
        OperationStatus::Succeeded => format!("✓ {} {subject}", kind.past_tense()),
        OperationStatus::Failed => format!("✖ Failed {subject}"),
        OperationStatus::Skipped => format!("↷ Skipped {subject}"),
    }
}

fn quote_field(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':'))
    {
        value.to_owned()
    } else {
        format!("{value:?}")
    }
}

fn download_label(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
                .map(ToOwned::to_owned)
        })
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| url.to_owned())
}

fn plain_download_line(
    phase: &str,
    subject: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    outcome: Option<&str>,
) -> String {
    let mut line = format!(
        "ts={} cpm-progress kind=download phase={} subject={} downloaded_bytes={}",
        unix_timestamp(),
        phase,
        quote_field(subject),
        downloaded_bytes,
    );
    if let Some(total_bytes) = total_bytes {
        line.push_str(&format!(" total_bytes={total_bytes}"));
    }
    if let Some(outcome) = outcome {
        line.push_str(&format!(" outcome={outcome}"));
    }
    line
}

fn rich_spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {prefix:>8.bold} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
}

fn rich_download_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.cyan} {prefix:>8.bold} [{bar:24.cyan/blue}] {percent:>3}% {bytes}/{total_bytes} {msg}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("=>-")
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn env_allows_rich_output() -> bool {
    if let Some(force) = std::env::var_os("CPM_PROGRESS") {
        match force.to_string_lossy().to_ascii_lowercase().as_str() {
            "rich" => return true,
            "plain" => return false,
            _ => {}
        }
    }

    let stderr_is_terminal = std::io::stderr().is_terminal();
    let stdout_is_terminal = std::io::stdout().is_terminal();
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let ci = std::env::var_os("CI").is_some();

    should_use_rich_output(stderr_is_terminal || stdout_is_terminal, no_color, ci)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_output_requires_terminal_and_no_ci_or_no_color() {
        assert!(should_use_rich_output(true, false, false));
        assert!(!should_use_rich_output(false, false, false));
        assert!(!should_use_rich_output(true, true, false));
        assert!(!should_use_rich_output(true, false, true));
    }

    #[test]
    fn plain_status_line_is_structured_and_ansi_free() {
        let line = plain_status_line(
            OperationKind::Install,
            "plugin:foo",
            OperationStatus::Running,
        );

        assert!(line.contains("cpm-progress operation=install subject=plugin:foo status=running"));
        assert!(!line.contains('\u{1b}'));
    }

    #[test]
    fn plain_status_line_quotes_fields_with_spaces() {
        let line = plain_status_line(
            OperationKind::Update,
            "copilot plugin",
            OperationStatus::Pending,
        );

        assert!(line.contains(r#"subject="copilot plugin""#));
    }

    #[test]
    fn plain_download_line_is_structured() {
        let line = plain_download_line(
            "finished",
            "bundle.tar.gz",
            128,
            Some(256),
            Some("succeeded"),
        );

        assert!(line.contains("cpm-progress kind=download phase=finished"));
        assert!(line.contains("downloaded_bytes=128"));
        assert!(line.contains("total_bytes=256"));
        assert!(line.contains("outcome=succeeded"));
        assert!(!line.contains('\u{1b}'));
    }

    #[test]
    fn rich_message_uses_operation_verbs() {
        assert_eq!(
            render_rich_message(
                OperationKind::Install,
                OperationStatus::Running,
                "skill:tracked"
            ),
            "→ Installing skill:tracked"
        );
        assert_eq!(
            render_rich_message(
                OperationKind::Remove,
                OperationStatus::Succeeded,
                "plugin:legacy"
            ),
            "✓ Removed plugin:legacy"
        );
    }

    #[test]
    fn download_label_prefers_file_name() {
        assert_eq!(
            download_label("https://raw.githubusercontent.com/owner/repo/rev/path/to/file.txt"),
            "file.txt"
        );
        assert_eq!(download_label("not a url"), "not a url");
    }
}
