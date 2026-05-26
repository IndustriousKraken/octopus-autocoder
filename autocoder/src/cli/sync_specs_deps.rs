//! Dependency-aware ordering pre-pass for `sync-specs --rebuild`.
//!
//! Within a `YYYY-MM-DD` day-group, alphabetical-on-slug ordering is
//! arbitrary with respect to dependency direction. A MODIFIED requirement
//! ordered before its providing ADD aborts the rebuild (openspec emits
//! `header "..." not found`). This module scans every archived change's
//! spec deltas, builds a per-capability dependency graph, topologically
//! reorders same-day archives via `aNN-` directory prefixes, and
//! persists the reordering so subsequent rebuilds see the dependency
//! order encoded in alphabetical sort.
//!
//! Two graph conditions cannot be resolved by within-day prefix renames
//! and abort the rebuild with a structured error before any rename or
//! canonical-spec update is applied:
//!
//! - cycles (A depends on B, B depends on A)
//! - cross-day backward dependencies (day D depends on day D' > D)

use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One parsed entry from a `## ADDED|MODIFIED|REMOVED|RENAMED Requirements`
/// block of an archived change's `specs/<capability>/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaEntry {
    Added { header: String },
    Modified { header: String },
    Removed { header: String },
    Renamed { from: String, to: String },
}

/// Dependency graph built from every archived change's spec deltas.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// For each (capability, requirement_header), the archived change
    /// directory name that first ADDED it. On a duplicate ADD (operator
    /// error), the alphabetically-first change name wins.
    pub originating_add: HashMap<(String, String), String>,
    /// For each archived change directory name, the set of
    /// (capability, requirement_header) pairs it depends on via
    /// MODIFIED / REMOVED / RENAMED-FROM.
    pub dependencies: HashMap<String, Vec<(String, String)>>,
}

/// Error from `build_dependency_graph` — purely filesystem reading errors
/// (the per-spec parse never fails; malformed deltas are skipped with a
/// WARN-log).
#[derive(Debug)]
pub struct ScanError {
    pub message: String,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ScanError {}

/// One planned rename: rename `from` → `to` under the archive root.
/// `dependency_chain` is a human-readable summary of why this rename is
/// needed; surfaces in the chatops notification + PR body.
#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub from: String,
    pub to: String,
    pub dependency_chain: Vec<String>,
}

/// Why the pre-pass aborted the rebuild. Carries enough detail for the
/// chatops/log message to name the offending change(s) and requirement(s)
/// without further lookup.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum RebuildAbortReason {
    Cycle {
        changes: Vec<String>,
        requirements: Vec<(String, String)>,
    },
    CrossDayBackwardDependency {
        /// Archived earlier — the change that MODIFIES / REMOVES / RENAMES-FROM.
        dependent: String,
        /// Archived later — the change that originally ADDED the requirement.
        dependency: String,
        capability: String,
        requirement_header: String,
    },
    ScanFailed {
        source_change: String,
        error: String,
    },
}

impl RebuildAbortReason {
    /// Human-readable one-line summary for chatops / log messages.
    pub fn summary(&self) -> String {
        match self {
            Self::Cycle { changes, requirements } => {
                let chs = changes.join(", ");
                let reqs: Vec<String> = requirements
                    .iter()
                    .map(|(c, h)| format!("({c}, \"{h}\")"))
                    .collect();
                format!(
                    "dependency cycle between changes [{chs}] over requirements [{}]",
                    reqs.join(", ")
                )
            }
            Self::CrossDayBackwardDependency {
                dependent,
                dependency,
                capability,
                requirement_header,
            } => format!(
                "cross-day backward dependency: `{dependent}` (archived earlier) MODIFIES/REMOVES/RENAMES-FROM `{capability}` / \"{requirement_header}\" first ADDED by `{dependency}` (archived later)"
            ),
            Self::ScanFailed { source_change, error } => {
                format!("scan failed in `{source_change}`: {error}")
            }
        }
    }
}

