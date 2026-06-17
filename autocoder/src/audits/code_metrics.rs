//! Language-agnostic code-size scanning shared by the
//! `architecture_advisor` audit AND the code reviewer's advisory size flag.
//!
//! This module holds the surviving, correct-in-every-language pieces of the
//! retired `architecture_brightline` audit:
//!
//! - **The whole-file line-count scan**, demoted from a finding-emitting
//!   metric to the advisor's candidate SELECTOR ([`select_candidate_files`]):
//!   it ranks scanned files by line count and returns only the longest few
//!   over a pain threshold. The raw count is never emitted as a finding — it
//!   only decides which files the advisor's judgment pass examines.
//! - **The function-span + production/test split helpers**
//!   ([`function_line_spans`], [`production_test_line_split`]) the code
//!   reviewer consumes for its advisory, non-blocking size observation.
//!
//! The removed brightline metrics (function-length findings,
//! duplicate-signature, duplicate-body, AND the `.brightline-ignore`
//! suppression file) do NOT live here — they were retired wholesale by the
//! architecture-advisory redesign.

use regex::Regex;
use std::path::{Path, PathBuf};

/// Default whole-file line count used by the code reviewer's advisory size
/// flag. This is the file-size target's surfacing threshold, NOT a contract;
/// the single canonical home of the size budget is the
/// `Source files and functions stay within a size budget` requirement, which
/// this value tracks.
pub(crate) const DEFAULT_FILE_LINES_THRESHOLD: u64 = 800;
/// Default function line count used by the code reviewer's advisory size
/// flag. Like the file threshold, this references the size-budget
/// requirement rather than restating a contract.
pub(crate) const DEFAULT_FUNCTION_LINES_THRESHOLD: u64 = 200;

/// Default whole-file line count the `architecture_advisor` uses as its
/// candidate-selection pain threshold: a file must exceed this to be
/// eligible for the judgment pass. Well past the ~500-line size-budget
/// target so the advisor samples the worst offenders, not every file over
/// the budget.
pub(crate) const DEFAULT_SELECTOR_THRESHOLD: u64 = 500;
/// Default cap on the number of candidate files the advisor examines per
/// run — the worst handful, not everything over the threshold.
pub(crate) const DEFAULT_CANDIDATE_CAP: usize = 8;

/// Directories to skip entirely. Vendored / generated trees would dominate
/// the scan otherwise.
const EXCLUDED_DIR_COMPONENTS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    ".git",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
];

/// Extensions the scanner examines. Anything else is ignored; binary
/// formats and asset blobs would just clutter the scan.
const SCANNED_EXTENSIONS: &[&str] = &[
    "rs", "py", "cs", "ts", "tsx", "js", "jsx", "go", "java", "kt", "swift",
];

/// One candidate file selected by the whole-file line-count scan: its
/// workspace-relative path AND line count. The line count is internal — it
/// chooses where the advisor points judgment AND is never emitted as a
/// finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateFile {
    pub rel_path: String,
    pub lines: u64,
}

