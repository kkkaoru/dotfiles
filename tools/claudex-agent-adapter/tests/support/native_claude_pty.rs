use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const REQUIRED_CLAUDE_CODE_VERSION: &str = "2.1.220";
const OPT_IN_ENV: &str = "CLAUDEX_RUN_NATIVE_AGENT_UI";
const RESUME_ID_ENV: &str = "CLAUDEX_NATIVE_AGENT_RESUME_ID";
const WORKDIR_ENV: &str = "CLAUDEX_NATIVE_AGENT_WORKDIR";
const CLAUDEX_PROGRAM_ENV: &str = "CLAUDEX_NATIVE_AGENT_COMMAND";
const CLAUDE_PROGRAM_ENV: &str = "CLAUDEX_NATIVE_CLAUDE_COMMAND";
const ARTIFACT_DIR_ENV: &str = "CLAUDEX_NATIVE_AGENT_ARTIFACT_DIR";
const STARTUP_TIMEOUT_ENV: &str = "CLAUDEX_NATIVE_AGENT_STARTUP_TIMEOUT_SECONDS";
const LAUNCH_TIMEOUT_ENV: &str = "CLAUDEX_NATIVE_AGENT_LAUNCH_TIMEOUT_SECONDS";
const RESPONSE_TIMEOUT_ENV: &str = "CLAUDEX_NATIVE_AGENT_RESPONSE_TIMEOUT_SECONDS";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub const AGENT_LABELS: [&str; 3] = [
    "synthetic-native-alpha",
    "synthetic-native-beta",
    "synthetic-native-gamma",
];
const NESTED_AGENT_LABEL: &str = "synthetic-nested-delta";
const WEB_EVIDENCE_URL: &str = "https://www.rfc-editor.org/rfc/rfc9110";
const WEB_EVIDENCE_MARKER: &str = "RFC9110_WEB_CONFIRMED";
const CONTROL_EVIDENCE_MARKER: &str = "CONTROL_SENT_WITHOUT_STOP";
const NESTED_EVIDENCE_MARKER: &str = "NESTED_AGENT_CONFIRMED";
const CONTEXT_LIMIT_FAILURE: &str = "Context limit reached";

const KNOWN_FAILURES: [&str; 13] = [
    "API Error: Content block is not a text block",
    "Agent terminated early due to an API error: Request timed out",
    "API Error: Response stalled mid-stream",
    "API Error: Server error mid-response",
    "SubAgent is still processing in the background",
    "Did 0 searches",
    "No task found with ID:",
    "Configured ACP turn/start queue wait timed out",
    CONTEXT_LIMIT_FAILURE,
    "Verification was blocked because this session had no shell/filesystem command tool",
    "502 Claude subscription model",
    "Claude subscription model claude-opus-5 exited with exit status: 1:",
    "Error during compaction: summarization produced empty response",
];

pub struct AcceptanceConfig {
    claudex_program: OsString,
    claude_program: OsString,
    working_directory: PathBuf,
    resume_id: Option<String>,
    artifact_directory: Option<PathBuf>,
    startup_timeout: Duration,
    launch_timeout: Duration,
    response_timeout: Duration,
}

impl AcceptanceConfig {
    pub fn opted_in() -> bool {
        env::var(OPT_IN_ENV).as_deref() == Ok("1")
    }

    pub fn from_environment() -> Result<Option<Self>, String> {
        if !Self::opted_in() {
            return Ok(None);
        }
        let working_directory = env::var_os(WORKDIR_ENV)
            .map(PathBuf::from)
            .unwrap_or(env::current_dir().map_err(|error| error.to_string())?);
        if !working_directory.is_dir() {
            return Err(format!(
                "{WORKDIR_ENV} is not a directory: {}",
                working_directory.display()
            ));
        }
        Ok(Some(Self {
            claudex_program: env::var_os(CLAUDEX_PROGRAM_ENV)
                .unwrap_or_else(|| OsString::from("claudex")),
            claude_program: env::var_os(CLAUDE_PROGRAM_ENV)
                .unwrap_or_else(|| OsString::from("claude")),
            working_directory,
            resume_id: env::var(RESUME_ID_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty()),
            artifact_directory: env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from),
            startup_timeout: duration_from_env(STARTUP_TIMEOUT_ENV, 60)?,
            launch_timeout: duration_from_env(LAUNCH_TIMEOUT_ENV, 180)?,
            response_timeout: duration_from_env(RESPONSE_TIMEOUT_ENV, 60)?,
        }))
    }

    pub fn assert_supported_version(&self) -> Result<String, String> {
        let output = Command::new(&self.claude_program)
            .arg("--version")
            .output()
            .map_err(|error| format!("failed to execute Claude Code --version: {error}"))?;
        let mut version = String::from_utf8_lossy(&output.stdout).into_owned();
        version.push_str(&String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            return Err(format!(
                "Claude Code --version failed with {}",
                output.status
            ));
        }
        if !has_exact_claude_version(&version) {
            return Err(format!(
                "native Agent UI harness requires Claude Code {REQUIRED_CLAUDE_CODE_VERSION}; observed `{}`",
                version.trim()
            ));
        }
        Ok(version.trim().to_owned())
    }

    fn claudex_arguments(&self) -> Vec<OsString> {
        let mut arguments = vec![OsString::from("--dangerously-skip-permissions")];
        if let Some(resume_id) = &self.resume_id {
            arguments.push(OsString::from("--resume"));
            arguments.push(OsString::from(resume_id));
        }
        arguments
    }
}

fn duration_from_env(name: &str, default_seconds: u64) -> Result<Duration, String> {
    let Some(value) = env::var(name).ok() else {
        return Ok(Duration::from_secs(default_seconds));
    };
    let seconds = value
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| format!("{name} must be a positive integer"))?;
    Ok(Duration::from_secs(seconds))
}

pub fn has_exact_claude_version(output: &str) -> bool {
    output.split_whitespace().any(|token| {
        token
            .trim_matches(|character| matches!(character, '(' | ')' | ','))
            .strip_prefix('v')
            .unwrap_or_else(|| token.trim_matches(|character| matches!(character, '(' | ')' | ',')))
            == REQUIRED_CLAUDE_CODE_VERSION
    })
}