/// Parse the `## ADDED|MODIFIED|REMOVED|RENAMED Requirements` blocks of a
/// single capability spec file. Returns the list of deltas the file
/// declares. Malformed blocks (e.g. a RENAMED FROM with no matching TO)
/// are skipped with a WARN-log; one bad delta does not abort the scan.
pub fn parse_capability_deltas(spec_md: &str) -> Vec<DeltaEntry> {
    #[derive(Clone, Copy)]
    enum Block {
        None,
        Added,
        Modified,
        Removed,
        Renamed,
    }

    let mut out: Vec<DeltaEntry> = Vec::new();
    let mut block = Block::None;
    let mut pending_renamed_from: Option<String> = None;

    let flush_pending_from = |from: &mut Option<String>| {
        if let Some(prev) = from.take() {
            tracing::warn!(
                from = %prev,
                "parse_capability_deltas: RENAMED block ended with unmatched FROM; skipping"
            );
        }
    };

    for line in spec_md.lines() {
        let trimmed = line.trim();

        if let Some(header) = trimmed.strip_prefix("## ").map(str::trim) {
            // Leaving the current block; reset pending RENAMED state so a
            // dangling FROM doesn't bleed into the next block.
            flush_pending_from(&mut pending_renamed_from);
            block = if header.eq_ignore_ascii_case("ADDED Requirements") {
                Block::Added
            } else if header.eq_ignore_ascii_case("MODIFIED Requirements") {
                Block::Modified
            } else if header.eq_ignore_ascii_case("REMOVED Requirements") {
                Block::Removed
            } else if header.eq_ignore_ascii_case("RENAMED Requirements") {
                Block::Renamed
            } else {
                Block::None
            };
            continue;
        }

        match block {
            Block::None => {}
            Block::Added | Block::Modified | Block::Removed => {
                if let Some(h) = parse_requirement_line(trimmed) {
                    let entry = match block {
                        Block::Added => DeltaEntry::Added { header: h },
                        Block::Modified => DeltaEntry::Modified { header: h },
                        Block::Removed => DeltaEntry::Removed { header: h },
                        _ => unreachable!(),
                    };
                    out.push(entry);
                }
            }
            Block::Renamed => {
                if let Some(from) = parse_rename_marker(trimmed, "FROM:") {
                    if let Some(prev) = pending_renamed_from.take() {
                        tracing::warn!(
                            from = %prev,
                            "parse_capability_deltas: consecutive FROM lines in RENAMED block; skipping previous"
                        );
                    }
                    pending_renamed_from = Some(from);
                } else if let Some(to) = parse_rename_marker(trimmed, "TO:") {
                    if let Some(from) = pending_renamed_from.take() {
                        out.push(DeltaEntry::Renamed { from, to });
                    } else {
                        tracing::warn!(
                            to = %to,
                            "parse_capability_deltas: TO line without matching FROM in RENAMED block; skipping"
                        );
                    }
                }
            }
        }
    }

    // EOF: any unmatched FROM is malformed.
    flush_pending_from(&mut pending_renamed_from);

    out
}

/// Extract the requirement header from a `### Requirement: <header>` line.
/// Returns `None` if the line does not match.
fn parse_requirement_line(line: &str) -> Option<String> {
    // Accept any number of leading `#` (3+); the canonical form is exactly
    // three but we are forgiving for parser robustness.
    let stripped = line.trim_start_matches('#');
    if stripped == line {
        return None;
    }
    let stripped = stripped.trim_start();
    let rest = stripped.strip_prefix("Requirement:")?;
    let header = rest.trim();
    if header.is_empty() {
        None
    } else {
        Some(header.to_string())
    }
}

/// Extract the requirement header from a RENAMED block FROM/TO marker
/// line. Accepts forms like `- FROM: \`header\``, `FROM: header`,
/// `TO: \`header\``. The header is whatever follows the marker, trimmed
/// of leading/trailing whitespace and backticks.
fn parse_rename_marker(line: &str, marker: &str) -> Option<String> {
    // Strip optional leading "- " bullet (and any extra whitespace).
    let body = line.trim_start_matches('-').trim_start();
    let rest = body.strip_prefix(marker)?;
    let header = rest.trim().trim_matches('`').trim();
    if header.is_empty() {
        None
    } else {
        Some(header.to_string())
    }
}

/// Date-prefix regex for archive directory names.
fn date_prefix_regex() -> Regex {
    Regex::new(r"^(\d{4}-\d{2}-\d{2})-(.+)$").expect("static regex compiles")
}