/// Select the advisor's candidate files: scan `workspace` for source files,
/// keep only those whose whole-file line count exceeds `threshold`, rank by
/// line count descending (path as a stable tie-break), AND return at most
/// `cap` of them — the longest few, not everything over the threshold. The
/// returned line counts are a SELECTOR signal only; callers MUST NOT emit
/// them as findings.
pub(crate) fn select_candidate_files(
    workspace: &Path,
    threshold: u64,
    cap: usize,
) -> Vec<CandidateFile> {
    let mut candidates: Vec<CandidateFile> = collect_source_files(workspace)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| {
            let lines = count_file_lines(&path)?;
            if lines > threshold {
                Some(CandidateFile {
                    rel_path: relative_path(&path, workspace),
                    lines,
                })
            } else {
                None
            }
        })
        .collect();
    // Longest first; ties broken by path for determinism across runs.
    candidates.sort_by(|a, b| {
        b.lines
            .cmp(&a.lines)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    candidates.truncate(cap);
    candidates
}

/// Whole-file line count for `path`, or `None` when the file cannot be read
/// (binary, permission error, vanished mid-scan).
fn count_file_lines(path: &Path) -> Option<u64> {
    let contents = std::fs::read_to_string(path).ok()?;
    Some(contents.lines().count() as u64)
}

fn collect_source_files(workspace: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(workspace, workspace, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if EXCLUDED_DIR_COMPONENTS.iter().any(|d| *d == name) {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else if ft.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && SCANNED_EXTENSIONS.contains(&ext)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// One function definition's line span within a file: its signature line
/// through the line bearing its matching closing brace. Indices are 0-based
/// into the (test-stripped) line vector.
#[derive(Debug, Clone)]
struct FunctionSpan {
    name: String,
    start_idx: usize,
    end_idx: usize,
}

/// Public view of a function's line span, exposed for cross-module reuse
/// (the code reviewer's advisory size flag). Line numbers are 1-based AND
/// inclusive, matching the diff/anchor convention.
#[derive(Debug, Clone)]
pub(crate) struct FunctionLineSpan {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl FunctionLineSpan {
    /// Number of source lines the function spans (signature through closing
    /// delimiter, inclusive).
    pub fn line_count(&self) -> u64 {
        (self.end_line - self.start_line + 1) as u64
    }
}

/// Scan `contents` for function definitions outside test-only regions,
/// returning each one's name AND 1-based inclusive line span. Test modules
/// are stripped before scanning, preserving line numbers. Exposed for the
/// reviewer's advisory size flag.
pub(crate) fn function_line_spans(contents: &str, ext: &str) -> Vec<FunctionLineSpan> {
    let stripped = if ext == "rs" {
        strip_rust_tests_modules(contents)
    } else {
        contents.to_string()
    };
    let lines: Vec<&str> = stripped.lines().collect();
    scan_function_spans(&lines, ext)
        .into_iter()
        .map(|s| FunctionLineSpan {
            name: s.name,
            start_line: s.start_idx + 1,
            end_line: s.end_idx + 1,
        })
        .collect()
}

/// Production/test line split for `contents`, exposed for the reviewer's
/// advisory. `None` when no test-only region is identifiable; otherwise
/// `(production_lines, test_lines)` summing to the total line count.
pub(crate) fn production_test_line_split(contents: &str, ext: &str) -> Option<(u64, u64)> {
    if ext != "rs" {
        return None;
    }
    let spans = rust_test_module_spans(contents);
    if spans.is_empty() {
        return None;
    }
    let total = contents.lines().count() as u64;
    let mut test_lines = 0u64;
    let mut offset = 0usize;
    for line in contents.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        if spans.iter().any(|(s, e)| line_start >= *s && line_start < *e) {
            test_lines += 1;
        }
    }
    let production = total.saturating_sub(test_lines);
    Some((production, test_lines))
}

/// Scan `lines` (already test-stripped for Rust) for function definitions,
/// returning each one's name AND line span. The span runs from the
/// signature line to the line carrying its matching closing delimiter,
/// brace-matched while skipping comments AND double-quoted strings.
/// Declarations with no `{ … }` body are skipped (no balanced span found).
fn scan_function_spans(lines: &[&str], ext: &str) -> Vec<FunctionSpan> {
    let re = match signature_regex(ext) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Some(caps) = re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if let Some(end_idx) = find_function_end(lines, idx) {
                out.push(FunctionSpan {
                    name,
                    start_idx: idx,
                    end_idx,
                });
            }
        }
    }
    out
}

/// Locate the line index of the matching closing brace for the function
/// whose signature is on `lines[start_idx]`. Brace-matches from the first
/// `{` at/after the signature line, skipping `//` line comments, `/* */`
/// block comments, AND double-quoted string literals (with `\` escapes).
/// Returns `None` when no balanced body is found.
fn find_function_end(lines: &[&str], start_idx: usize) -> Option<usize> {
    let mut depth: i64 = 0;
    let mut seen_open = false;
    let mut in_block_comment = false;
    for (i, line) in lines.iter().enumerate().skip(start_idx) {
        let bytes = line.as_bytes();
        let mut j = 0;
        let mut in_string = false;
        while j < bytes.len() {
            let b = bytes[j];
            if in_block_comment {
                if b == b'*' && j + 1 < bytes.len() && bytes[j + 1] == b'/' {
                    in_block_comment = false;
                    j += 2;
                    continue;
                }
                j += 1;
                continue;
            }
            if in_string {
                if b == b'\\' {
                    j += 2;
                    continue;
                }
                if b == b'"' {
                    in_string = false;
                }
                j += 1;
                continue;
            }
            if b == b'/' && j + 1 < bytes.len() && bytes[j + 1] == b'/' {
                break; // rest of the line is a line comment
            }
            if b == b'/' && j + 1 < bytes.len() && bytes[j + 1] == b'*' {
                in_block_comment = true;
                j += 2;
                continue;
            }
            if b == b'"' {
                in_string = true;
                j += 1;
                continue;
            }
            if b == b'{' {
                depth += 1;
                seen_open = true;
            } else if b == b'}' {
                depth -= 1;
                if seen_open && depth <= 0 {
                    return Some(i);
                }
            }
            j += 1;
        }
    }
    None
}

fn signature_regex(ext: &str) -> Option<Regex> {
    let pattern = match ext {
        "rs" => Some(r"^\s*(?:pub\s+(?:\([^)]*\)\s+)?)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)(?:\s*->\s*([^{]+))?"),
        "py" => Some(r"^\s*(?:async\s+)?def\s+(\w+)\s*\(([^)]*)\)\s*(?:->[^:]+)?\s*:"),
        "cs" => Some(r"^\s*(?:public|private|protected|internal)?\s*(?:static\s+)?(?:async\s+)?\w[\w<>?]*\s+(\w+)\s*\(([^)]*)\)"),
        "ts" | "tsx" | "js" | "jsx" => Some(
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)\s*\(([^)]*)\)",
        ),
        "go" => Some(r"^\s*func\s+(?:\([^)]+\)\s+)?(\w+)\s*\(([^)]*)\)"),
        "java" | "kt" => Some(
            r"^\s*(?:public|private|protected)?\s*(?:static\s+)?(?:async\s+)?[\w<>?\[\]]+\s+(\w+)\s*\(([^)]*)\)",
        ),
        "swift" => Some(r"^\s*(?:public|private|internal|fileprivate)?\s*func\s+(\w+)\s*\(([^)]*)\)"),
        _ => None,
    };
    pattern.and_then(|p| Regex::new(p).ok())
}