pub fn known_failure(text: &str) -> Option<&'static str> {
    known_failure_except(text, None)
}

fn known_failure_except(text: &str, ignored_failure: Option<&str>) -> Option<&'static str> {
    let text = canonical_match_text(text);
    KNOWN_FAILURES
        .into_iter()
        .filter(|failure| ignored_failure != Some(*failure))
        .find(|failure| text.contains(&canonical_match_text(failure)))
}

pub fn has_native_panel_evidence(text: &str) -> bool {
    let text = canonical_match_text(text);
    let all_labels = AGENT_LABELS.iter().all(|label| text.contains(label));
    let has_launch_status = text.contains("backgroundagentslaunched")
        || text.contains("agentslaunched")
        || text.contains("agentsfinished");
    let has_agent_table =
        (text.contains("worker/model") || text.contains("workermodel")) && text.contains("result");
    all_labels && (has_launch_status || has_agent_table)
}

pub fn has_tasks_evidence(text: &str) -> bool {
    let text = canonical_match_text(text);
    ["alpha", "beta", "gamma"]
        .iter()
        .all(|suffix| text.contains(suffix))
        && (text.contains("localagents(3)") || text.contains("3activeagents"))
        && ["running", "inprogress", "background", "finished"]
            .iter()
            .any(|state| text.contains(state))
}

fn has_interactive_prompt(text: &str) -> bool {
    let text = canonical_match_text(text);
    match (
        text.rfind('❯'),
        text.rfind("bypasspermissionson"),
        text.rfind("esctointerrupt"),
    ) {
        (Some(prompt), Some(idle), busy) => prompt < idle && busy.is_none_or(|busy| busy < idle),
        _ => false,
    }
}

fn has_completed_compaction(text: &str) -> bool {
    let text = canonical_match_text(text);
    match (
        text.find("compactingconversation"),
        text.rfind('❯'),
        text.rfind("bypasspermissionson"),
        text.rfind("esctointerrupt"),
    ) {
        (Some(compacting), Some(prompt), Some(idle), busy) => {
            compacting < prompt && prompt < idle && busy.is_none_or(|busy| busy < idle)
        }
        _ => false,
    }
}

fn has_web_and_nested_evidence(text: &str) -> bool {
    let text = canonical_match_text(text);
    text.contains(&canonical_match_text(WEB_EVIDENCE_MARKER))
        && text.contains(WEB_EVIDENCE_URL)
        && text.contains(&canonical_match_text(NESTED_EVIDENCE_MARKER))
        && text.contains(NESTED_AGENT_LABEL)
}

fn canonical_match_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

#[derive(Clone, Copy)]
enum TerminalState {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

fn text_state(byte: u8, visible: &mut Vec<u8>) -> TerminalState {
    match byte {
        0x1b => TerminalState::Escape,
        0x08 => {
            visible.pop();
            TerminalState::Text
        }
        b'\r' => {
            visible.push(b'\n');
            TerminalState::Text
        }
        b'\n' | b'\t' | 0x20..=0xff => {
            visible.push(byte);
            TerminalState::Text
        }
        _ => TerminalState::Text,
    }
}

fn escape_state(byte: u8) -> TerminalState {
    match byte {
        b'[' => TerminalState::Csi,
        b']' => TerminalState::Osc,
        _ => TerminalState::Text,
    }
}

fn osc_state(byte: u8) -> TerminalState {
    match byte {
        0x07 => TerminalState::Text,
        0x1b => TerminalState::OscEscape,
        _ => TerminalState::Osc,
    }
}

fn next_terminal_state(state: TerminalState, byte: u8, visible: &mut Vec<u8>) -> TerminalState {
    match state {
        TerminalState::Text => text_state(byte, visible),
        TerminalState::Escape => escape_state(byte),
        TerminalState::Csi if (0x40..=0x7e).contains(&byte) => TerminalState::Text,
        TerminalState::Csi => TerminalState::Csi,
        TerminalState::Osc => osc_state(byte),
        TerminalState::OscEscape if byte == b'\\' => TerminalState::Text,
        TerminalState::OscEscape => TerminalState::Osc,
    }
}

pub fn normalize_terminal(raw: &[u8]) -> String {
    let mut state = TerminalState::Text;
    let mut visible = Vec::with_capacity(raw.len());
    for &byte in raw {
        state = next_terminal_state(state, byte, &mut visible);
    }
    String::from_utf8_lossy(&visible).into_owned()
}

#[derive(Default)]
struct Capture {
    raw: Mutex<Vec<u8>>,
    changed: Condvar,
}

struct TranscriptCursor {
    path: PathBuf,
    file: File,
    offset: u64,
    device: u64,
    inode: u64,
    pending: Vec<u8>,
    parsed_lines: usize,
    evidence: TranscriptEvidence,
    failures: Vec<&'static str>,
}

impl TranscriptCursor {
    fn discover(resume_id: &str) -> Result<Option<Self>, String> {
        let Some(home) = env::var_os("HOME") else {
            return Ok(None);
        };
        let projects = PathBuf::from(home).join(".claude/projects");
        let entries = match std::fs::read_dir(&projects) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "failed to inspect Claude project transcripts {}: {error}",
                    projects.display()
                ));
            }
        };
        let mut candidates = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join(format!("{resume_id}.jsonl")))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.into_iter().next().map(Self::open).transpose()
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        let mut file = File::open(&path).map_err(|error| {
            format!(
                "failed to open resume transcript {}: {error}",
                path.display()
            )
        })?;
        let metadata = file.metadata().map_err(|error| {
            format!(
                "failed to inspect resume transcript {}: {error}",
                path.display()
            )
        })?;
        let offset = metadata.len();
        file.seek(SeekFrom::Start(offset)).map_err(|error| {
            format!(
                "failed to seek resume transcript {}: {error}",
                path.display()
            )
        })?;
        Ok(Self {
            path,
            file,
            offset,
            device: metadata.dev(),
            inode: metadata.ino(),
            pending: Vec::new(),
            parsed_lines: 0,
            evidence: TranscriptEvidence::default(),
            failures: Vec::new(),
        })
    }

    fn refresh(&mut self) -> Result<(), String> {
        let metadata = std::fs::metadata(&self.path).map_err(|error| {
            format!(
                "failed to inspect resume transcript {}: {error}",
                self.path.display()
            )
        })?;
        self.verify_identity(&metadata)?;
        self.file
            .seek(SeekFrom::Start(self.offset))
            .map_err(|error| {
                format!(
                    "failed to seek resume transcript {}: {error}",
                    self.path.display()
                )
            })?;
        let mut appended = Vec::new();
        self.file.read_to_end(&mut appended).map_err(|error| {
            format!(
                "failed to read resume transcript {}: {error}",
                self.path.display()
            )
        })?;
        self.offset = self
            .offset
            .checked_add(appended.len() as u64)
            .ok_or("resume transcript offset overflowed")?;
        let post_read_metadata = std::fs::metadata(&self.path).map_err(|error| {
            format!(
                "failed to re-inspect resume transcript {} after reading: {error}",
                self.path.display()
            )
        })?;
        self.verify_identity(&post_read_metadata)?;
        self.pending.extend_from_slice(&appended);
        let records = self.take_complete_records()?;
        for record in &records {
            self.observe_record(record);
        }
        validate_partial_utf8(&self.pending, &self.path)
    }

    fn observe_record(&mut self, record: &serde_json::Value) {
        self.evidence.observe(record);
        self.failures.extend(transcript_event_failure(record));
    }

    fn verify_identity(&self, metadata: &std::fs::Metadata) -> Result<(), String> {
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(format!(
                "resume transcript {} was replaced (device/inode changed)",
                self.path.display()
            ));
        }
        if metadata.len() < self.offset {
            return Err(format!(
                "resume transcript {} was truncated from offset {} to {} bytes",
                self.path.display(),
                self.offset,
                metadata.len()
            ));
        }
        Ok(())
    }

    fn take_complete_records(&mut self) -> Result<Vec<serde_json::Value>, String> {
        let Some(end) = self
            .pending
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
        else {
            return Ok(Vec::new());
        };
        let complete = self.pending.drain(..end).collect::<Vec<_>>();
        let records = parse_json_lines(&complete, self.parsed_lines, &self.path)?;
        self.parsed_lines += complete.iter().filter(|byte| **byte == b'\n').count();
        Ok(records)
    }

    fn compaction_completed(&self) -> bool {
        self.evidence.completed()
    }

    fn failure(&self, ignored_failure: Option<&str>) -> Option<&'static str> {
        self.failures
            .iter()
            .copied()
            .find(|failure| ignored_failure != Some(*failure))
    }

    fn historical_compaction_completed(&self) -> Result<bool, String> {
        let bytes = std::fs::read(&self.path).map_err(|error| {
            format!(
                "failed to read resume transcript {}: {error}",
                self.path.display()
            )
        })?;
        let mut evidence = TranscriptEvidence::default();
        let text = String::from_utf8_lossy(&bytes);
        let records = text
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());
        for record in records {
            evidence.observe(&record);
        }
        Ok(evidence.completed())
    }
}