/// Walk `archive_root`, parse every entry's `specs/<capability>/spec.md`
/// via `parse_capability_deltas`, and assemble a `DependencyGraph`.
/// Returns `Ok` even when individual entries lack spec files (no deltas
/// → no graph contribution). Returns `Err(ScanError)` only on
/// filesystem-level failure to read `archive_root` itself.
pub fn build_dependency_graph(archive_root: &Path) -> Result<DependencyGraph, ScanError> {
    let mut graph = DependencyGraph::default();
    let read = std::fs::read_dir(archive_root).map_err(|e| ScanError {
        message: format!("reading {}: {e}", archive_root.display()),
    })?;

    let date_re = date_prefix_regex();
    // Collect entries first so we can iterate in deterministic order
    // (alphabetical) for duplicate-ADD tie-breaking.
    let mut entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }
        if !date_re.is_match(&name) {
            continue;
        }
        entries.push((name, entry.path()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path) in &entries {
        let specs_dir = path.join("specs");
        if !specs_dir.is_dir() {
            continue;
        }
        // Each subdir is a capability. Iterate in deterministic order so
        // duplicate ADDs across capabilities have a stable tie-breaker.
        let cap_read = match std::fs::read_dir(&specs_dir) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    archive = %name,
                    "build_dependency_graph: cannot read {}: {e}; skipping entry",
                    specs_dir.display()
                );
                continue;
            }
        };
        let mut caps: Vec<(String, std::path::PathBuf)> = Vec::new();
        for cap in cap_read.flatten() {
            let cap_name = match cap.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let cap_path = cap.path();
            if !cap_path.is_dir() {
                continue;
            }
            caps.push((cap_name, cap_path));
        }
        caps.sort_by(|a, b| a.0.cmp(&b.0));

        for (cap_name, cap_path) in caps {
            let spec_md_path = cap_path.join("spec.md");
            if !spec_md_path.is_file() {
                continue;
            }
            let body = match std::fs::read_to_string(&spec_md_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        archive = %name,
                        capability = %cap_name,
                        "build_dependency_graph: cannot read {}: {e}; skipping",
                        spec_md_path.display()
                    );
                    continue;
                }
            };
            let deltas = parse_capability_deltas(&body);
            for delta in deltas {
                match delta {
                    DeltaEntry::Added { header } => {
                        let key = (cap_name.clone(), header);
                        if let Some(prev) = graph.originating_add.get(&key) {
                            tracing::warn!(
                                capability = %key.0,
                                requirement = %key.1,
                                prev_change = %prev,
                                this_change = %name,
                                "duplicate ADDED requirement; first-alphabetical wins"
                            );
                        } else {
                            graph.originating_add.insert(key, name.clone());
                        }
                    }
                    DeltaEntry::Modified { header } | DeltaEntry::Removed { header } => {
                        graph
                            .dependencies
                            .entry(name.clone())
                            .or_default()
                            .push((cap_name.clone(), header));
                    }
                    DeltaEntry::Renamed { from, to } => {
                        // RENAMED contributes BOTH a dependency on the FROM
                        // header AND a new originating_add for the TO header.
                        graph
                            .dependencies
                            .entry(name.clone())
                            .or_default()
                            .push((cap_name.clone(), from));
                        let to_key = (cap_name.clone(), to);
                        if let Some(prev) = graph.originating_add.get(&to_key) {
                            tracing::warn!(
                                capability = %to_key.0,
                                requirement = %to_key.1,
                                prev_change = %prev,
                                this_change = %name,
                                "duplicate originating_add (via RENAMED TO); first-alphabetical wins"
                            );
                        } else {
                            graph.originating_add.insert(to_key, name.clone());
                        }
                    }
                }
            }
        }
    }

    Ok(graph)
}

