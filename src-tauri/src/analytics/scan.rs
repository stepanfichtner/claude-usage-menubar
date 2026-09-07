//! Reading Claude Code's own transcripts under `~/.claude/projects`.
//!
//! This is the only part of the app that reads the user's local files rather
//! than the usage endpoint, so the standing constraint on what may be
//! deserialized applies here exactly as it does in `profile.rs`: **the
//! structs below declare token counts, a model id, a request id and a
//! timestamp, and nothing else.** A transcript line carries the user's
//! prompts, Claude's replies, tool inputs and tool output beside those
//! counts; none of those fields exist on any type in this file, so serde
//! never materializes them and no later code can leak what was never built.
//! Nothing here logs, and nothing here panics on file content — every parse
//! failure is a discarded line, so no message anywhere can carry a fragment
//! of a transcript.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One billable request: what it cost in tokens, which model ran it, when,
/// and which project directory it came from. Deliberately not `Serialize` —
/// entries stay inside the process and only `analytics::Summary` crosses to
/// the webview.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub request_id: String,
    pub model: String,
    pub timestamp: DateTime<Utc>,
    pub project: String,
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

/// The four fields of a transcript line this app looks at. `message.content`,
/// `toolUseResult`, `cwd`, `gitBranch` and everything else a line carries are
/// undeclared, so serde skips them (spec §12.3).
#[derive(Deserialize)]
struct RawLine {
    #[serde(default)]
    r#type: String,
    #[serde(rename = "requestId", default)]
    request_id: Option<String>,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    message: Option<RawMessage>,
}

/// The model id and the usage block. Notably *not* `content`, which is where
/// the prompt and the reply live.
#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// One transcript line, or `None` if it is not an assistant turn that
/// reported usage — a user turn, a tool result, a half-written line, or
/// anything else the format grows later.
pub fn parse_line(line: &str, project: &str) -> Option<Entry> {
    let raw: RawLine = serde_json::from_str(line).ok()?;
    if raw.r#type != "assistant" {
        return None;
    }
    let message = raw.message?;
    let usage = message.usage?;

    // The TTL split is newer than the aggregate field. When it is missing,
    // attribute the whole cache write to the 5-minute rate, the cheaper of the
    // two, so the estimate errs low rather than high.
    let (write_5m, write_1h) = match usage.cache_creation {
        Some(split) => (
            split.ephemeral_5m_input_tokens,
            split.ephemeral_1h_input_tokens,
        ),
        None => (usage.cache_creation_input_tokens, 0),
    };

    Some(Entry {
        request_id: raw.request_id.unwrap_or_default(),
        model: message.model.unwrap_or_default(),
        timestamp: raw.timestamp.unwrap_or_else(Utc::now),
        project: project.to_string(),
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_write_5m: write_5m,
        cache_write_1h: write_1h,
        cache_read: usage.cache_read_input_tokens,
    })
}