#[derive(Default)]
struct TranscriptEvidence {
    boundaries: Vec<(String, String)>,
    summaries: Vec<(String, String)>,
}

impl TranscriptEvidence {
    fn observe(&mut self, record: &serde_json::Value) {
        if let Some(identity) = compaction_boundary_identity(record) {
            self.boundaries.push(identity);
        }
        if let Some(identity) = compaction_summary_identity(record) {
            self.summaries.push(identity);
        }
    }

    fn completed(&self) -> bool {
        self.boundaries
            .iter()
            .any(|boundary| self.has_matching_summary(boundary))
    }

    fn has_matching_summary(&self, boundary: &(String, String)) -> bool {
        let (boundary_uuid, anchor_uuid) = boundary;
        self.summaries.iter().any(|(summary_uuid, parent_uuid)| {
            boundary_uuid == parent_uuid && anchor_uuid == summary_uuid
        })
    }
}

fn parse_json_lines(
    complete: &[u8],
    parsed_lines: usize,
    path: &Path,
) -> Result<Vec<serde_json::Value>, String> {
    let lines = complete
        .strip_suffix(b"\n")
        .expect("complete transcript segment must end with newline");
    lines
        .split(|byte| *byte == b'\n')
        .enumerate()
        .map(|(index, line)| parse_json_line(line, parsed_lines + index + 1, path))
        .collect()
}

fn parse_json_line(
    line: &[u8],
    line_number: usize,
    path: &Path,
) -> Result<serde_json::Value, String> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let text = std::str::from_utf8(line).map_err(|error| {
        format!(
            "resume transcript {} contains invalid UTF-8 on complete JSON line {line_number}: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(text).map_err(|error| {
        format!(
            "resume transcript {} contains malformed complete JSON line {line_number}: {error}",
            path.display()
        )
    })
}

fn validate_partial_utf8(pending: &[u8], path: &Path) -> Result<(), String> {
    match std::str::from_utf8(pending) {
        Ok(_) => Ok(()),
        Err(error) if error.error_len().is_none() => Ok(()),
        Err(error) => Err(format!(
            "resume transcript {} contains invalid UTF-8 in an appended partial line: {error}",
            path.display()
        )),
    }
}

fn transcript_event_failure(record: &serde_json::Value) -> Option<&'static str> {
    is_failure_event(record)
        .then(|| failure_event_text(record))
        .flatten()
        .and_then(|text| known_failure(&text))
}

fn is_failure_event(record: &serde_json::Value) -> bool {
    match record.get("type").and_then(serde_json::Value::as_str) {
        Some("assistant") => {
            record
                .pointer("/message/role")
                .and_then(serde_json::Value::as_str)
                == Some("assistant")
                && record
                    .get("isApiErrorMessage")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }
        Some("system") => is_failure_system_event(record),
        Some("error") => true,
        _ => false,
    }
}

fn is_failure_system_event(record: &serde_json::Value) -> bool {
    match record.get("subtype").and_then(serde_json::Value::as_str) {
        Some("local_command") => {
            record.get("isMeta").and_then(serde_json::Value::as_bool) == Some(false)
        }
        Some("api_error" | "error") => true,
        _ => false,
    }
}