/// Compute the set of `aNN-` prefix renames needed to make every same-day
/// archive's dependency-providing change sort before its dependents.
/// Returns the minimum set of renames; entries already in the correct
/// alphabetical position are not prefixed.
pub fn compute_dependency_prefix_renames(
    archive_root: &Path,
) -> Result<Vec<RenamePlan>, RebuildAbortReason> {
    let graph = build_dependency_graph(archive_root).map_err(|e| {
        RebuildAbortReason::ScanFailed {
            source_change: String::new(),
            error: e.message,
        }
    })?;

    // Group archive entries by date prefix.
    let date_re = date_prefix_regex();
    let read = std::fs::read_dir(archive_root).map_err(|e| {
        RebuildAbortReason::ScanFailed {
            source_change: String::new(),
            error: format!("reading {}: {e}", archive_root.display()),
        }
    })?;

    // (day, current_dir_name)
    let mut all_entries: Vec<(String, String)> = Vec::new();
    for entry in read.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }
        let Some(c) = date_re.captures(&name) else {
            continue;
        };
        let day = c.get(1).expect("regex group 1").as_str().to_string();
        all_entries.push((day, name));
    }

    // Map each archive entry to its day.
    let mut entry_day: HashMap<String, String> = HashMap::new();
    for (day, name) in &all_entries {
        entry_day.insert(name.clone(), day.clone());
    }

    // First: cross-day backward dependency check (rejects before any work).
    // For every dependency edge `dependent → (cap, header)`, if the
    // originating ADD's day > dependent's day, abort.
    let mut dep_keys: Vec<&String> = graph.dependencies.keys().collect();
    dep_keys.sort();
    for dep_name in dep_keys {
        let dep_day = match entry_day.get(dep_name) {
            Some(d) => d,
            // dependency entry doesn't exist in our enumeration (e.g. a
            // change with a delta but no date prefix) — skip.
            None => continue,
        };
        let edges = graph.dependencies.get(dep_name).expect("just iterated");
        // Stable iteration of edges for deterministic error messages.
        let mut sorted_edges: Vec<&(String, String)> = edges.iter().collect();
        sorted_edges.sort();
        for (cap, header) in sorted_edges {
            let Some(originator) = graph.originating_add.get(&(cap.clone(), header.clone()))
            else {
                continue;
            };
            // A change can MODIFY/REMOVE its OWN ADD (no-op self-edge).
            if originator == dep_name {
                continue;
            }
            let Some(orig_day) = entry_day.get(originator) else {
                continue;
            };
            if orig_day.as_str() > dep_day.as_str() {
                return Err(RebuildAbortReason::CrossDayBackwardDependency {
                    dependent: dep_name.clone(),
                    dependency: originator.clone(),
                    capability: cap.clone(),
                    requirement_header: header.clone(),
                });
            }
        }
    }

    // Group entries by day.
    let mut by_day: HashMap<String, Vec<String>> = HashMap::new();
    for (day, name) in all_entries {
        by_day.entry(day).or_default().push(name);
    }
    let mut days: Vec<String> = by_day.keys().cloned().collect();
    days.sort();

    let mut plans: Vec<RenamePlan> = Vec::new();

    for day in days {
        let mut group = by_day.remove(&day).expect("from keys()");
        group.sort();

        // Build the within-day edges: for each entry in the group, find
        // its same-day dependencies (originator must also be in the group;
        // cross-day was handled above).
        let group_set: HashSet<String> = group.iter().cloned().collect();
        // edges_from_originator: originator → [dependent, ...]
        let mut edges_from_originator: HashMap<String, Vec<String>> = HashMap::new();
        // edge_reasons: (dependent, originator) → first reason string
        let mut edge_reasons: HashMap<(String, String), String> = HashMap::new();
        // in_degree[node]
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for n in &group {
            in_degree.insert(n.clone(), 0);
            edges_from_originator.entry(n.clone()).or_default();
        }
        for dep_name in &group {
            if let Some(edges) = graph.dependencies.get(dep_name) {
                let mut already_seen: HashSet<String> = HashSet::new();
                let mut sorted_edges: Vec<&(String, String)> = edges.iter().collect();
                sorted_edges.sort();
                for (cap, header) in sorted_edges {
                    let key = (cap.clone(), header.clone());
                    let Some(originator) = graph.originating_add.get(&key) else {
                        continue;
                    };
                    if originator == dep_name {
                        continue;
                    }
                    if !group_set.contains(originator) {
                        continue;
                    }
                    if !already_seen.insert(originator.clone()) {
                        continue;
                    }
                    edges_from_originator
                        .entry(originator.clone())
                        .or_default()
                        .push(dep_name.clone());
                    *in_degree.entry(dep_name.clone()).or_default() += 1;
                    edge_reasons.insert(
                        (dep_name.clone(), originator.clone()),
                        format!(
                            "dependency of `{dep}`, which MODIFIES requirement \"{header}\" added here",
                            dep = dep_name,
                            header = header
                        ),
                    );
                }
            }
        }

        // Kahn's algorithm with stable secondary sort: when multiple nodes
        // have in-degree 0, pop in original alphabetical order to preserve
        // alphabetical-secondary stability.
        let mut ready: Vec<String> = group
            .iter()
            .filter(|n| in_degree.get(*n).copied().unwrap_or(0) == 0)
            .cloned()
            .collect();
        ready.sort();
        let mut sorted_order: Vec<String> = Vec::with_capacity(group.len());
        let mut remaining_in_degree = in_degree.clone();
        while let Some(next) = if ready.is_empty() {
            None
        } else {
            Some(ready.remove(0))
        } {
            sorted_order.push(next.clone());
            if let Some(neighbors) = edges_from_originator.get(&next) {
                let mut neighbors_sorted = neighbors.clone();
                neighbors_sorted.sort();
                for nb in neighbors_sorted {
                    if let Some(deg) = remaining_in_degree.get_mut(&nb) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            ready.push(nb);
                        }
                    }
                }
                ready.sort();
            }
        }

        if sorted_order.len() != group.len() {
            // Cycle: gather the nodes that still have in-degree > 0 and
            // the edges among them.
            let cyclic_nodes: Vec<String> = group
                .iter()
                .filter(|n| !sorted_order.contains(*n))
                .cloned()
                .collect();
            let mut cyclic_reqs: Vec<(String, String)> = Vec::new();
            for dep in &cyclic_nodes {
                if let Some(edges) = graph.dependencies.get(dep) {
                    let mut sorted_edges: Vec<&(String, String)> = edges.iter().collect();
                    sorted_edges.sort();
                    for (cap, header) in sorted_edges {
                        let key = (cap.clone(), header.clone());
                        if let Some(originator) = graph.originating_add.get(&key)
                            && cyclic_nodes.contains(originator)
                        {
                            cyclic_reqs.push((cap.clone(), header.clone()));
                        }
                    }
                }
            }
            cyclic_reqs.sort();
            cyclic_reqs.dedup();
            return Err(RebuildAbortReason::Cycle {
                changes: cyclic_nodes,
                requirements: cyclic_reqs,
            });
        }

        // Determine the minimum split: the smallest k in [0..=n] such
        // that K = sorted_order[k..] (the unrenamed suffix) is sorted
        // alphabetically AND every entry in R = sorted_order[..k] (the
        // entries to be renamed `a01-`, `a02-`, …) lexically sorts
        // before K[0] when rewritten with its aNN- prefix.
        //
        // Smaller k → fewer renames. k = n (rename everything) always
        // works (modulo the >99 limit).
        //
        // Why both constraints: prefixed names sort to the front of the
        // day-group only when their (post-prefix) text is < the first
        // unrenamed entry. "a01-foo" < "no-op" because 'a' < 'n' — fine
        // for the canonical inversion. "a01-c-adds" > "a-modifies" though
        // (since '0' > '-'), so the unrenamed neighbor must also be
        // renamed when it would otherwise sort between the prefixed
        // entries.
        let n = sorted_order.len();
        let mut best_k = n;
        for k in 0..=n {
            let k_slice = &sorted_order[k..];
            let r_slice = &sorted_order[..k];
            // (a) K must be alphabetically sorted.
            if k_slice.windows(2).any(|w| w[0] > w[1]) {
                continue;
            }
            // (b) Every entry in R, after its aNN- rename, sorts before K[0].
            if let Some(first_k) = k_slice.first() {
                let mut all_under = true;
                for (i, name) in r_slice.iter().enumerate() {
                    let renamed = compose_renamed_name(name, i + 1, &date_re);
                    if renamed.as_str() >= first_k.as_str() {
                        all_under = false;
                        break;
                    }
                }
                if !all_under {
                    continue;
                }
            }
            best_k = k;
            break;
        }

        if best_k == 0 {
            // No reordering needed for this day-group.
            continue;
        }

        if best_k > 99 {
            return Err(RebuildAbortReason::ScanFailed {
                source_change: String::new(),
                error: "more than 99 same-day reorderable entries; manual intervention required"
                    .to_string(),
            });
        }

        let to_rename = &sorted_order[..best_k];

        for (i, name) in to_rename.iter().enumerate() {
            let renamed = compose_renamed_name(name, i + 1, &date_re);
            // Build a dependency-chain summary: prefer reasons taken from
            // the originator role (this entry has dependents in the
            // group), else describe the rename as collateral reordering.
            let mut chain: Vec<String> = Vec::new();
            if let Some(dependents) = edges_from_originator.get(name) {
                let mut sorted_deps = dependents.clone();
                sorted_deps.sort();
                for d in sorted_deps {
                    if let Some(reason) = edge_reasons.get(&(d.clone(), name.clone())) {
                        chain.push(reason.clone());
                    }
                }
            }
            if chain.is_empty() {
                chain.push(format!(
                    "reordered to keep dependency-prefix renames contiguous within day-group {day}"
                ));
            }
            plans.push(RenamePlan {
                from: name.clone(),
                to: renamed,
                dependency_chain: chain,
            });
        }
    }

    Ok(plans)
}

