use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A discovered Claude Code project directory with its JSONL session files.
#[derive(Debug, Clone)]
pub struct ProjectSource {
    /// The encoded directory name (e.g. `-Users-hmenzagh-code-CCMeter`).
    pub dir_name: String,
    /// Absolute path to the project directory inside Claude's data.
    pub path: PathBuf,
    /// JSONL session files found (direct or nested in subagent dirs).
    pub session_files: Vec<PathBuf>,
    /// The actual working directory extracted from JSONL metadata.
    pub cwd: Option<String>,
    /// Which Claude root this came from (e.g. `~/.claude/projects`).
    pub source_root: PathBuf,
}

impl ProjectSource {
    /// The effective root path: cwd if available, otherwise the data directory path.
    pub fn effective_root(&self) -> PathBuf {
        self.cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.path.clone())
    }
}

/// A group of project sources that belong to the same git repository.
#[derive(Debug, Clone)]
pub struct ProjectGroup {
    /// Display name for this project group.
    pub name: String,
    /// The resolved root path (git root or heuristic).
    pub root_path: PathBuf,
    /// Git remote URL if available.
    pub remote_url: Option<String>,
    /// All project sources belonging to this group.
    pub sources: Vec<ProjectSource>,
    /// Total number of JSONL session files across all sources.
    pub total_sessions: usize,
    /// Set when this group was created by an override (split or merge).
    pub override_info: Option<OverrideInfo>,
}

impl ProjectGroup {
    /// String key derived from root_path, used for overrides and cache lookups.
    pub fn root_key(&self) -> String {
        self.root_path.to_string_lossy().into_owned()
    }
}

/// How a group was created by an override.
#[derive(Debug, Clone)]
pub enum OverrideInfo {
    /// This group was split out from a larger auto-detected group.
    Split { original_root: PathBuf },
    /// This group was created by manually merging two or more groups.
    Merged,
}

/// Result of resolving a project's git identity.
#[derive(Debug, Clone)]
struct ResolvedIdentity {
    root_path: PathBuf,
    remote_url: Option<String>,
}

/// Mapping from each source root to the set of cwds that originate from it.
pub type RootMap = HashMap<PathBuf, HashSet<String>>;

/// Mapping from each session file basename to `(root, cwd)`.
pub type SessionMap = HashMap<String, (String, String)>;

/// Discover all Claude Code projects and group them by git identity.
/// Also returns:
/// - A mapping from each source root to the set of cwds that originate from it.
/// - A mapping from each session file basename to `(root, cwd)`.
///
/// Both are built *before* grouping, so they are not affected by same-cwd
/// merges across roots.
pub fn discover_project_groups_with_root_map() -> (Vec<ProjectGroup>, RootMap, SessionMap) {
    let sources = discover_sources();

    let mut root_map: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut session_map: HashMap<String, (String, String)> = HashMap::new();

    for s in &sources {
        let cwd = match &s.cwd {
            Some(c) => c.clone(),
            None => s.path.to_string_lossy().to_string(),
        };
        let root_str = s.source_root.to_string_lossy().to_string();
        root_map
            .entry(s.source_root.clone())
            .or_default()
            .insert(cwd.clone());
        for f in &s.session_files {
            if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                session_map.insert(name.to_string(), (root_str.clone(), cwd.clone()));
            }
        }
    }

    let groups = group_by_identity(sources);
    (groups, root_map, session_map)
}

// ---------------------------------------------------------------------------
// Source discovery
// ---------------------------------------------------------------------------

fn discover_sources() -> Vec<ProjectSource> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let roots = find_project_roots(&home);
    let mut sources = Vec::new();

    for root in &roots {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                let session_files = collect_jsonl_files_recursive(&path);
                if session_files.is_empty() {
                    continue;
                }

                let cwd = extract_cwd_from_first_session(&session_files);

                sources.push(ProjectSource {
                    dir_name,
                    path,
                    session_files,
                    cwd,
                    source_root: root.clone(),
                });
            }
        }
    }

    sources
}

fn find_project_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    let config_claude = home.join(".config").join("claude").join("projects");
    if config_claude.is_dir() {
        roots.push(config_claude);
    }

    let dot_claude = home.join(".claude").join("projects");
    if dot_claude.is_dir() {
        roots.push(dot_claude);
    }

    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if !name_str.starts_with('.') || !name_str.contains("claude") {
                continue;
            }
            if name_str == ".claude" || name_str == ".config" {
                continue;
            }

            let projects_dir = entry.path().join("projects");
            if projects_dir.is_dir() && !roots.contains(&projects_dir) {
                roots.push(projects_dir);
            }
        }
    }

    roots
}

// ---------------------------------------------------------------------------
// JSONL collection
// ---------------------------------------------------------------------------

