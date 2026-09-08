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

/// Zero for a count written as an explicit `null`.
///
/// `#[serde(default)]` covers a field that is *absent*; one present as
/// `null` is a type error, and a type error anywhere in the line fails the
/// whole `RawLine` — so a single `"output_tokens": null` took the other four
/// counts down with it and the request vanished from the estimate entirely.
/// Absent and `null` say the same thing about a token class, so they get the
/// same answer.
fn null_as_zero<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<u64>::deserialize(deserializer)?.unwrap_or(0))
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default, deserialize_with = "null_as_zero")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "null_as_zero")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "null_as_zero")]
    cache_read_input_tokens: u64,
    #[serde(default, deserialize_with = "null_as_zero")]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default, deserialize_with = "null_as_zero")]
    ephemeral_5m_input_tokens: u64,
    #[serde(default, deserialize_with = "null_as_zero")]
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
    // A line with no usable timestamp is dropped rather than dated. This
    // used to fall back to `Utc::now()`, which silently added the request to
    // *today's* row however old it was, and made the same transcript
    // summarize differently either side of midnight. Every figure in this
    // tab is grouped by day, so a request that cannot be placed in time
    // cannot be shown truthfully; dropping it also matches what an
    // *unparseable* timestamp already did — serde fails the whole line — so
    // the two ways of having no date now behave the same. The cost is real
    // and is the lesser one: those tokens leave the totals rather than
    // landing on a day they did not happen.
    let timestamp = raw.timestamp?;
    let message = raw.message?;
    let usage = message.usage?;

    // The TTL split is newer than the aggregate field, and can be missing or
    // incomplete. Missing: attribute the whole cache write to the 5-minute
    // rate. Incomplete — a TTL beyond the two named here, so the parts sum to
    // less than the aggregate — attribute the remainder the same way.
    //
    // The 5-minute rate is the cheaper of the two, so either kind of gap errs
    // low rather than high. What must not happen is the remainder being
    // dropped: those tokens would leave both the cost and the token count on a
    // model that *is* priced, so nothing downstream would mark the figure
    // short.
    let (write_5m, write_1h) = match usage.cache_creation {
        Some(split) => {
            let (known_5m, known_1h) = (
                split.ephemeral_5m_input_tokens,
                split.ephemeral_1h_input_tokens,
            );
            let unattributed = usage
                .cache_creation_input_tokens
                .saturating_sub(known_5m.saturating_add(known_1h));
            (known_5m.saturating_add(unattributed), known_1h)
        }
        None => (usage.cache_creation_input_tokens, 0),
    };

    Some(Entry {
        request_id: raw.request_id.unwrap_or_default(),
        model: message.model.unwrap_or_default(),
        timestamp,
        project: project.to_string(),
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_write_5m: write_5m,
        cache_write_1h: write_1h,
        cache_read: usage.cache_read_input_tokens,
    })
}