/// Build the renamed archive directory name for an entry placed at
/// topological position `nn` (1-based). Strips any existing `aNN-` prefix
/// from the slug so the new prefix slots in cleanly (idempotency).
/// `date_re` is the date-prefix regex; the entry name must match it.
fn compose_renamed_name(name: &str, nn: usize, date_re: &Regex) -> String {
    let Some(c) = date_re.captures(name) else {
        // Caller guarantees a date-prefix match; fall through to the
        // original name so a malformed entry doesn't crash the loop.
        return name.to_string();
    };
    let date = c.get(1).expect("group 1").as_str();
    let slug = c.get(2).expect("group 2").as_str();
    let stripped_slug = strip_existing_a_prefix(slug);
    format!("{date}-a{nn:02}-{stripped_slug}")
}

/// If the slug starts with `aNN-` (two digits), strip that prefix so a
/// fresh aNN- can be assigned. Idempotency relies on this: a second
/// rebuild against already-prefixed archives reassigns the same prefixes,
/// producing no net rename plan (entries are already in correct order).
fn strip_existing_a_prefix(slug: &str) -> &str {
    let bytes = slug.as_bytes();
    if bytes.len() >= 4
        && bytes[0] == b'a'
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == b'-'
    {
        &slug[4..]
    } else {
        slug
    }
}

/// Apply a rename plan to the filesystem. Each rename is atomic
/// (`std::fs::rename`); on per-rename failure, the error is logged with
/// the from/to and the loop continues so a single permission glitch
/// doesn't strand the entire plan in a half-applied state. Logs a summary
/// line with attempted vs successful counts.
pub fn apply_rename_plan(
    archive_root: &Path,
    plan: &[RenamePlan],
) -> Result<(), std::io::Error> {
    let attempted = plan.len();
    let mut successful = 0usize;
    let mut last_err: Option<std::io::Error> = None;
    for r in plan {
        let from = archive_root.join(&r.from);
        let to = archive_root.join(&r.to);
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                successful += 1;
                tracing::info!(
                    from = %from.display(),
                    to = %to.display(),
                    "applied dependency-prefix rename"
                );
            }
            Err(e) => {
                tracing::error!(
                    from = %from.display(),
                    to = %to.display(),
                    "rename failed: {e}"
                );
                last_err = Some(e);
            }
        }
    }
    tracing::info!(
        attempted,
        successful,
        "apply_rename_plan complete"
    );
    match last_err {
        Some(e) if successful < attempted => Err(e),
        _ => Ok(()),
    }
}