fn collect_jsonl_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_into(dir, &mut files);
    files.sort();
    files
}

fn collect_jsonl_into(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            collect_jsonl_into(&path, files);
        }
    }
}

// ---------------------------------------------------------------------------
// CWD extraction from JSONL
// ---------------------------------------------------------------------------

fn extract_cwd_from_first_session(session_files: &[PathBuf]) -> Option<String> {
    for path in session_files {
        if let Some(cwd) = extract_cwd_from_jsonl(path) {
            return Some(cwd);
        }
    }
    None
}

fn extract_cwd_from_jsonl(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    for (i, line) in reader.lines().enumerate() {
        if i > 30 {
            break;
        }
        let line = line.ok()?;
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
            && value.get("type").and_then(|t| t.as_str()) == Some("user")
            && let Some(cwd) = value.get("cwd").and_then(|c| c.as_str())
        {
            return Some(cwd.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Git identity resolution
// ---------------------------------------------------------------------------

/// Resolve the git identity (root path + remote URL) for a cwd.
fn resolve_identity(cwd: &str) -> ResolvedIdentity {
    let cwd_path = Path::new(cwd);

    // Case 1: cwd exists on disk
    if cwd_path.is_dir() {
        if let Some(root) = find_git_root(cwd_path) {
            let remote_url = get_remote_url(&root);
            return ResolvedIdentity {
                root_path: root,
                remote_url,
            };
        }
        // Not a git repo — maybe it contains exactly one git child
        // (e.g., a monorepo wrapper dir like Francis-Monorepo/ or oppus/)
        if let Some(identity) = find_unique_git_child(cwd_path) {
            return identity;
        }
        return ResolvedIdentity {
            root_path: cwd_path.to_path_buf(),
            remote_url: None,
        };
    }

    // Case 2: cwd deleted — walk up parents
    let mut ancestor = cwd_path.parent();
    while let Some(dir) = ancestor {
        if !dir.is_dir() {
            ancestor = dir.parent();
            continue;
        }

        // Try git directly in this parent
        if let Some(root) = find_git_root(dir) {
            let remote_url = get_remote_url(&root);
            return ResolvedIdentity {
                root_path: root,
                remote_url,
            };
        }

        // Parent exists but isn't a git repo.
        // Scan its immediate children for a .git — if there's exactly one,
        // the deleted cwd likely belonged to that repo.
        if let Some(identity) = find_unique_git_child(dir) {
            return identity;
        }

        // Multiple or zero git children — use the original cwd path as-is
        // (don't climb further to avoid over-grouping into ~/code/ etc.)
        return ResolvedIdentity {
            root_path: cwd_path.to_path_buf(),
            remote_url: None,
        };
    }

    // Nothing exists on disk
    ResolvedIdentity {
        root_path: heuristic_root(cwd),
        remote_url: None,
    }
}

/// Find the main git repository root for a path.
/// For linked worktrees, resolves back to the main repo root.
fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let toplevel = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());

    // If .git is a file (not a directory), this is a linked worktree.
    let git_path = toplevel.join(".git");
    if git_path.is_file()
        && let Some(main_root) = resolve_worktree_main_root(&git_path)
    {
        return Some(main_root);
    }

    Some(toplevel)
}

/// Follow a worktree `.git` file → main repo root.
/// File content: `gitdir: /path/to/main-repo/.git/worktrees/<name>`
fn resolve_worktree_main_root(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir: ")?.trim();
    let gitdir_path = PathBuf::from(gitdir);

    // <main-repo>/.git/worktrees/<name> → go up 2 to .git, up 1 to main-repo
    let main_root = gitdir_path.parent()?.parent()?.parent()?;

    if main_root.is_dir() {
        Some(main_root.to_path_buf())
    } else {
        None
    }
}

/// Get the `origin` remote URL for a git repo.
fn get_remote_url(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "remote", "get-url", "origin"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

/// Scan immediate children of `dir` for `.git`.
/// If exactly one child has a git repo, return its identity.
fn find_unique_git_child(dir: &Path) -> Option<ResolvedIdentity> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };

    let mut git_children: Vec<PathBuf> = Vec::new();

    for entry in entries.filter_map(Result::ok) {
        let child = entry.path();
        if child.is_dir() && child.join(".git").exists() {
            git_children.push(child);
        }
    }

    if git_children.len() == 1 {
        let child = &git_children[0];
        let root = find_git_root(child).unwrap_or_else(|| child.clone());
        let remote_url = get_remote_url(&root);
        Some(ResolvedIdentity {
            root_path: root,
            remote_url,
        })
    } else {
        None
    }
}

/// Heuristic root for paths that don't exist on disk at all.
fn heuristic_root(cwd: &str) -> PathBuf {
    // Strip worktree segments
    for pattern in &["/worktrees/", "/Worktrees/"] {
        if let Some(idx) = cwd.find(pattern) {
            return PathBuf::from(&cwd[..idx]);
        }
    }
    PathBuf::from(cwd)
}

// ---------------------------------------------------------------------------
// Grouping by git identity (remote URL > root path)
// ---------------------------------------------------------------------------

fn group_by_identity(sources: Vec<ProjectSource>) -> Vec<ProjectGroup> {
    let mut cwd_cache: HashMap<String, ResolvedIdentity> = HashMap::new();

    // Phase 1: resolve every source
    let resolved: Vec<(ProjectSource, ResolvedIdentity)> = sources
        .into_iter()
        .map(|source| {
            let identity = if let Some(cwd) = &source.cwd {
                cwd_cache
                    .entry(cwd.clone())
                    .or_insert_with(|| resolve_identity(cwd))
                    .clone()
            } else {
                ResolvedIdentity {
                    root_path: source.path.clone(),
                    remote_url: None,
                }
            };
            (source, identity)
        })
        .collect();

    // Phase 2: build canonical root per remote URL.
    // When multiple root_paths share the same remote URL, pick the shortest
    // (most likely the actual repo root, not a worktree or subdir).
    let mut url_to_canonical: HashMap<String, PathBuf> = HashMap::new();
    for (_, id) in &resolved {
        if let Some(url) = &id.remote_url {
            let entry = url_to_canonical
                .entry(url.clone())
                .or_insert_with(|| id.root_path.clone());
            if id.root_path.as_os_str().len() < entry.as_os_str().len() {
                *entry = id.root_path.clone();
            }
        }
    }

    // Phase 3: assign each source to a group key (canonical root)
    let mut groups_map: HashMap<PathBuf, (Option<String>, Vec<ProjectSource>)> = HashMap::new();

    for (source, id) in resolved {
        let group_key = if let Some(url) = &id.remote_url {
            url_to_canonical.get(url).cloned().unwrap_or(id.root_path)
        } else {
            id.root_path
        };

        let entry = groups_map
            .entry(group_key)
            .or_insert_with(|| (id.remote_url.clone(), Vec::new()));
        entry.1.push(source);
    }

    // Phase 4: merge sources that share the same cwd
    for (_url, sources) in groups_map.values_mut() {
        let mut merged: Vec<ProjectSource> = Vec::new();
        for source in sources.drain(..) {
            if let Some(existing) = source
                .cwd
                .as_ref()
                .and_then(|cwd| merged.iter_mut().find(|m| m.cwd.as_ref() == Some(cwd)))
            {
                existing.session_files.extend(source.session_files);
            } else {
                merged.push(source);
            }
        }
        // Re-sort session files after merge
        for s in &mut merged {
            s.session_files.sort();
        }
        *sources = merged;
    }

    // Phase 5: build ProjectGroup vec
    let mut groups: Vec<ProjectGroup> = groups_map
        .into_iter()
        .map(|(root_path, (remote_url, sources))| {
            let total_sessions = sources.iter().map(|s| s.session_files.len()).sum();
            let name = derive_group_name(&root_path);

            ProjectGroup {
                name,
                root_path,
                remote_url,
                sources,
                total_sessions,
                override_info: None,
            }
        })
        .collect();

    groups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    groups
}

/// Derive a human-readable name from a root path.
pub fn derive_group_name(root: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let path_str = root.to_string_lossy();

    let relative = if let Some(stripped) = path_str.strip_prefix(home.to_string_lossy().as_ref()) {
        stripped.trim_start_matches('/')
    } else {
        &path_str
    };

    let parts: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        return root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.to_string());
    }

    if parts.len() <= 2 {
        parts.join("/")
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_root_worktrees_segment() {
        assert_eq!(
            heuristic_root(
                "/Users/hmenzagh/oppus/GitHubCode/Francis-Monorepo/Worktrees/Francis/feat/ai"
            ),
            PathBuf::from("/Users/hmenzagh/oppus/GitHubCode/Francis-Monorepo")
        );
    }

    #[test]
    fn test_heuristic_root_regular() {
        assert_eq!(
            heuristic_root("/Users/hmenzagh/code/CCMeter"),
            PathBuf::from("/Users/hmenzagh/code/CCMeter")
        );
    }

    #[test]
    fn test_derive_group_name() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            derive_group_name(&home.join("code/CCMeter")),
            "code/CCMeter"
        );
        assert_eq!(
            derive_group_name(&home.join("oppus/GitHubCode/Francis-Monorepo/Francis")),
            "Francis-Monorepo/Francis"
        );
    }

    #[test]
    fn test_discover_runs() {
        let (groups, _, _) = discover_project_groups_with_root_map();
        for g in &groups {
            assert!(!g.name.is_empty());
            assert!(g.total_sessions > 0);
        }
    }
}