fn failure_event_text(record: &serde_json::Value) -> Option<String> {
    let mut text = String::new();
    for pointer in ["/content", "/error", "/errorDetails", "/message/content"] {
        if let Some(value) = record.pointer(pointer) {
            append_json_text(value, &mut text);
        }
    }
    (!text.is_empty()).then_some(text)
}

fn append_json_text(value: &serde_json::Value, output: &mut String) {
    match value {
        serde_json::Value::String(text) => {
            output.push_str(text);
            output.push('\n');
        }
        serde_json::Value::Array(values) => {
            values
                .iter()
                .for_each(|value| append_json_text(value, output));
        }
        serde_json::Value::Object(values) => {
            values
                .values()
                .for_each(|value| append_json_text(value, output));
        }
        _ => {}
    }
}

fn compaction_boundary_identity(record: &serde_json::Value) -> Option<(String, String)> {
    (record.get("type")?.as_str()? == "system"
        && record.get("subtype")?.as_str()? == "compact_boundary"
        && record.pointer("/compactMetadata/trigger")?.as_str()? == "manual")
        .then(|| {
            Some((
                record.get("uuid")?.as_str()?.to_owned(),
                record
                    .pointer("/compactMetadata/preservedMessages/anchorUuid")?
                    .as_str()?
                    .to_owned(),
            ))
        })?
}

fn compaction_summary_identity(record: &serde_json::Value) -> Option<(String, String)> {
    (record.get("type")?.as_str()? == "user" && record.get("isCompactSummary")?.as_bool()?).then(
        || {
            Some((
                record.get("uuid")?.as_str()?.to_owned(),
                record.get("parentUuid")?.as_str()?.to_owned(),
            ))
        },
    )?
}

impl Capture {
    fn append(&self, bytes: &[u8]) {
        self.raw
            .lock()
            .expect("PTY capture lock poisoned")
            .extend_from_slice(bytes);
        self.changed.notify_all();
    }

    fn mark(&self) -> usize {
        self.raw.lock().expect("PTY capture lock poisoned").len()
    }

    fn text_since(&self, mark: usize) -> String {
        let raw = self.raw.lock().expect("PTY capture lock poisoned");
        normalize_terminal(&raw[mark.min(raw.len())..])
    }

    fn all_text(&self) -> String {
        normalize_terminal(&self.raw.lock().expect("PTY capture lock poisoned"))
    }
}

pub struct PtySession {
    child: Child,
    input: Option<ChildStdin>,
    capture: Arc<Capture>,
    readers: Vec<thread::JoinHandle<()>>,
    failure_baseline: Option<usize>,
}

enum WaitIteration {
    Ready(String),
    Pending(Duration),
}

impl PtySession {
    pub fn spawn(config: &AcceptanceConfig) -> Result<Self, String> {
        let arguments = config.claudex_arguments();
        Self::spawn_program(
            &config.claudex_program,
            &arguments,
            &config.working_directory,
        )
    }

    fn spawn_program(
        program: &OsStr,
        arguments: &[OsString],
        working_directory: &Path,
    ) -> Result<Self, String> {
        let mut command = script_command(program, arguments);
        command
            .current_dir(working_directory)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start PTY through `script`: {error}"))?;
        let input = child.stdin.take().ok_or("PTY stdin was not captured")?;
        let stdout = child.stdout.take().ok_or("PTY stdout was not captured")?;
        let stderr = child.stderr.take().ok_or("PTY stderr was not captured")?;
        let capture = Arc::new(Capture::default());
        let readers = vec![
            spawn_reader(stdout, Arc::clone(&capture)),
            spawn_reader(stderr, Arc::clone(&capture)),
        ];
        Ok(Self {
            child,
            input: Some(input),
            capture,
            readers,
            failure_baseline: None,
        })
    }

    pub fn mark(&self) -> usize {
        self.capture.mark()
    }