/// One persisted record of an applied rename, surfaced via `RebuildReport`
/// for chatops + PR body composition.
#[derive(Debug, Clone, Serialize)]
pub struct RenameRecord {
    pub from: String,
    pub to: String,
    pub day: String,
    pub dependency_summary: String,
}

impl RenameRecord {
    /// Extract the `YYYY-MM-DD` day from an archive entry name.
    pub fn day_from_name(name: &str) -> String {
        let re = date_prefix_regex();
        re.captures(name)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_default()
    }
}

impl From<&RenamePlan> for RenameRecord {
    fn from(p: &RenamePlan) -> Self {
        let day = RenameRecord::day_from_name(&p.from);
        let summary = if p.dependency_chain.is_empty() {
            String::new()
        } else {
            p.dependency_chain.join("; ")
        };
        RenameRecord {
            from: p.from.clone(),
            to: p.to.clone(),
            day,
            dependency_summary: summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---------- parse_capability_deltas ----------

    #[test]
    fn parse_added_block_single_requirement() {
        let md = "## ADDED Requirements\n\n### Requirement: Foo\nThe system SHALL foo.\n";
        let out = parse_capability_deltas(md);
        assert_eq!(out, vec![DeltaEntry::Added { header: "Foo".into() }]);
    }

    #[test]
    fn parse_modified_block_multiple_requirements() {
        let md = "## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n\n### Requirement: Bar\nBody2.\n";
        let out = parse_capability_deltas(md);
        assert_eq!(
            out,
            vec![
                DeltaEntry::Modified { header: "Foo".into() },
                DeltaEntry::Modified { header: "Bar".into() },
            ]
        );
    }

    #[test]
    fn parse_renamed_block_paired_from_to() {
        let md = "## RENAMED Requirements\n\n- FROM: `Old Name`\n  TO: `New Name`\n";
        let out = parse_capability_deltas(md);
        assert_eq!(
            out,
            vec![DeltaEntry::Renamed {
                from: "Old Name".into(),
                to: "New Name".into()
            }]
        );
    }

    #[test]
    fn parse_renamed_malformed_from_without_to_skipped() {
        let md = "## RENAMED Requirements\n\n- FROM: `Old Name`\n";
        let out = parse_capability_deltas(md);
        assert!(out.is_empty(), "malformed RENAMED must be skipped, got {out:?}");
    }

    #[test]
    fn parse_removed_block_extracts_headers() {
        let md = "## REMOVED Requirements\n\n### Requirement: Gone\nBody.\n";
        let out = parse_capability_deltas(md);
        assert_eq!(out, vec![DeltaEntry::Removed { header: "Gone".into() }]);
    }

    #[test]
    fn parse_no_delta_blocks_returns_empty() {
        let md = "# Some Capability\n\n## Purpose\n\nText.\n";
        let out = parse_capability_deltas(md);
        assert!(out.is_empty());
    }

    #[test]
    fn parse_added_block_trailing_whitespace_tolerated() {
        let md = "## ADDED Requirements   \n\n### Requirement: Foo\nBody.\n";
        let out = parse_capability_deltas(md);
        assert_eq!(out, vec![DeltaEntry::Added { header: "Foo".into() }]);
    }

    #[test]
    fn parse_renamed_from_to_without_backticks() {
        let md = "## RENAMED Requirements\n\n- FROM: Old Name\n  TO: New Name\n";
        let out = parse_capability_deltas(md);
        assert_eq!(
            out,
            vec![DeltaEntry::Renamed {
                from: "Old Name".into(),
                to: "New Name".into()
            }]
        );
    }

    #[test]
    fn parse_mixed_blocks_all_extracted() {
        let md = concat!(
            "## ADDED Requirements\n\n### Requirement: A\nBody.\n\n",
            "## MODIFIED Requirements\n\n### Requirement: B\nBody.\n\n",
            "## REMOVED Requirements\n\n### Requirement: C\nBody.\n\n",
            "## RENAMED Requirements\n\n- FROM: `D`\n  TO: `E`\n"
        );
        let out = parse_capability_deltas(md);
        assert_eq!(
            out,
            vec![
                DeltaEntry::Added { header: "A".into() },
                DeltaEntry::Modified { header: "B".into() },
                DeltaEntry::Removed { header: "C".into() },
                DeltaEntry::Renamed { from: "D".into(), to: "E".into() },
            ]
        );
    }

    // ---------- build_dependency_graph helpers ----------

    fn make_archive_root() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("archive")).unwrap();
        dir
    }