/// Walk `<root>/<project>/*.jsonl`, reading only what has been appended since
/// the last call. `offsets` is the caller's persistent cursor map.
///
/// Every failure is a skip, never an error: an unreadable root, an unreadable
/// project directory, a file that vanished between listing and opening, a
/// line that will not parse. A transcript directory is not this app's to
/// validate, and there is no error string it could raise that would not risk
/// quoting a path or a line back at the user.
pub fn scan_dir(root: &Path, offsets: &mut HashMap<PathBuf, u64>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let Ok(projects) = std::fs::read_dir(root) else {
        return entries;
    };
    for project_dir in projects.flatten() {
        let project = project_dir.file_name().to_string_lossy().to_string();
        let Ok(files) = std::fs::read_dir(project_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(mut handle) = std::fs::File::open(&path) else {
                continue;
            };
            let size = handle.metadata().map(|m| m.len()).unwrap_or(0);
            let cursor = offsets.get(&path).copied().unwrap_or(0);
            // A cursor past the end means the file was rotated or truncated.
            let start = if cursor > size { 0 } else { cursor };
            if handle.seek(SeekFrom::Start(start)).is_err() {
                continue;
            }

            let mut consumed = start;
            for line in BufReader::new(&mut handle).lines().map_while(Result::ok) {
                consumed += line.len() as u64 + 1;
                if let Some(entry) = parse_line(&line, &project) {
                    // One request can span several lines; count it once. An
                    // empty id is not an identity, so it can never merge two
                    // genuinely different requests into one.
                    if entry.request_id.is_empty() || seen.insert(entry.request_id.clone()) {
                        entries.push(entry);
                    }
                }
            }
            offsets.insert(path, consumed);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcript")
    }

    #[test]
    fn parses_an_assistant_line() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40,"cache_creation":{"ephemeral_5m_input_tokens":15,"ephemeral_1h_input_tokens":25}}}}"#;
        let entry = parse_line(line, "alpha").unwrap();
        assert_eq!(entry.request_id, "r1");
        assert_eq!(entry.model, "claude-opus-5");
        assert_eq!(entry.project, "alpha");
        assert_eq!(entry.input, 10);
        assert_eq!(entry.output, 20);
        assert_eq!(entry.cache_read, 30);
        assert_eq!(entry.cache_write_5m, 15);
        assert_eq!(entry.cache_write_1h, 25);
    }

    #[test]
    fn falls_back_to_the_total_when_the_cache_ttl_split_is_absent() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"m","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":40}}}"#;
        let entry = parse_line(line, "alpha").unwrap();
        assert_eq!(entry.cache_write_5m, 40);
        assert_eq!(entry.cache_write_1h, 0);
    }

    #[test]
    fn ignores_non_assistant_and_malformed_lines() {
        assert!(parse_line(r#"{"type":"user","message":{"role":"user"}}"#, "alpha").is_none());
        assert!(parse_line("not json", "alpha").is_none());
        assert!(parse_line("", "alpha").is_none());
    }

    #[test]
    fn scanning_deduplicates_repeated_request_ids() {
        let mut offsets = HashMap::new();
        let entries = scan_dir(&fixture_root(), &mut offsets);
        let req_1_count = entries.iter().filter(|e| e.request_id == "req_1").count();
        assert_eq!(
            req_1_count, 1,
            "one line per content block must not double count"
        );
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn the_project_name_comes_from_the_directory() {
        let mut offsets = HashMap::new();
        let entries = scan_dir(&fixture_root(), &mut offsets);
        assert!(entries
            .iter()
            .all(|e| e.project == "-Users-me-Projects-alpha"));
    }

    #[test]
    fn a_second_scan_reads_nothing_new() {
        let mut offsets = HashMap::new();
        let first = scan_dir(&fixture_root(), &mut offsets);
        assert!(!first.is_empty());
        let second = scan_dir(&fixture_root(), &mut offsets);
        assert!(second.is_empty(), "the scan must be incremental");
    }

    #[test]
    fn a_shrunken_file_is_re_read_from_the_start() {
        let mut offsets = HashMap::new();
        scan_dir(&fixture_root(), &mut offsets);
        for offset in offsets.values_mut() {
            *offset = u64::MAX; // as if the file had been truncated beneath us
        }
        let entries = scan_dir(&fixture_root(), &mut offsets);
        assert_eq!(entries.len(), 3);
    }

    /// A transcript line as Claude Code really writes one: the token counts
    /// this app wants, sitting beside the prompt, the reply and a tool's
    /// output. None of those three may survive parsing.
    ///
    /// Asserted against the `Debug` rendering rather than the fields, because
    /// `Debug` is the leak surface that matters — it is what a `dbg!`, a
    /// `log::debug!` or an `unwrap` panic would print. Adding a `content`
    /// field to `RawMessage` and carrying it onto `Entry` is the regression
    /// this fails on; skipping the fields is not a convention here, it is the
    /// only reason the data cannot escape.
    #[test]
    fn transcript_content_is_never_deserialized() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","cwd":"/Users/me/SECRET-DIRECTORY","message":{"model":"claude-opus-5","content":[{"type":"text","text":"SECRET-REPLY"},{"type":"tool_use","input":{"command":"SECRET-COMMAND"}}],"usage":{"input_tokens":10,"output_tokens":20}},"toolUseResult":"SECRET-TOOL-OUTPUT"}"#;
        let entry = parse_line(line, "alpha").unwrap();

        // The counts did come through, so this is not passing by failing to
        // parse the line at all.
        assert_eq!(entry.input, 10);
        assert_eq!(entry.output, 20);
        assert_eq!(entry.model, "claude-opus-5");

        let rendered = format!("{entry:?}");
        for secret in [
            "SECRET-REPLY",
            "SECRET-COMMAND",
            "SECRET-TOOL-OUTPUT",
            "SECRET-DIRECTORY",
        ] {
            assert!(!rendered.contains(secret), "{secret} reached Entry");
        }
    }

    /// Four of the 31,000 usage-bearing assistant lines on this machine carry
    /// no `requestId` at all. An empty id is an absence, not an identity, so
    /// two such lines are two requests — deduplicating on it would silently
    /// merge unrelated requests and undercount them.
    #[test]
    fn lines_without_a_request_id_are_each_counted() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-Users-me-Projects-beta");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("session.jsonl"),
            concat!(
                r#"{"type":"assistant","timestamp":"2026-09-07T08:00:00.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":1,"output_tokens":0}}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-09-07T08:00:01.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":0}}}"#,
                "\n",
            ),
        )
        .unwrap();

        let mut offsets = HashMap::new();
        let entries = scan_dir(dir.path(), &mut offsets);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.iter().map(|e| e.input).sum::<u64>(), 3);
    }

    /// The half of incremental scanning that `a_second_scan_reads_nothing_new`
    /// cannot see: a scan that always returned nothing after the first call
    /// would satisfy that test and lose every request made from then on.
    #[test]
    fn a_line_appended_after_a_scan_is_read_by_the_next_one() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-Users-me-Projects-beta");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("session.jsonl");
        let first_line = concat!(
            r#"{"type":"assistant","requestId":"a","timestamp":"2026-09-07T08:00:00.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":1,"output_tokens":0}}}"#,
            "\n"
        );
        std::fs::write(&path, first_line).unwrap();

        let mut offsets = HashMap::new();
        assert_eq!(scan_dir(dir.path(), &mut offsets).len(), 1);
        assert!(scan_dir(dir.path(), &mut offsets).is_empty());

        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        handle
            .write_all(
                concat!(
                    r#"{"type":"assistant","requestId":"b","timestamp":"2026-09-07T08:00:01.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        drop(handle);

        let appended = scan_dir(dir.path(), &mut offsets);
        assert_eq!(appended.len(), 1, "only the appended line, and it");
        assert_eq!(appended[0].request_id, "b");
        assert_eq!(appended[0].input, 2);
    }
}