    pub fn send_line(&mut self, line: &str) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("PTY stdin is closed")?;
        input
            .write_all(line.as_bytes())
            .and_then(|_| input.flush())
            .map_err(|error| format!("failed to write PTY input: {error}"))?;
        // Claude Code collapses a large burst into a paste placeholder. A TUI
        // Enter key is carriage return, not the line-feed accepted by simple
        // pipe fixtures, and must arrive after the paste has been rendered.
        thread::sleep(Duration::from_millis(150));
        input
            .write_all(b"\r")
            .and_then(|_| input.flush())
            .map_err(|error| format!("failed to submit PTY input: {error}"))
    }

    pub fn send_escape(&mut self) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("PTY stdin is closed")?;
        input
            .write_all(b"\x1b")
            .and_then(|_| input.flush())
            .map_err(|error| format!("failed to write PTY escape: {error}"))
    }

    pub fn begin_failure_scan(&mut self, baseline: usize) {
        self.failure_baseline = Some(baseline);
    }

    pub fn wait_for(
        &mut self,
        mark: usize,
        timeout: Duration,
        label: &str,
        predicate: impl Fn(&str) -> bool,
    ) -> Result<String, String> {
        self.wait_for_ignoring_known_failure(mark, timeout, label, predicate, None)
    }

    fn wait_for_ignoring_known_failure(
        &mut self,
        mark: usize,
        timeout: Duration,
        label: &str,
        predicate: impl Fn(&str) -> bool,
        ignored_failure: Option<&str>,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.inspect_wait(mark, deadline, timeout, label, &predicate, ignored_failure)? {
                WaitIteration::Ready(text) => return Ok(text),
                WaitIteration::Pending(interval) => self.wait_for_capture_change(interval),
            }
        }
    }

    fn wait_for_compaction(
        &mut self,
        mark: usize,
        timeout: Duration,
        transcript: &mut Option<TranscriptCursor>,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.inspect_compaction_wait(mark, deadline, timeout, transcript)? {
                WaitIteration::Ready(text) => return Ok(text),
                WaitIteration::Pending(interval) => self.wait_for_capture_change(interval),
            }
        }
    }

    fn inspect_compaction_wait(
        &mut self,
        mark: usize,
        deadline: Instant,
        timeout: Duration,
        transcript: &mut Option<TranscriptCursor>,
    ) -> Result<WaitIteration, String> {
        let transcript_completed = match transcript.as_mut() {
            Some(cursor) => Some(refresh_compaction_cursor(cursor)?),
            None => None,
        };
        let text = self.capture.text_since(mark);
        if compaction_wait_completed(&text, transcript_completed) {
            return Ok(WaitIteration::Ready(text));
        }
        self.inspect_wait(
            mark,
            deadline,
            timeout,
            "explicit resume compaction and prompt return",
            &|_| false,
            Some(CONTEXT_LIMIT_FAILURE),
        )
    }

    fn inspect_wait(
        &mut self,
        mark: usize,
        deadline: Instant,
        timeout: Duration,
        label: &str,
        predicate: &impl Fn(&str) -> bool,
        ignored_failure: Option<&str>,
    ) -> Result<WaitIteration, String> {
        let text = self.capture.text_since(mark);
        let failure = self.failure_baseline.and_then(|baseline| {
            known_failure_except(&self.capture.text_since(baseline), ignored_failure)
        });
        if let Some(failure) = failure {
            return Err(format!(
                "observed known failure while waiting for {label}: {failure}"
            ));
        }
        if predicate(&text) {
            return Ok(WaitIteration::Ready(text));
        }
        let status = self
            .child
            .try_wait()
            .map_err(|error| format!("failed to inspect PTY child: {error}"))?;
        if let Some(status) = status {
            return Err(format!(
                "PTY child exited with {status} while waiting for {label}"
            ));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("timed out after {timeout:?} waiting for {label}"));
        }
        Ok(WaitIteration::Pending(POLL_INTERVAL.min(deadline - now)))
    }

    fn wait_for_capture_change(&self, interval: Duration) {
        let guard = self.capture.raw.lock().expect("PTY capture lock poisoned");
        let _ = self
            .capture
            .changed
            .wait_timeout(guard, interval)
            .expect("PTY capture lock poisoned");
    }

    pub fn assert_no_known_failure(&self) -> Result<(), String> {
        let text = self
            .failure_baseline
            .map(|baseline| self.capture.text_since(baseline))
            .unwrap_or_default();
        known_failure(&text)
            .map(|failure| Err(format!("native Agent acceptance observed: {failure}")))
            .unwrap_or(Ok(()))
    }

    pub fn write_artifact(&self, config: &AcceptanceConfig) -> Result<Option<PathBuf>, String> {
        let Some(directory) = &config.artifact_directory else {
            return Ok(None);
        };
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("failed to create artifact directory: {error}"))?;
        let path = directory.join(format!("native-agent-ui-{}.txt", self.child.id()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("failed to create PTY artifact: {error}"))?;
        file.write_all(self.capture.all_text().as_bytes())
            .map_err(|error| format!("failed to write PTY artifact: {error}"))?;
        Ok(Some(path))
    }
}

fn compaction_wait_completed(text: &str, transcript_completed: Option<bool>) -> bool {
    let ui_completed = has_completed_compaction(text);
    transcript_completed.map_or(ui_completed, |completed| completed && ui_completed)
}

fn refresh_compaction_cursor(cursor: &mut TranscriptCursor) -> Result<bool, String> {
    cursor.refresh()?;
    if let Some(failure) = cursor.failure(Some(CONTEXT_LIMIT_FAILURE)) {
        return Err(format!(
            "observed known failure while waiting for explicit resume compaction and prompt return: {failure}"
        ));
    }
    Ok(cursor.compaction_completed())
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Some(input) = self.input.as_mut() {
            let _ = input.write_all(b"\x03");
            let _ = input.flush();
        }
        self.input.take();
        terminate_process_group(&mut self.child);
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn terminate_process_group(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    // The harness creates `script` as a process-group leader. Signalling
    // the group also terminates the interactive Claude descendant.
    let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
    thread::sleep(Duration::from_millis(100));
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = child.kill();
}

fn capture_reader_chunk(reader: &mut impl Read, capture: &Capture, buffer: &mut [u8]) -> bool {
    match reader.read(buffer) {
        Ok(0) | Err(_) => false,
        Ok(count) => {
            capture.append(&buffer[..count]);
            true
        }
    }
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    capture: Arc<Capture>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while capture_reader_chunk(&mut reader, &capture, &mut buffer) {}
    })
}

fn script_command(program: &OsStr, arguments: &[OsString]) -> Command {
    let mut command = Command::new("script");
    #[cfg(target_os = "macos")]
    {
        command.args([OsStr::new("-q"), OsStr::new("/dev/null"), program]);
        command.args(arguments);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let shell_command = std::iter::once(program.to_os_string())
            .chain(arguments.iter().cloned())
            .map(|argument| shell_quote(&argument))
            .collect::<Vec<_>>()
            .join(" ");
        command.args(["-qefc", &shell_command, "/dev/null"]);
    }
    command
}

#[cfg(not(target_os = "macos"))]
fn shell_quote(value: &OsStr) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
}

pub fn launch_prompt() -> String {
    format!(
        "Use exactly three native Agent tool calls with run_in_background=true, one each with subagent_type `claudex-gpt-spark`, `claudex-grok`, and `claudex-qwen`. Give them the descriptions `{}`, `{}`, and `{}`. Alpha must actually use WebSearch for RFC 9110 on rfc-editor.org, obtain `{WEB_EVIDENCE_URL}`, then run `/bin/sleep 45` and return `{WEB_EVIDENCE_MARKER}` plus that URL. Beta must run `/bin/sleep 45` and then follow any queued user change before returning its matching label. Gamma must itself launch one native child Agent described `{NESTED_AGENT_LABEL}`; that child runs `/bin/sleep 15` and returns `{NESTED_AGENT_LABEL}`. Gamma must obtain the child result, run `/bin/sleep 30`, and return `{NESTED_EVIDENCE_MARKER}` plus `{NESTED_AGENT_LABEL}`. As soon as all three top-level async launch results are available, end the main turn immediately without waiting for task completion.",
        AGENT_LABELS[0], AGENT_LABELS[1], AGENT_LABELS[2]
    )
}