    fn write_spec(archive_root: &Path, change: &str, capability: &str, body: &str) {
        let p = archive_root.join(change).join("specs").join(capability);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("spec.md"), body).unwrap();
    }

    // ---------- build_dependency_graph ----------

    #[test]
    fn build_graph_add_and_modify() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        write_spec(
            archive,
            "2026-05-14-add-foo",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nThe system SHALL foo.\n",
        );
        write_spec(
            archive,
            "2026-05-14-modify-foo",
            "cap",
            "## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let g = build_dependency_graph(archive).unwrap();
        assert_eq!(
            g.originating_add.get(&("cap".into(), "Foo".into())).map(String::as_str),
            Some("2026-05-14-add-foo")
        );
        let deps = g.dependencies.get("2026-05-14-modify-foo").unwrap();
        assert_eq!(deps, &vec![("cap".to_string(), "Foo".to_string())]);
    }

    #[test]
    fn build_graph_renamed_contributes_both_dep_and_add() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        write_spec(
            archive,
            "2026-05-14-rename",
            "cap",
            "## RENAMED Requirements\n\n- FROM: `Foo`\n  TO: `Bar`\n",
        );

        let g = build_dependency_graph(archive).unwrap();
        let deps = g.dependencies.get("2026-05-14-rename").unwrap();
        assert_eq!(deps, &vec![("cap".to_string(), "Foo".to_string())]);
        assert_eq!(
            g.originating_add.get(&("cap".into(), "Bar".into())).map(String::as_str),
            Some("2026-05-14-rename")
        );
    }

    #[test]
    fn build_graph_empty_returns_empty() {
        let dir = TempDir::new().unwrap();
        let g = build_dependency_graph(dir.path()).unwrap();
        assert!(g.originating_add.is_empty());
        assert!(g.dependencies.is_empty());
    }

    #[test]
    fn build_graph_duplicate_add_first_alphabetical_wins() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        write_spec(
            archive,
            "2026-05-14-bbb",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-aaa",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let g = build_dependency_graph(archive).unwrap();
        // Alphabetical first ("aaa" < "bbb") wins.
        assert_eq!(
            g.originating_add.get(&("cap".into(), "Foo".into())).map(String::as_str),
            Some("2026-05-14-aaa")
        );
    }

    // ---------- compute_dependency_prefix_renames ----------

    #[test]
    fn compute_renames_two_entry_inversion() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // The ADD sorts alphabetically AFTER the MODIFY. The pre-pass
        // must prefix the ADD with a01- to move it first.
        write_spec(
            archive,
            "2026-05-14-no-op-completion-is-failure",
            "orchestrator",
            "## MODIFIED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-self-healing-deployment",
            "orchestrator",
            "## ADDED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );

        let plans = compute_dependency_prefix_renames(archive).unwrap();
        assert_eq!(plans.len(), 1, "expected exactly one rename, got {plans:#?}");
        assert_eq!(plans[0].from, "2026-05-14-self-healing-deployment");
        assert_eq!(plans[0].to, "2026-05-14-a01-self-healing-deployment");
    }

    #[test]
    fn compute_renames_no_dependencies_empty() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        write_spec(
            archive,
            "2026-05-14-foo",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-bar",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Bar\nBody.\n",
        );

        let plans = compute_dependency_prefix_renames(archive).unwrap();
        assert!(plans.is_empty(), "expected no renames, got {plans:#?}");
    }

    #[test]
    fn compute_renames_three_entry_chain() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // Alphabetical: a-modifies, b-modifies, c-adds (the ADD sorts last).
        // Both modifies depend on the ADD. Topological order is
        // [c-adds, a-modifies, b-modifies] (Kahn's with stable secondary
        // sort by alphabetical).
        //
        // To make alphabetical match topological, we must rename `c-adds`
        // with `a01-` so it sorts first. That alone isn't enough: the
        // entry `a-modifies` would alphabetically sort BEFORE `a01-c-adds`
        // (because '-' < '0'), so it must also be renamed with `a02-`.
        // `b-modifies` can stay alphabetically last. Two renames total.
        write_spec(
            archive,
            "2026-05-14-a-modifies",
            "cap",
            "## MODIFIED Requirements\n\n### Requirement: X\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-b-modifies",
            "cap",
            "## MODIFIED Requirements\n\n### Requirement: X\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-c-adds",
            "cap",
            "## ADDED Requirements\n\n### Requirement: X\nBody.\n",
        );

        let plans = compute_dependency_prefix_renames(archive).unwrap();
        assert_eq!(plans.len(), 2, "expected two renames, got {plans:#?}");
        assert_eq!(plans[0].from, "2026-05-14-c-adds");
        assert_eq!(plans[0].to, "2026-05-14-a01-c-adds");
        assert_eq!(plans[1].from, "2026-05-14-a-modifies");
        assert_eq!(plans[1].to, "2026-05-14-a02-a-modifies");
        // Verify the post-rename alphabetical order matches the
        // topological order: [a01-c-adds, a02-a-modifies, b-modifies].
        let mut final_names = vec![
            "2026-05-14-a01-c-adds".to_string(),
            "2026-05-14-a02-a-modifies".to_string(),
            "2026-05-14-b-modifies".to_string(),
        ];
        let original = final_names.clone();
        final_names.sort();
        assert_eq!(final_names, original);
    }

    #[test]
    fn compute_renames_cycle_returns_err() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // A ADDs Foo and MODIFIES Bar; B ADDs Bar and MODIFIES Foo. Cycle.
        write_spec(
            archive,
            "2026-05-14-a",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n\n## MODIFIED Requirements\n\n### Requirement: Bar\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-b",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Bar\nBody.\n\n## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let err = compute_dependency_prefix_renames(archive)
            .expect_err("cycle must abort");
        match err {
            RebuildAbortReason::Cycle { changes, requirements } => {
                assert!(changes.contains(&"2026-05-14-a".to_string()));
                assert!(changes.contains(&"2026-05-14-b".to_string()));
                assert!(!requirements.is_empty());
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn compute_renames_cross_day_backward_returns_err() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // Day-D=2026-05-10 MODIFIES requirement Foo first ADDED on day-D'=2026-05-15. Backward.
        write_spec(
            archive,
            "2026-05-10-modify-foo",
            "cap",
            "## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-15-add-foo",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let err = compute_dependency_prefix_renames(archive)
            .expect_err("cross-day backward must abort");
        match err {
            RebuildAbortReason::CrossDayBackwardDependency {
                dependent,
                dependency,
                capability,
                requirement_header,
            } => {
                assert_eq!(dependent, "2026-05-10-modify-foo");
                assert_eq!(dependency, "2026-05-15-add-foo");
                assert_eq!(capability, "cap");
                assert_eq!(requirement_header, "Foo");
            }
            other => panic!("expected CrossDayBackwardDependency, got {other:?}"),
        }
    }

    #[test]
    fn compute_renames_stability_preserves_unrelated_alphabetical() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // Three changes, none depending on each other.
        write_spec(
            archive,
            "2026-05-14-a",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-b",
            "cap",
            "## ADDED Requirements\n\n### Requirement: B\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-c",
            "cap",
            "## ADDED Requirements\n\n### Requirement: C\nBody.\n",
        );
        let plans = compute_dependency_prefix_renames(archive).unwrap();
        assert!(plans.is_empty(), "expected no renames, got {plans:#?}");
    }

    #[test]
    fn compute_renames_idempotent_on_already_prefixed() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path();
        // Names already include a01- so alphabetical IS topological.
        write_spec(
            archive,
            "2026-05-14-a01-self-healing-deployment",
            "orchestrator",
            "## ADDED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );
        write_spec(
            archive,
            "2026-05-14-no-op-completion-is-failure",
            "orchestrator",
            "## MODIFIED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );
        let plans = compute_dependency_prefix_renames(archive).unwrap();
        assert!(plans.is_empty(), "second-run should be a no-op, got {plans:#?}");
    }

    // ---------- apply_rename_plan ----------

    #[test]
    fn apply_rename_plan_moves_directory_on_disk() {
        let dir = make_archive_root();
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(archive.join("2026-05-14-foo")).unwrap();
        std::fs::write(archive.join("2026-05-14-foo/proposal.md"), b"x").unwrap();

        let plan = vec![RenamePlan {
            from: "2026-05-14-foo".into(),
            to: "2026-05-14-a01-foo".into(),
            dependency_chain: vec!["test reason".into()],
        }];
        apply_rename_plan(&archive, &plan).unwrap();
        assert!(!archive.join("2026-05-14-foo").exists());
        assert!(archive.join("2026-05-14-a01-foo").is_dir());
        assert!(archive.join("2026-05-14-a01-foo/proposal.md").exists());
    }

    #[test]
    fn apply_rename_plan_continues_on_per_rename_failure() {
        let dir = make_archive_root();
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(archive.join("2026-05-14-good")).unwrap();

        let plan = vec![
            RenamePlan {
                from: "does-not-exist".into(),
                to: "2026-05-14-a01-bad".into(),
                dependency_chain: vec!["x".into()],
            },
            RenamePlan {
                from: "2026-05-14-good".into(),
                to: "2026-05-14-a02-good".into(),
                dependency_chain: vec!["y".into()],
            },
        ];
        // First rename fails but the second still succeeds.
        let _ = apply_rename_plan(&archive, &plan);
        assert!(archive.join("2026-05-14-a02-good").is_dir());
    }

    // ---------- RenameRecord ----------

    #[test]
    fn rename_record_day_from_name_extracts_date() {
        assert_eq!(
            RenameRecord::day_from_name("2026-05-14-foo"),
            "2026-05-14"
        );
        assert_eq!(RenameRecord::day_from_name("bogus"), "");
    }

    #[test]
    fn strip_existing_a_prefix_removes_two_digit_aprefix() {
        assert_eq!(strip_existing_a_prefix("a01-foo"), "foo");
        assert_eq!(strip_existing_a_prefix("a99-bar"), "bar");
        assert_eq!(strip_existing_a_prefix("foo"), "foo");
        assert_eq!(strip_existing_a_prefix("aXX-foo"), "aXX-foo");
    }
}
