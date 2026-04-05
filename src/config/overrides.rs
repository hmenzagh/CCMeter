use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::discovery::{self, OverrideInfo, ProjectGroup, ProjectSource};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DisplaySettings {
    /// When true, heatmap cells scale to fill the panel width.
    #[serde(default)]
    pub expanded_heatmap: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Overrides {
    #[serde(default)]
    pub merges: Vec<Vec<String>>,
    #[serde(default)]
    pub splits: HashSet<String>,
    /// Individual sources (by cwd) extracted from their group.
    #[serde(default)]
    pub source_extracts: HashSet<String>,
    #[serde(default)]
    pub stars: HashSet<String>,
    #[serde(default)]
    pub hidden: HashSet<String>,
    #[serde(default)]
    pub renames: HashMap<String, String>,
    #[serde(default)]
    pub display: DisplaySettings,
}

impl Overrides {
    pub fn load() -> Self {
        let path = config_path();
        if !path.exists() {
            return Self::default();
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn add_merge(&mut self, a: &str, b: &str) {
        let mut a_idx = None;
        let mut b_idx = None;
        for (i, set) in self.merges.iter().enumerate() {
            if set.iter().any(|p| p == a) {
                a_idx = Some(i);
            }
            if set.iter().any(|p| p == b) {
                b_idx = Some(i);
            }
        }

        match (a_idx, b_idx) {
            (Some(ai), Some(bi)) if ai == bi => {}
            (Some(ai), Some(bi)) => {
                let (lo, hi) = if ai < bi { (ai, bi) } else { (bi, ai) };
                let hi_set = self.merges.remove(hi);
                let lo_set = &mut self.merges[lo];
                lo_set.extend(hi_set);
                lo_set.sort();
                lo_set.dedup();
            }
            (Some(ai), None) => {
                self.merges[ai].push(b.to_string());
            }
            (None, Some(bi)) => {
                self.merges[bi].push(a.to_string());
            }
            (None, None) => {
                self.merges.push(vec![a.to_string(), b.to_string()]);
            }
        }
    }

    pub fn add_split(&mut self, root_path: &str) {
        self.splits.insert(root_path.to_string());
        self.remove_from_merges(root_path);
    }

    pub fn remove_overrides_for(&mut self, root_path: &str) {
        self.splits.remove(root_path);
        self.remove_from_merges(root_path);
    }

    fn remove_from_merges(&mut self, root_path: &str) {
        for set in &mut self.merges {
            set.retain(|p| p != root_path);
        }
        self.merges.retain(|set| set.len() >= 2);
    }

    pub fn toggle_star(&mut self, root_path: &str) {
        if !self.stars.remove(root_path) {
            self.stars.insert(root_path.to_string());
            self.hidden.remove(root_path);
        }
    }

    pub fn toggle_hidden(&mut self, root_path: &str) {
        if !self.hidden.remove(root_path) {
            self.hidden.insert(root_path.to_string());
            self.stars.remove(root_path);
        }
    }

    pub fn is_starred(&self, root_path: &str) -> bool {
        self.stars.contains(root_path)
    }

    pub fn is_hidden(&self, root_path: &str) -> bool {
        self.hidden.contains(root_path)
    }

    pub fn rename(&mut self, root_path: &str, new_name: &str) {
        if new_name.is_empty() {
            self.renames.remove(root_path);
        } else {
            self.renames
                .insert(root_path.to_string(), new_name.to_string());
        }
    }

    pub fn get_name(&self, root_path: &str) -> Option<&str> {
        self.renames.get(root_path).map(|s| s.as_str())
    }

    pub fn extract_source(&mut self, cwd: &str) {
        self.source_extracts.insert(cwd.to_string());
    }

    pub fn reset_all(&mut self) {
        *self = Self::default();
    }
}

fn config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config").join("ccmeter").join("overrides.json")
}

// ---------------------------------------------------------------------------
// Apply overrides to discovered groups
// ---------------------------------------------------------------------------

fn make_split_group(
    source: ProjectSource,
    original_root: PathBuf,
    remote_url: Option<String>,
) -> ProjectGroup {
    let root = source.effective_root();
    let name = discovery::derive_group_name(&root);
    let total_sessions = source.session_files.len();
    ProjectGroup {
        name,
        root_path: root,
        remote_url,
        sources: vec![source],
        total_sessions,
        override_info: Some(OverrideInfo::Split { original_root }),
    }
}

pub fn apply_overrides(groups: &[ProjectGroup], overrides: &mut Overrides) -> Vec<ProjectGroup> {
    let has_structural = !overrides.merges.is_empty()
        || !overrides.splits.is_empty()
        || !overrides.source_extracts.is_empty();
    let has_display = !overrides.stars.is_empty()
        || !overrides.hidden.is_empty()
        || !overrides.renames.is_empty();

    if !has_structural && !has_display {
        return groups.to_vec();
    }

    // Phase 1: extract individual sources by cwd
    let mut groups: Vec<ProjectGroup> = groups.to_vec();
    if !overrides.source_extracts.is_empty() {
        let mut result: Vec<ProjectGroup> = Vec::new();
        for mut group in groups.drain(..) {
            let mut kept: Vec<ProjectSource> = Vec::new();
            let mut extracted: Vec<ProjectSource> = Vec::new();
            for source in group.sources.drain(..) {
                if source
                    .cwd
                    .as_ref()
                    .is_some_and(|c| overrides.source_extracts.contains(c.as_str()))
                {
                    extracted.push(source);
                } else {
                    kept.push(source);
                }
            }
            // Create standalone groups for extracted sources
            for source in extracted {
                result.push(make_split_group(
                    source,
                    group.root_path.clone(),
                    group.remote_url.clone(),
                ));
            }
            // Keep the remainder of the group (if any sources left)
            if !kept.is_empty() {
                group.total_sessions = kept.iter().map(|s| s.session_files.len()).sum();
                group.sources = kept;
                result.push(group);
            }
        }
        groups = result;
    }

    // Phase 2: apply full splits
    let mut working: Vec<ProjectGroup> = Vec::new();
    for group in groups {
        let key = group.root_key();
        if overrides.splits.contains(&key) && group.sources.len() > 1 {
            let original_root = PathBuf::from(&key);
            for source in group.sources {
                working.push(make_split_group(
                    source,
                    original_root.clone(),
                    group.remote_url.clone(),
                ));
            }
        } else {
            working.push(group);
        }
    }

    // Phase 3: apply merges
    for merge_set in &overrides.merges {
        let set: HashSet<&str> = merge_set.iter().map(|s| s.as_str()).collect();

        let mut to_merge: Vec<ProjectGroup> = Vec::new();
        let mut keep: Vec<ProjectGroup> = Vec::new();

        for g in working.drain(..) {
            if set.contains(g.root_key().as_str()) {
                to_merge.push(g);
            } else {
                keep.push(g);
            }
        }

        working = keep;

        if to_merge.len() > 1 {
            let mut combined_sources: Vec<ProjectSource> = Vec::new();
            let mut best_root = to_merge[0].root_path.clone();
            let mut best_remote: Option<String> = None;

            // Collect keys of all merged projects
            let merged_keys: Vec<String> = to_merge.iter().map(|g| g.root_key()).collect();

            for g in &to_merge {
                combined_sources.extend(g.sources.clone());
                if g.root_path.as_os_str().len() < best_root.as_os_str().len() {
                    best_root = g.root_path.clone();
                }
                if best_remote.is_none() {
                    best_remote.clone_from(&g.remote_url);
                }
            }

            let best_key = best_root.to_string_lossy().to_string();

            // Propagate overrides: rename > star > visible
            // Rename: first found wins
            if !overrides.renames.contains_key(&best_key) {
                for k in &merged_keys {
                    if let Some(name) = overrides.renames.get(k).cloned() {
                        overrides.renames.insert(best_key.clone(), name);
                        break;
                    }
                }
            }
            // Star: if any source is starred → starred
            if !overrides.stars.contains(&best_key)
                && merged_keys.iter().any(|k| overrides.stars.contains(k))
            {
                overrides.stars.insert(best_key.clone());
            }
            // Hidden: only if ALL sources were hidden
            let all_hidden = merged_keys.iter().all(|k| overrides.hidden.contains(k));
            if all_hidden && !merged_keys.is_empty() {
                overrides.hidden.insert(best_key.clone());
            } else {
                overrides.hidden.remove(&best_key);
            }

            // Clean up overrides from old keys
            for k in &merged_keys {
                if k != &best_key {
                    overrides.renames.remove(k);
                    overrides.stars.remove(k);
                    overrides.hidden.remove(k);
                }
            }

            let total_sessions: usize =
                combined_sources.iter().map(|s| s.session_files.len()).sum();
            let name = discovery::derive_group_name(&best_root);

            working.push(ProjectGroup {
                name,
                root_path: best_root,
                remote_url: best_remote,
                sources: combined_sources,
                total_sessions,
                override_info: Some(OverrideInfo::Merged),
            });
        } else {
            working.extend(to_merge);
        }
    }

    // Apply renames
    for group in &mut working {
        let key = group.root_key();
        if let Some(custom_name) = overrides.get_name(&key) {
            group.name = custom_name.to_string();
        }
    }

    // Sort: starred first, hidden last, then alphabetical
    working.sort_by(|a, b| {
        let ak = a.root_key();
        let bk = b.root_key();
        let a_hidden = overrides.is_hidden(&ak);
        let b_hidden = overrides.is_hidden(&bk);
        let a_star = overrides.is_starred(&ak);
        let b_star = overrides.is_starred(&bk);

        match (a_hidden, b_hidden) {
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            _ => {}
        }
        match (a_star, b_star) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    working
}