/// Every `.jsonl` file anywhere under `dir`, sorted.
///
/// Sorted because `read_dir` yields in no defined order, and a scan that
/// visits the same tree in a different order each run is a scan whose output
/// cannot be reasoned about.
///
/// Iterative rather than recursive, and `DirEntry::file_type` does not follow
/// symlinks: it reports a symlink as a symlink, which is neither `is_dir` nor
/// `is_file`, so the match below skips it entirely. No arrangement of links
/// can loop this walk, and the cost — a transcript reachable only through a
/// symlink is not read — is accepted rather than overlooked. Claude Code
/// writes real files; following links would mean resolving them and tracking
/// visited inodes to stay safe, for a case that does not arise.
fn jsonl_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let Ok(children) = std::fs::read_dir(&dir) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            match child.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind)
                    if kind.is_file()
                        && path.extension().and_then(|e| e.to_str()) == Some("jsonl") =>
                {
                    files.push(path);
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

/// Walk every `*.jsonl` under `<root>/<project>/`, reading only what has been
/// appended since the last call. `offsets` is the caller's persistent cursor
/// map.
///
/// The walk goes all the way down, not one level. Claude Code keeps a
/// session's subagent transcripts at
/// `<project>/<session-uuid>/subagents/*.jsonl`, which on this machine is 164
/// of 214 files. Stopping at the project directory's immediate children found
/// 8,782 of 15,308 requests — 2.73B tokens against 3.56B, and $1,939 against
/// $2,360, so 18% low in money and 23% in tokens — with nothing on screen to
/// suggest it. The project name is still the top-level directory's, however
/// deep the file sits: a subagent's spend belongs to the project whose session
/// ran it.
///
/// Every failure is a skip, never an error: an unreadable root, an unreadable
/// project directory, a file that vanished between listing and opening, a
/// line that will not parse. A transcript directory is not this app's to
/// validate, and there is no error string it could raise that would not risk
/// quoting a path or a line back at the user.
/// Every transcript to read, each paired with the project it counts towards,
/// in a stable order.
///
/// A child directory of `root` is a project, and every `.jsonl` anywhere
/// beneath it belongs to it however deep it sits. A `.jsonl` sitting
/// *directly* in `root` has no project directory to take a name from and was
/// skipped entirely — `read_dir` on a file fails, so the whole file was
/// passed over silently. Those tokens were really spent, and leaving them
/// out puts every total and every day quietly low, which is the one failure
/// this module is built to avoid. They now count under the file's own stem,
/// which is at least the session that produced them.
fn transcripts_under(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(children) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = children.flatten().map(|child| child.path()).collect();
    paths.sort();

    let mut transcripts = Vec::new();
    for path in paths {
        let is_loose_transcript = path.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl");
        if is_loose_transcript {
            let session = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            transcripts.push((session, path));
            continue;
        }
        // Anything else is treated as a project directory, exactly as
        // before: `jsonl_files_under` answers with an empty list for a path
        // it cannot read, so a stray non-directory costs nothing.
        let project = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        for file in jsonl_files_under(&path) {
            transcripts.push((project.clone(), file));
        }
    }
    transcripts
}

pub fn scan_dir(root: &Path, offsets: &mut HashMap<PathBuf, u64>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (project, path) in transcripts_under(root) {
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

        // Read with `read_line`, which keeps the terminator, rather than
        // `lines()`, which discards it. The cursor may only advance past
        // bytes that are certainly there: `line.len() + 1` assumes every
        // line ended in a newline, and a transcript caught mid-write — the
        // last line present, its newline not yet flushed — then leaves the
        // cursor one byte past the end of the file. That costs a full
        // re-read of the whole transcript on every later scan, and if the
        // file grows without supplying the missing newline it starts the
        // next read one byte late and loses the request that follows.
        let mut reader = BufReader::new(&mut handle);
        let mut consumed = start;
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(bytes) = reader.read_line(&mut line) else {
                break;
            };
            if bytes == 0 {
                break;
            }
            // The unterminated tail is still parsed. A line that parses is
            // a complete JSON object with only its terminator missing; a
            // genuinely half-written one is invalid JSON and is discarded
            // like any other. It is simply not counted as consumed, so the
            // next scan reads it again — and the dedup below, plus the
            // caller's own across-call guard, keep that from double
            // counting it.
            if line.ends_with('\n') {
                consumed += bytes as u64;
            }
            if let Some(entry) = parse_line(line.trim_end(), &project) {
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

    /// The first line is the one that makes the `type` guard load-bearing: it
    /// is a user turn that nonetheless carries a full `usage` block, so
    /// `message.usage?` would happily accept it and count those tokens a
    /// second time, on top of the assistant turn that really reports them.
    /// Without it the guard could be deleted and nothing would fail.
    #[test]
    fn ignores_non_assistant_and_malformed_lines() {
        assert!(
            parse_line(
                r#"{"type":"user","requestId":"r1","timestamp":"2026-09-07T08:00:00.000Z","message":{"role":"user","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":20}}}"#,
                "alpha"
            )
            .is_none(),
            "only assistant turns report what a request cost"
        );
        assert!(parse_line(r#"{"type":"user","message":{"role":"user"}}"#, "alpha").is_none());
        assert!(parse_line("not json", "alpha").is_none());
        assert!(parse_line("", "alpha").is_none());
    }

    /// A count written as an explicit `null` used to fail the whole line —
    /// serde's `default` fills in an *absent* field, not a null one, and a
    /// type error anywhere in the object fails the object. So one
    /// `"output_tokens": null` took the other four counts with it and the
    /// request left the estimate entirely. The other counts here are
    /// non-zero so that a line silently dropped is visibly different from
    /// one parsed with a zero in the null's place.
    #[test]
    fn a_null_token_count_reads_as_zero_rather_than_dropping_the_line() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":null,"cache_read_input_tokens":30,"cache_creation_input_tokens":null}}}"#;
        let entry = parse_line(line, "alpha").expect("a null count must not drop the line");
        assert_eq!(entry.input, 10);
        assert_eq!(entry.output, 0);
        assert_eq!(entry.cache_read, 30);
        assert_eq!(entry.cache_write_5m, 0);
    }

    /// The same rule one level down, in the TTL split, where a null would
    /// otherwise be doubly expensive: the line carries a
    /// `cache_creation_input_tokens` aggregate that would have been counted
    /// had the split not failed the parse.
    #[test]
    fn a_null_inside_the_cache_ttl_split_reads_as_zero_too() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"m","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":40,"cache_creation":{"ephemeral_5m_input_tokens":null,"ephemeral_1h_input_tokens":25}}}}"#;
        let entry = parse_line(line, "alpha").expect("a null count must not drop the line");
        assert_eq!(entry.cache_write_1h, 25);
        assert_eq!(
            entry.cache_write_5m, 15,
            "the null 5m field is zero, and the unattributed remainder still lands there"
        );
    }

    /// A line with no `timestamp` is dropped, not dated `Utc::now()`. The
    /// old fallback put an arbitrarily old request on *today's* row and made
    /// the same transcript summarize differently either side of midnight,
    /// and every figure in this tab is grouped by day. It also brings the
    /// missing case in line with the unparseable one below, which serde has
    /// always failed.
    ///
    /// The line is otherwise complete — a real model and real counts — so
    /// this fails if the timestamp guard is deleted rather than passing
    /// because nothing here would parse anyway.
    #[test]
    fn a_line_with_no_timestamp_is_dropped_rather_than_dated_today() {
        let usable = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":20}}}"#;
        assert!(
            parse_line(usable, "alpha").is_some(),
            "the same line with a timestamp does parse"
        );

        let undated = r#"{"type":"assistant","requestId":"r1","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":20}}}"#;
        assert!(parse_line(undated, "alpha").is_none());

        let unparseable = r#"{"type":"assistant","requestId":"r1","timestamp":"the day before yesterday","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":20}}}"#;
        assert!(
            parse_line(unparseable, "alpha").is_none(),
            "the two ways of having no date behave the same"
        );
    }

    /// A `.jsonl` sitting directly in the root rather than inside a project
    /// directory was skipped in full: the walk treated every root child as a
    /// directory, `read_dir` on a file fails, and the failure is a silent
    /// skip. Every token in it went missing from the totals with nothing on
    /// screen to say so.
    ///
    /// The project directory beside it is what makes this a test of the new
    /// branch rather than of the walk in general — the ordinary path has to
    /// keep working, and the loose file's spend has to be additional to it.
    #[test]
    fn a_transcript_directly_in_the_root_is_read_rather_than_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-Users-me-Projects-alpha");
        std::fs::create_dir_all(&project).unwrap();

        let line = |id: &str, input: u64| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","timestamp":"2026-09-07T08:00:00.000Z","message":{{"model":"claude-opus-5","usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#
            ) + "\n"
        };
        std::fs::write(project.join("session.jsonl"), line("in-project", 10)).unwrap();
        std::fs::write(dir.path().join("loose-session.jsonl"), line("loose", 20)).unwrap();

        let mut offsets = HashMap::new();
        let mut entries = scan_dir(dir.path(), &mut offsets);
        entries.sort_by(|a, b| a.request_id.cmp(&b.request_id));

        assert_eq!(entries.len(), 2, "both transcripts must be read");
        assert_eq!(entries[0].request_id, "in-project");
        assert_eq!(entries[0].project, "-Users-me-Projects-alpha");
        assert_eq!(entries[1].request_id, "loose");
        assert_eq!(
            entries[1].project, "loose-session",
            "with no project directory to name it, the file's own stem does"
        );
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
        // `all` is vacuously true on an empty vec, so a scan that found
        // nothing at all would otherwise satisfy this.
        assert!(!entries.is_empty(), "the fixture must have been read");
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

    /// Claude Code does not keep every transcript directly under the project
    /// directory: a session's subagent transcripts live at
    /// `<project>/<session-uuid>/subagents/*.jsonl`. On this machine that is
    /// 164 of 214 files — reading only the project directory's immediate
    /// children found 8,782 of 15,308 requests, leaving the estimate 18% low
    /// in money and 23% in tokens, with nothing on screen to suggest it.
    ///
    /// The spend still belongs to the project the session ran in, so the
    /// project name stays the top-level directory's however deep the file is.
    #[test]
    fn transcripts_nested_below_the_project_directory_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-Users-me-Projects-alpha");
        let nested = project.join("f5c1a581-6621-46bb-b6bb-1adf4f0d41a7/subagents");
        std::fs::create_dir_all(&nested).unwrap();

        let line = |id: &str, input: u64| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","timestamp":"2026-09-07T08:00:00.000Z","message":{{"model":"claude-opus-5","usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#
            ) + "\n"
        };
        std::fs::write(project.join("session.jsonl"), line("top", 10)).unwrap();
        std::fs::write(nested.join("agent.jsonl"), line("nested", 20)).unwrap();

        let mut offsets = HashMap::new();
        let mut entries = scan_dir(dir.path(), &mut offsets);
        entries.sort_by(|a, b| a.request_id.cmp(&b.request_id));

        assert_eq!(
            entries.len(),
            2,
            "the subagent transcript three levels down must be read too"
        );
        assert_eq!(entries[0].request_id, "nested");
        assert_eq!(entries[0].input, 20);
        assert!(
            entries
                .iter()
                .all(|e| e.project == "-Users-me-Projects-alpha"),
            "a subagent's spend belongs to the project its session ran in"
        );
    }

    /// The split being *incomplete* is a different failure from it being
    /// absent, and a worse one. If `cache_creation` ever carries a TTL this
    /// build does not know about, the two fields it does read sum to less than
    /// `cache_creation_input_tokens`, and the remainder would vanish from the
    /// cost *and* from the token count — on a priced model, so no "+" marker
    /// would appear anywhere to say the figure was short.
    ///
    /// The remainder goes to the 5-minute rate, the cheaper of the two, so an
    /// unknown TTL errs low rather than high — the same choice made when the
    /// split is missing altogether.
    #[test]
    fn a_cache_split_that_does_not_add_up_keeps_the_remainder() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":30,"ephemeral_1h_input_tokens":20}}}}"#;
        let entry = parse_line(line, "alpha").unwrap();

        assert_eq!(
            entry.cache_write_1h, 20,
            "the known 1-hour tokens are untouched"
        );
        assert_eq!(
            entry.cache_write_5m, 80,
            "the 50 tokens on an unrecognised TTL must not evaporate"
        );
        assert_eq!(
            entry.cache_write_5m + entry.cache_write_1h,
            100,
            "every cache-creation token the line reports is accounted for"
        );
    }

    /// The ordinary case must not gain phantom tokens from the same rule: when
    /// the split already adds up, there is no remainder to attribute.
    #[test]
    fn a_cache_split_that_adds_up_is_left_alone() {
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T08:00:05.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":40,"cache_creation":{"ephemeral_5m_input_tokens":15,"ephemeral_1h_input_tokens":25}}}}"#;
        let entry = parse_line(line, "alpha").unwrap();
        assert_eq!(entry.cache_write_5m, 15);
        assert_eq!(entry.cache_write_1h, 25);
    }

    /// A transcript caught mid-write: the last line is there but its newline
    /// has not been flushed yet. The cursor must not be left past the end of
    /// the file — with `line.len() + 1` it was, which both forced a full
    /// re-read of that whole transcript on every later scan and, if the file
    /// grew without supplying the missing newline, started the next read one
    /// byte late and lost the request that followed.
    ///
    /// The unterminated line is still parsed. If it parses at all it is a
    /// complete JSON object with only its terminator missing; a genuinely
    /// half-written line is invalid JSON and is discarded like any other.
    #[test]
    fn an_unterminated_final_line_leaves_the_cursor_inside_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-Users-me-Projects-alpha");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("session.jsonl");

        let line = |id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","timestamp":"2026-09-07T08:00:00.000Z","message":{{"model":"claude-opus-5","usage":{{"input_tokens":1,"output_tokens":0}}}}}}"#
            )
        };
        std::fs::write(&path, format!("{}\n{}", line("a"), line("b"))).unwrap();

        let mut offsets = HashMap::new();
        let entries = scan_dir(dir.path(), &mut offsets);
        assert_eq!(entries.len(), 2, "the unterminated line is still counted");

        let size = std::fs::metadata(&path).unwrap().len();
        let cursor = offsets[&path];
        assert!(
            cursor <= size,
            "cursor {cursor} is past the end of a {size}-byte file"
        );

        // The writer finishes the line and adds another.
        std::fs::write(
            &path,
            format!("{}\n{}\n{}\n", line("a"), line("b"), line("c")),
        )
        .unwrap();

        // Of these two, `b` is the one that does the work. The old cursor sat
        // one byte past the old end of file, which is exactly where `c` begins
        // once the newline arrives — so the buggy code reads `c` fine and
        // drops `b`, having counted it consumed before it was ever read.
        // Asserting only `c` would pass either way. (The cursor assertion
        // above catches the same bug and fires first; these two say what it
        // costs.)
        let appended = scan_dir(dir.path(), &mut offsets);
        let ids: Vec<&str> = appended.iter().map(|e| e.request_id.as_str()).collect();
        assert!(
            ids.contains(&"b"),
            "the unterminated line was consumed without being re-read: {ids:?}"
        );
        assert!(
            ids.contains(&"c"),
            "the line after the unterminated one must not be skipped: {ids:?}"
        );
    }
}