fn verify_control_and_child_evidence(
    session: &mut PtySession,
    config: &AcceptanceConfig,
) -> Result<(), String> {
    let control = session.mark();
    session.send_line(&format!(
        "Send a native follow-up message to the running task `{}`. Change only its final result by appending `USER_CHANGE_APPLIED`; do not stop it or either other task. After the native control tool succeeds, reply with only `{CONTROL_EVIDENCE_MARKER}`.",
        AGENT_LABELS[1]
    ))?;
    session.wait_for(
        control,
        config.response_timeout,
        "user-directed native Agent change",
        |text| text.contains(CONTROL_EVIDENCE_MARKER),
    )?;

    let evidence = session.mark();
    session.send_line(&format!(
        "Wait for the native results of `{}` and `{}` without launching replacement tasks. Reply with `{WEB_EVIDENCE_MARKER} {WEB_EVIDENCE_URL} {NESTED_EVIDENCE_MARKER} {NESTED_AGENT_LABEL}` only after their returned results independently prove the WebSearch and nested child completed. Do not perform WebSearch in the main session.",
        AGENT_LABELS[0], AGENT_LABELS[2]
    ))?;
    session.wait_for(
        evidence,
        config.launch_timeout,
        "SubAgent WebSearch and nested Agent completion",
        has_web_and_nested_evidence,
    )?;
    Ok(())
}

fn compact_resumed_session(
    session: &mut PtySession,
    config: &AcceptanceConfig,
) -> Result<(), String> {
    let Some(resume_id) = config.resume_id.as_deref() else {
        return Ok(());
    };
    let mut transcript = TranscriptCursor::discover(resume_id)?;
    let already_compacted = transcript
        .as_ref()
        .map(TranscriptCursor::historical_compaction_completed)
        .transpose()?
        .unwrap_or(false);
    let suspended_failure_scan = transcript.is_some();
    let saved_failure_baseline = if suspended_failure_scan {
        session.failure_baseline.take()
    } else {
        None
    };
    if already_compacted {
        if suspended_failure_scan {
            session.failure_baseline = saved_failure_baseline;
        }
        return Ok(());
    }
    let compact = session.mark();
    session.send_line(
        "/compact Preserve the active goal, repository state, unresolved user requirements, and verified test evidence. Remove redundant historical retries and repeated error transcripts.",
    )?;
    let wait_result = session.wait_for_compaction(compact, config.launch_timeout, &mut transcript);
    if let Err(error) = wait_result {
        if suspended_failure_scan {
            session.failure_baseline = saved_failure_baseline;
        }
        return Err(error);
    }
    session.begin_failure_scan(session.mark());
    scan_compaction_transcript_failure(&mut transcript)?;
    thread::sleep(Duration::from_millis(500));
    scan_compaction_transcript_failure(&mut transcript)?;
    session.assert_no_known_failure()?;
    Ok(())
}

fn scan_compaction_transcript_failure(
    transcript: &mut Option<TranscriptCursor>,
) -> Result<(), String> {
    let Some(cursor) = transcript.as_mut() else {
        return Ok(());
    };
    cursor.refresh()?;
    let Some(failure) = cursor.failure(Some(CONTEXT_LIMIT_FAILURE)) else {
        return Ok(());
    };
    Err(format!(
        "observed known failure after explicit resume compaction: {failure}"
    ))
}

pub fn run_acceptance(config: &AcceptanceConfig) -> Result<String, String> {
    let version = config.assert_supported_version()?;
    let mut session = PtySession::spawn(config)?;
    let result = (|| {
        let startup = session.mark();
        session.wait_for(
            startup,
            config.startup_timeout,
            "initial Claude prompt",
            has_interactive_prompt,
        )?;
        thread::sleep(Duration::from_millis(500));
        let failure_baseline = if config.resume_id.is_some() {
            session.mark()
        } else {
            0
        };
        session.begin_failure_scan(failure_baseline);
        session.assert_no_known_failure()?;
        compact_resumed_session(&mut session, config)?;

        let launch = session.mark();
        session.send_line(&launch_prompt())?;
        session.wait_for(
            launch,
            config.launch_timeout,
            "native Agent panel",
            has_native_panel_evidence,
        )?;
        let prompt = session.mark();
        session.wait_for(
            prompt,
            config.launch_timeout,
            "prompt return after launch",
            |text| text.contains('❯'),
        )?;

        let tasks = session.mark();
        session.send_line("/tasks")?;
        session.wait_for(
            tasks,
            config.response_timeout,
            "/tasks native Agent state",
            has_tasks_evidence,
        )?;
        session.send_escape()?;
        thread::sleep(Duration::from_millis(150));

        let response = session.mark();
        session.send_line("Compute the sum of 314159 and 271828. Reply with only the number.")?;
        session.wait_for(
            response,
            config.response_timeout,
            "main prompt response",
            |text| text.contains("585987"),
        )?;

        verify_control_and_child_evidence(&mut session, config)?;
        session.assert_no_known_failure()?;
        Ok(format!(
            "Claude Code {version}; native panel, /tasks, responsive main prompt, user-directed change, SubAgent WebSearch, and nested Agent verified for {} agents{}",
            AGENT_LABELS.len(),
            if config.resume_id.is_some() {
                " after resume"
            } else {
                ""
            }
        ))
    })();
    augment_failure_with_artifact(result, &session, config)
}