/// Byte ranges of every `mod tests { ... }` block in Rust source,
/// brace-matched so nested braces don't trip the scanner. The block can be
/// preceded by `#[cfg(test)]` or any other attribute. Each returned
/// `(start, end)` is a half-open byte range `[start, end)`.
fn rust_test_module_spans(src: &str) -> Vec<(usize, usize)> {
    let re = match Regex::new(r"(?m)^\s*(?:#\[[^\]]+\]\s*)?mod\s+tests\s*\{") {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut spans = Vec::new();
    let mut last = 0;
    let bytes = src.as_bytes();
    while let Some(m) = re.find_at(src, last) {
        let body_start = m.end(); // position right after `{`
        let mut depth: i64 = 1;
        let mut idx = body_start;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        while idx < bytes.len() {
            let b = bytes[idx];
            if in_line_comment {
                if b == b'\n' {
                    in_line_comment = false;
                }
                idx += 1;
                continue;
            }
            if in_block_comment {
                if b == b'*' && idx + 1 < bytes.len() && bytes[idx + 1] == b'/' {
                    in_block_comment = false;
                    idx += 2;
                    continue;
                }
                idx += 1;
                continue;
            }
            if b == b'/' && idx + 1 < bytes.len() {
                let next = bytes[idx + 1];
                if next == b'/' {
                    in_line_comment = true;
                    idx += 2;
                    continue;
                } else if next == b'*' {
                    in_block_comment = true;
                    idx += 2;
                    continue;
                }
            }
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    idx += 1;
                    break;
                }
            }
            idx += 1;
        }
        spans.push((m.start(), idx));
        last = idx;
    }
    spans
}

