//! Markdown-aware chunking for canonical OpenSpec specs (a21).
//!
//! Splits a `openspec/specs/<capability>/spec.md` file into one chunk
//! per `### Requirement:` heading by default. Each chunk's `text`
//! includes the requirement title and body — the SHALL paragraph + every
//! `#### Scenario:` block beneath it — so the embedding captures both
//! title and content semantics.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::config::ChunkStrategy;

/// A unit of canonical-spec content destined for embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInput {
    pub source_path: PathBuf,
    pub capability: String,
    pub requirement_title: String,
    pub scenario_titles: Vec<String>,
    /// Full text fed to the embedding provider: title + body.
    pub text: String,
}

/// Chunk a single canonical spec file into `ChunkInput`s per the
/// requested strategy.
///
/// Strategies:
/// - `PerRequirement` (default): one chunk per `### Requirement:` heading.
/// - `PerScenario`: one chunk per `#### Scenario:` (future; this fn
///   reuses `PerRequirement` semantics today and is spec-validated
///   against a contract scaffold).
/// - `PerCapability`: one chunk per spec file (future; same scaffold).
pub fn chunk_canonical_spec(
    spec_path: &Path,
    strategy: ChunkStrategy,
) -> Result<Vec<ChunkInput>> {
    let raw = std::fs::read_to_string(spec_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", spec_path.display()))?;
    let capability = derive_capability_from_path(spec_path);
    match strategy {
        ChunkStrategy::PerRequirement => Ok(chunk_per_requirement(
            spec_path,
            &capability,
            &raw,
        )),
        ChunkStrategy::PerScenario => {
            // Future strategy: ship the contract via `PerRequirement`
            // semantics so callers AND tests can exercise the surface.
            Ok(chunk_per_requirement(spec_path, &capability, &raw))
        }
        ChunkStrategy::PerCapability => {
            // Future strategy: single chunk per file = full body. The
            // contract returns a non-empty vec when the file contains
            // any text and an empty vec for an empty file.
            if raw.trim().is_empty() {
                return Ok(Vec::new());
            }
            Ok(vec![ChunkInput {
                source_path: spec_path.to_path_buf(),
                capability,
                requirement_title: "<entire-capability>".to_string(),
                scenario_titles: Vec::new(),
                text: raw,
            }])
        }
    }
}

/// Capability slug = the parent directory's basename. Falls back to
/// the file stem for unusual layouts.
fn derive_capability_from_path(spec_path: &Path) -> String {
    spec_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            spec_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        })
}

/// Per-requirement chunker. Splits on `### Requirement:` headings.
/// Lines that precede the first requirement (file preamble) are
/// discarded — the embedding is per-requirement, not per-file.
fn chunk_per_requirement(
    spec_path: &Path,
    capability: &str,
    raw: &str,
) -> Vec<ChunkInput> {
    let mut chunks = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_body: Vec<String> = Vec::new();
    let mut current_scenarios: Vec<String> = Vec::new();

    for line in raw.lines() {
        if let Some(title) = parse_heading(line, "### Requirement:") {
            // Close the previous requirement, if any.
            if let Some(prev_title) = current_title.take() {
                chunks.push(build_chunk(
                    spec_path,
                    capability,
                    &prev_title,
                    &current_scenarios,
                    &current_body,
                ));
            }
            current_title = Some(title);
            current_body.clear();
            current_scenarios.clear();
            continue;
        }
        if let Some(scenario_title) = parse_heading(line, "#### Scenario:") {
            current_scenarios.push(scenario_title);
        }
        if current_title.is_some() {
            current_body.push(line.to_string());
        }
    }

    if let Some(title) = current_title {
        chunks.push(build_chunk(
            spec_path,
            capability,
            &title,
            &current_scenarios,
            &current_body,
        ));
    }

    if chunks.is_empty() {
        tracing::warn!(
            "canonical RAG chunker: no `### Requirement:` headings found in {}",
            spec_path.display()
        );
    }
    chunks
}

fn parse_heading(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let suffix = trimmed.strip_prefix(prefix)?;
    Some(suffix.trim().to_string())
}

fn build_chunk(
    spec_path: &Path,
    capability: &str,
    title: &str,
    scenarios: &[String],
    body_lines: &[String],
) -> ChunkInput {
    let mut text = format!("### Requirement: {title}\n");
    text.push_str(&body_lines.join("\n"));
    ChunkInput {
        source_path: spec_path.to_path_buf(),
        capability: capability.to_string(),
        requirement_title: title.to_string(),
        scenario_titles: scenarios.to_vec(),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_spec(dir: &Path, capability: &str, body: &str) -> PathBuf {
        let cap_dir = dir.join("openspec/specs").join(capability);
        std::fs::create_dir_all(&cap_dir).unwrap();
        let path = cap_dir.join("spec.md");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn per_requirement_extracts_requirements_with_scenarios() {
        let tmp = TempDir::new().unwrap();
        let body = "\
## Some preamble

### Requirement: First
The system SHALL do A.

#### Scenario: Happy path
- WHEN x THEN y

#### Scenario: Edge case
- WHEN q THEN r

### Requirement: Second
The system SHALL do B.

#### Scenario: Only one
- WHEN m THEN n
";
        let path = write_spec(tmp.path(), "demo-cap", body);
        let chunks = chunk_canonical_spec(&path, ChunkStrategy::PerRequirement).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].capability, "demo-cap");
        assert_eq!(chunks[0].requirement_title, "First");
        assert_eq!(
            chunks[0].scenario_titles,
            vec!["Happy path".to_string(), "Edge case".to_string()]
        );
        assert!(chunks[0].text.contains("First"));
        assert!(chunks[0].text.contains("SHALL do A"));

        assert_eq!(chunks[1].requirement_title, "Second");
        assert_eq!(chunks[1].scenario_titles, vec!["Only one".to_string()]);
    }

    #[test]
    fn per_requirement_with_no_headings_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let body = "## Some heading\n\nJust prose. No requirements.";
        let path = write_spec(tmp.path(), "no-reqs", body);
        let chunks = chunk_canonical_spec(&path, ChunkStrategy::PerRequirement).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn per_requirement_with_empty_body_keeps_chunk() {
        let tmp = TempDir::new().unwrap();
        let body = "### Requirement: Lonely heading\n";
        let path = write_spec(tmp.path(), "empty-body", body);
        let chunks = chunk_canonical_spec(&path, ChunkStrategy::PerRequirement).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].requirement_title, "Lonely heading");
        assert!(chunks[0].scenario_titles.is_empty());
    }

    #[test]
    fn per_capability_returns_single_chunk_when_nonempty() {
        let tmp = TempDir::new().unwrap();
        let body = "### Requirement: x\n";
        let path = write_spec(tmp.path(), "cap1", body);
        let chunks = chunk_canonical_spec(&path, ChunkStrategy::PerCapability).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].requirement_title, "<entire-capability>");
    }

    #[test]
    fn per_scenario_uses_per_requirement_scaffold_today() {
        // Until full implementation lands, the contract is "returns
        // chunks for the file"; we reuse `PerRequirement` so callers
        // and tests can still exercise the surface.
        let tmp = TempDir::new().unwrap();
        let body = "### Requirement: r1\n\n#### Scenario: s1\n";
        let path = write_spec(tmp.path(), "cap-scen", body);
        let chunks = chunk_canonical_spec(&path, ChunkStrategy::PerScenario).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].requirement_title, "r1");
    }
}