fn augment_failure_with_artifact(
    result: Result<String, String>,
    session: &PtySession,
    config: &AcceptanceConfig,
) -> Result<String, String> {
    let error = match result {
        Ok(summary) => return Ok(summary),
        Err(error) => error,
    };
    let Some(path) = session.write_artifact(config)? else {
        return Err(error);
    };
    Err(format!(
        "{error}; private terminal artifact: {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn append_fixture(path: &Path, text: &str) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open transcript fixture for append")
            .write_all(text.as_bytes())
            .expect("append transcript fixture");
    }

    fn manual_boundary() -> &'static str {
        r#"{"type":"system","subtype":"compact_boundary","uuid":"boundary-1","compactMetadata":{"trigger":"manual","preservedMessages":{"anchorUuid":"summary-1"}}}"#
    }

    fn compact_summary() -> &'static str {
        r#"{"type":"user","uuid":"summary-1","parentUuid":"boundary-1","isCompactSummary":true}"#
    }

    #[test]
    fn version_gate_requires_the_exact_supported_release() {
        assert!(has_exact_claude_version("Claude Code v2.1.220"));
        assert!(has_exact_claude_version("2.1.220 (Claude Code)"));
        assert!(has_exact_claude_version("Claude Code (v2.1.220)"));
        for rejected in [
            "Claude Code v2.1.221",
            "Claude Code v2.1.220-beta",
            "Claude Code v2.1.220+metadata",
            "Claude Code prefix2.1.220",
            "Claude Code 2.1.220suffix",
            "Claude Code 12.1.220",
        ] {
            assert!(!has_exact_claude_version(rejected), "accepted `{rejected}`");
        }
    }

    #[test]
    fn terminal_normalizer_removes_ansi_osc_and_backspaces() {
        let raw = b"before\x1b[31mred\x1b[0m\r\nabc\x08d\x1b]0;title\x07after";
        assert_eq!(normalize_terminal(raw), "beforered\n\nabdafter");
    }

    #[test]
    fn transcript_cursor_recognizes_prior_compaction_without_reusing_append_state() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let path = root.path().join("resume.jsonl");
        std::fs::write(
            &path,
            format!("{}\n{}\n", manual_boundary(), compact_summary()),
        )
        .expect("write historical compaction");
        let cursor = TranscriptCursor::open(path).expect("open transcript fixture");
        assert!(
            cursor
                .historical_compaction_completed()
                .expect("scan historical compaction")
        );
        assert!(!cursor.compaction_completed());
    }

    #[test]
    fn evidence_requires_all_native_labels_and_task_state() {
        let labels = AGENT_LABELS.join(" ");
        assert!(has_native_panel_evidence(&format!(
            "3 background agents launched {labels}"
        )));
        assert!(has_tasks_evidence(&format!(
            "Background 3 active agents Local agents (3) running {labels}"
        )));
        assert!(!has_native_panel_evidence(&format!(
            "3 background agents launched {} {}",
            AGENT_LABELS[0], AGENT_LABELS[1]
        )));
        assert!(!has_tasks_evidence(&format!(
            "Background 2 active agents running {labels}"
        )));
        assert!(has_interactive_prompt(
            "old prompt ❯\ncurrent prompt ❯\nbypass permissions on"
        ));
        assert!(!has_interactive_prompt(
            "old prompt ❯\nbypass permissions pending"
        ));
        assert!(has_completed_compaction(
            "bypass permissions on\nCompacting conversation...\n❯\nbypass permissions on"
        ));
        assert!(!has_completed_compaction(
            "bypass permissions on\nCompacting conversation...\nesc to interrupt"
        ));
        assert!(!has_completed_compaction(
            "bypass permissions on\nCompacting conversation...\nbypass permissions on"
        ));
        assert!(!has_completed_compaction(
            "Compacting conversation...\n❯\nbypass permissions on\nesc to interrupt"
        ));
        assert!(has_completed_compaction(
            "Compacting conversation...\n❯\nbypass permissions on\nCompacting conversation..."
        ));
        assert!(!has_completed_compaction(
            "❯\nbypass permissions on\nCompacting conversation..."
        ));
        assert_eq!(
            known_failure_except(CONTEXT_LIMIT_FAILURE, Some(CONTEXT_LIMIT_FAILURE)),
            None
        );
        assert_eq!(
            known_failure_except(
                "Context limit reached\nAPI Error: Content block is not a text block",
                Some(CONTEXT_LIMIT_FAILURE)
            ),
            Some("API Error: Content block is not a text block")
        );
        assert!(has_web_and_nested_evidence(&format!(
            "{WEB_EVIDENCE_MARKER} {WEB_EVIDENCE_URL} {NESTED_EVIDENCE_MARKER} {NESTED_AGENT_LABEL}"
        )));
        assert!(!has_web_and_nested_evidence(&format!(
            "{WEB_EVIDENCE_MARKER} {WEB_EVIDENCE_URL} {NESTED_EVIDENCE_MARKER}"
        )));
    }

    #[test]
    fn native_panel_evidence_accepts_the_rendered_worker_table_without_prompt_echo() {
        let table = format!(
            "Label Worker / model Result {} {} {}",
            AGENT_LABELS[0], AGENT_LABELS[1], AGENT_LABELS[2]
        );
        assert!(has_native_panel_evidence(&table));
        assert!(!has_native_panel_evidence(&format!(
            "Use three agents with descriptions {} {} {} and report results",
            AGENT_LABELS[0], AGENT_LABELS[1], AGENT_LABELS[2]
        )));
    }

    #[test]
    fn known_failures_are_detected_without_generic_false_positives() {
        assert_eq!(
            known_failure("API Error: Content block is not a text block"),
            Some("API Error: Content block is not a text block")
        );
        assert_eq!(
            known_failure("APIError:Contentblockisnotatextblock"),
            Some("API Error: Content block is not a text block")
        );
        assert_eq!(known_failure("all native agents are running"), None);
    }

    #[test]
    fn transcript_cursor_ignores_history_and_summary_failure_quotations() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let path = root.path().join("resume.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"system\",\"subtype\":\"local_command\",\"isMeta\":false,\"content\":\"API Error: Response stalled mid-stream\"}\n",
        )
            .expect("write historical failure");
        let mut cursor = TranscriptCursor::open(path.clone()).expect("open transcript cursor");
        append_fixture(
            &path,
            "{\"type\":\"user\",\"uuid\":\"summary-1\",\"parentUuid\":\"boundary-1\",\"isCompactSummary\":true,\"message\":{\"content\":\"API Error: Content block is not a text block\"}}\n",
        );
        cursor.refresh().expect("scan summary quotation");
        assert_eq!(cursor.failure(Some(CONTEXT_LIMIT_FAILURE)), None);
    }

    #[test]
    fn transcript_cursor_detects_real_system_and_assistant_error_events() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let path = root.path().join("resume.jsonl");
        std::fs::write(&path, "").expect("create transcript fixture");
        let mut cursor = TranscriptCursor::open(path.clone()).expect("open transcript cursor");
        append_fixture(
            &path,
            "{\"type\":\"system\",\"subtype\":\"local_command\",\"isMeta\":false,\"content\":\"API Error: Response stalled mid-stream\"}\n",
        );
        cursor.refresh().expect("scan system failure");
        assert_eq!(
            cursor.failure(None),
            Some("API Error: Response stalled mid-stream")
        );
        append_fixture(
            &path,
            "{\"type\":\"assistant\",\"isApiErrorMessage\":true,\"error\":\"API Error: Content block is not a text block\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
        );
        cursor.refresh().expect("scan assistant failure");
        assert!(
            cursor
                .failures
                .contains(&"API Error: Content block is not a text block")
        );
    }

    #[test]
    fn resume_compaction_requires_pair_and_idle_in_either_order() {
        let idle = "Compacting conversation...\n❯\nbypass permissions on";
        let busy = "Compacting conversation...\nesc to interrupt";
        assert!(!compaction_wait_completed(idle, Some(false)));
        assert!(!compaction_wait_completed(busy, Some(true)));
        assert!(compaction_wait_completed(idle, Some(true)));
        assert!(compaction_wait_completed(idle, None));
        assert!(compaction_wait_completed(
            "Compacting conversation...\n❯\nbypass permissions on\nCompacting conversation...\n❯\nbypass permissions on",
            Some(true)
        ));
    }

    #[test]
    fn transcript_cursor_incrementally_parses_partial_matching_pair() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let path = root.path().join("resume.jsonl");
        std::fs::write(&path, "").expect("create transcript fixture");
        let mut cursor = TranscriptCursor::open(path.clone()).expect("open transcript cursor");
        append_fixture(
            &path,
            &format!("{}\n{}", manual_boundary(), &compact_summary()[..40]),
        );
        cursor.refresh().expect("scan boundary and partial summary");
        assert!(!cursor.compaction_completed());
        assert!(!cursor.pending.is_empty());
        append_fixture(&path, &format!("{}\n", &compact_summary()[40..]));
        cursor.refresh().expect("finish partial summary");
        assert!(cursor.compaction_completed());
        assert!(cursor.pending.is_empty());
    }

    #[test]
    fn transcript_cursor_rejects_malformed_complete_json_and_invalid_utf8() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let malformed = root.path().join("malformed.jsonl");
        std::fs::write(&malformed, "").expect("create malformed fixture");
        let mut cursor = TranscriptCursor::open(malformed.clone()).expect("open malformed cursor");
        append_fixture(&malformed, "{not-json}\n");
        let error = cursor.refresh().expect_err("malformed JSON must fail");
        assert!(error.contains("malformed complete JSON line"), "{error}");

        let invalid = root.path().join("invalid-utf8.jsonl");
        std::fs::write(&invalid, "").expect("create invalid UTF-8 fixture");
        let mut cursor = TranscriptCursor::open(invalid.clone()).expect("open UTF-8 cursor");
        OpenOptions::new()
            .append(true)
            .open(&invalid)
            .expect("append invalid UTF-8 fixture")
            .write_all(&[0xff, b'\n'])
            .expect("write invalid UTF-8 fixture");
        let error = cursor.refresh().expect_err("invalid UTF-8 must fail");
        assert!(error.contains("invalid UTF-8"), "{error}");
    }

    #[test]
    fn transcript_cursor_rejects_truncation_and_replacement() {
        let root = tempfile::tempdir().expect("transcript fixture");
        let truncated = root.path().join("truncated.jsonl");
        std::fs::write(&truncated, "historical transcript\n").expect("create truncate fixture");
        let mut cursor = TranscriptCursor::open(truncated.clone()).expect("open truncate cursor");
        File::create(&truncated).expect("truncate transcript");
        let error = cursor.refresh().expect_err("truncation must fail");
        assert!(error.contains("was truncated"), "{error}");

        let replaced = root.path().join("replaced.jsonl");
        std::fs::write(&replaced, "history\n").expect("create replace fixture");
        let mut cursor = TranscriptCursor::open(replaced.clone()).expect("open replace cursor");
        std::fs::remove_file(&replaced).expect("remove original transcript");
        std::fs::write(&replaced, "replacement\n").expect("replace transcript");
        let error = cursor.refresh().expect_err("replacement must fail");
        assert!(error.contains("device/inode changed"), "{error}");
    }

    #[test]
    fn local_pty_round_trip_timeout_cleanup_and_private_artifact() {
        let root = tempfile::tempdir().expect("PTY fixture");
        let artifact_directory = root.path().join("artifacts");
        let command = "read value; printf 'PTY_REPLY_%s\\n' \"$value\"; sleep 30";
        let arguments = [OsString::from("-c"), OsString::from(command)];
        let mut session = PtySession::spawn_program(OsStr::new("/bin/sh"), &arguments, root.path())
            .expect("spawn local PTY fixture");
        let process_group = session.child.id();
        let mark = session.mark();
        session
            .send_line("SYNTHETIC_INPUT")
            .expect("write PTY fixture");
        session
            .wait_for(mark, Duration::from_secs(2), "PTY round trip", |text| {
                text.contains("PTY_REPLY_SYNTHETIC_INPUT")
            })
            .expect("PTY reply");
        let timeout = session
            .wait_for(
                session.mark(),
                Duration::from_millis(25),
                "absent PTY marker",
                |text| text.contains("NEVER_EMITTED"),
            )
            .expect_err("absent marker must time out");
        assert!(timeout.contains("timed out"));

        let config = AcceptanceConfig {
            claudex_program: OsString::from("unused"),
            claude_program: OsString::from("unused"),
            working_directory: root.path().to_owned(),
            resume_id: None,
            artifact_directory: Some(artifact_directory),
            startup_timeout: Duration::from_secs(1),
            launch_timeout: Duration::from_secs(1),
            response_timeout: Duration::from_secs(1),
        };
        let artifact = session
            .write_artifact(&config)
            .expect("write artifact")
            .expect("configured artifact path");
        assert_eq!(
            std::fs::metadata(&artifact)
                .expect("artifact metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            std::fs::read_to_string(&artifact)
                .expect("artifact text")
                .contains("PTY_REPLY_SYNTHETIC_INPUT")
        );

        drop(session);
        let group_alive = unsafe { libc::kill(-(process_group as i32), 0) } == 0;
        assert!(!group_alive, "PTY process group survived harness cleanup");
    }
}