/// Strip every `mod tests { ... }` block from Rust source, brace-matched so
/// nested braces don't trip the scanner. Returns the source with the
/// matched ranges replaced by empty lines (preserving line numbers for the
/// function-span anchors).
fn strip_rust_tests_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for (start, end) in rust_test_module_spans(src) {
        out.push_str(&src[last..start]);
        let span = &src[start..end];
        let newlines = span.bytes().filter(|b| *b == b'\n').count();
        for _ in 0..newlines {
            out.push('\n');
        }
        last = end;
    }
    out.push_str(&src[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(p: &Path, contents: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("// line {i}\n")).collect()
    }

    /// The selector keeps only the longest files over the threshold, capped,
    /// ranked longest-first. (Tests task 7.1.)
    #[test]
    fn selector_picks_longest_files_over_threshold_capped() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(&ws.join("src/huge.rs"), &lines(1200));
        write(&ws.join("src/big.rs"), &lines(900));
        write(&ws.join("src/medium.rs"), &lines(700));
        write(&ws.join("src/small.rs"), &lines(100));
        // threshold 600 → huge, big, medium qualify; cap 2 → the two longest.
        let picked = select_candidate_files(ws, 600, 2);
        assert_eq!(picked.len(), 2, "cap limits the candidate set: {picked:?}");
        assert_eq!(picked[0].rel_path, "src/huge.rs");
        assert_eq!(picked[0].lines, 1200);
        assert_eq!(picked[1].rel_path, "src/big.rs");
        assert!(
            !picked.iter().any(|c| c.rel_path.contains("small.rs")),
            "short files never selected: {picked:?}"
        );
    }

    /// A short file with a long-ish function is NOT separately selected:
    /// there is no function-length metric, only the whole-file selector.
    /// (Tests task 7.1.)
    #[test]
    fn selector_does_not_flag_short_file_with_long_function() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // A 120-line file containing one 100-line function. Below the
        // 500-line file threshold → never selected.
        let mut body = String::from("fn god() {\n");
        for i in 0..100 {
            body.push_str(&format!("    let v{i} = {i};\n"));
        }
        body.push_str("}\n");
        write(&ws.join("src/onebig.rs"), &body);
        let picked = select_candidate_files(ws, DEFAULT_SELECTOR_THRESHOLD, DEFAULT_CANDIDATE_CAP);
        assert!(
            picked.is_empty(),
            "a short file is not selected on a long function: {picked:?}"
        );
    }

    #[test]
    fn selector_skips_excluded_dirs() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(&ws.join("node_modules/lib/big.js"), &lines(2000));
        write(&ws.join("target/debug/big.rs"), &lines(2000));
        let picked = select_candidate_files(ws, 500, 8);
        assert!(picked.is_empty(), "excluded dirs contribute nothing: {picked:?}");
    }

    #[test]
    fn function_line_spans_measures_rust_function() {
        let src = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let spans = function_line_spans(src, "rs");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "a");
        assert_eq!(spans[0].start_line, 1);
        assert_eq!(spans[0].end_line, 4);
        assert_eq!(spans[0].line_count(), 4);
    }

    #[test]
    fn function_line_spans_skips_test_module() {
        let src = "fn prod() {\n}\n#[cfg(test)]\nmod tests {\n    fn helper() {\n    }\n}\n";
        let spans = function_line_spans(src, "rs");
        assert!(
            spans.iter().all(|s| s.name != "helper"),
            "test-module fns are stripped: {spans:?}"
        );
        assert!(spans.iter().any(|s| s.name == "prod"));
    }

    #[test]
    fn production_test_split_reports_rust_test_region() {
        let src = "fn prod() {\n}\n#[cfg(test)]\nmod tests {\n    fn t() {\n    }\n}\n";
        let (prod, test) = production_test_line_split(src, "rs").expect("rust split present");
        assert!(test > 0, "test region counted: prod={prod} test={test}");
        assert_eq!(prod + test, src.lines().count() as u64);
    }

    #[test]
    fn production_test_split_none_for_non_rust() {
        assert!(production_test_line_split("function x() {}\n", "ts").is_none());
    }
}
