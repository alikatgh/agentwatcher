//! Bounded, read-only discovery of installed instructions. Call from a worker.
use crate::telemetry::redact;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_DEPTH: usize = 10;
const MAX_DIRS: usize = 4096;
const MAX_ENTRIES: usize = 65_536;
const MAX_DOCUMENTS: usize = 512;
const DOCUMENT_BYTES: usize = 32 * 1024;
const TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub skills: Vec<SkillEntry>,
    pub profiles: Vec<AgentProfile>,
    pub guidance: Vec<CatalogGuidance>,
    pub roots: Vec<CatalogRoot>,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: String,
    pub path: String,
    pub text: String,
    pub package: String,
    pub version: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Default)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub model: String,
    pub workdir: String,
    pub instructions: String,
    pub scope: String,
    pub path: String,
}
/// Current instruction files arranged from the global location through workspace ancestors.
/// Presence on disk does not establish that a recorded agent read or followed a file.
#[derive(Clone, Debug, Default)]
pub struct CatalogGuidance {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub path: String,
    pub directory: String,
    pub description: String,
    pub text: String,
    pub truncated: bool,
    pub order: usize,
}
#[derive(Clone, Debug, Default)]
pub struct CatalogRoot {
    pub path: String,
    pub scope: String,
    pub available: bool,
}

struct Budget {
    started: Instant,
    duration: Duration,
    directories: usize,
    entries: usize,
    documents: usize,
    bytes: usize,
}
impl Budget {
    fn new(duration: Duration) -> Self {
        Self {
            started: Instant::now(),
            duration,
            directories: 0,
            entries: 0,
            documents: 0,
            bytes: 0,
        }
    }
    fn exhausted(&self) -> bool {
        self.started.elapsed() >= self.duration
            || self.directories >= MAX_DIRS
            || self.entries >= MAX_ENTRIES
            || self.documents >= MAX_DOCUMENTS
    }
}

/// Discover only the known user/project skill and agent-profile locations.
/// No configuration, authentication, provider calls, or executable files are loaded.
pub fn collect(project: &str) -> Catalog {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Catalog {
            warnings: vec!["Home folder is unavailable; local catalog was not scanned.".into()],
            ..Default::default()
        };
    };
    collect_with_admin(
        &home,
        Path::new(project),
        Path::new("/etc/codex/skills"),
        Duration::from_secs(2),
    )
}

fn collect_with_admin(home: &Path, project: &Path, admin: &Path, duration: Duration) -> Catalog {
    let mut catalog = Catalog::default();
    let mut budget = Budget::new(duration);
    let mut roots = Vec::new();
    if project.is_absolute() {
        roots.push((project.join(".agents/skills"), "Project", false));
        roots.push((project.join(".codex/skills"), "Project", false));
        roots.push((project.join(".agents/agents"), "Project", true));
        roots.push((project.join(".codex/agents"), "Project", true));
    }
    roots.extend([
        (home.join(".codex/agents"), "Global", true),
        (home.join(".agents/agents"), "Global", true),
        (home.join(".codex/skills/.system"), "System", false),
        (admin.to_path_buf(), "Admin", false),
        (home.join(".codex/skills"), "Global", false),
        (home.join(".agents/skills"), "Global", false),
        (home.join(".codex/plugins/cache"), "Plugin", false),
    ]);
    let mut seen = HashSet::new();
    collect_guidance(home, project, &mut catalog, &mut budget, &mut seen);
    for (root, scope, profiles) in roots {
        let available = root.is_dir();
        catalog.roots.push(CatalogRoot {
            path: root.to_string_lossy().into_owned(),
            scope: scope.into(),
            available,
        });
        if !available || budget.exhausted() {
            continue;
        }
        let Ok(canonical_root) = root.canonicalize() else {
            continue;
        };
        let mut pending = VecDeque::from([(canonical_root.clone(), 0usize)]);
        while let Some((directory, depth)) = pending.pop_front() {
            if budget.exhausted() {
                break;
            }
            budget.directories += 1;
            let Ok(entries) = fs::read_dir(&directory) else {
                catalog
                    .warnings
                    .push(format!("Could not read {}", directory.display()));
                continue;
            };
            // Bound enumeration as well as directory depth, files, bytes, and time.
            for entry in entries {
                if budget.exhausted() {
                    break;
                }
                budget.entries += 1;
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_symlink() {
                    if !profiles {
                        // A directly discovered linked skill is one document, never a new scan root.
                        if let Ok(target) = path.canonicalize() {
                            if target.is_dir() {
                                let document = target.join("SKILL.md");
                                add_document(
                                    &document,
                                    &target,
                                    scope,
                                    false,
                                    &mut seen,
                                    &mut budget,
                                    &mut catalog,
                                );
                            }
                        }
                    }
                    continue;
                }
                if kind.is_dir() && depth < MAX_DEPTH && !profiles {
                    let name = entry.file_name();
                    if !(scope == "Global" && name == ".system")
                        && !matches!(
                            name.to_str(),
                            Some(".git" | "node_modules" | "target" | "__pycache__")
                        )
                    {
                        pending.push_back((path, depth + 1));
                    }
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let wanted = if profiles {
                    path.extension().is_some_and(|v| v == "toml")
                } else {
                    entry.file_name() == "SKILL.md"
                };
                if !wanted {
                    continue;
                }
                add_document(
                    &path,
                    &canonical_root,
                    scope,
                    profiles,
                    &mut seen,
                    &mut budget,
                    &mut catalog,
                );
            }
        }
    }
    if budget.exhausted() {
        catalog.warnings.push("Catalog scan reached its time or item limit. Refresh to retry; all configured roots are listed.".into());
    }
    if budget.bytes >= TOTAL_BYTES {
        catalog.warnings.push("Instruction previews reached the 8 MiB content limit. Source files remain available through Reveal source.".into());
    }
    catalog.skills.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.scope.cmp(&b.scope))
            .then(a.path.cmp(&b.path))
    });
    catalog.profiles.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.path.cmp(&b.path))
    });
    catalog
}

fn add_document(
    path: &Path,
    root: &Path,
    scope: &str,
    profiles: bool,
    seen: &mut HashSet<PathBuf>,
    budget: &mut Budget,
    catalog: &mut Catalog,
) {
    if budget.exhausted()
        || !fs::symlink_metadata(path)
            .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
    {
        return;
    }
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    if !canonical.starts_with(root) || !seen.insert(canonical.clone()) {
        return;
    }
    budget.documents += 1;
    let remaining = TOTAL_BYTES.saturating_sub(budget.bytes);
    let Ok((raw, truncated)) = read_document(&canonical, remaining.min(DOCUMENT_BYTES)) else {
        catalog
            .warnings
            .push(format!("Could not read {}", canonical.display()));
        return;
    };
    let mut text = redact(&raw);
    let truncated = truncated || text.len() > remaining.min(DOCUMENT_BYTES);
    truncate_bytes(&mut text, remaining.min(DOCUMENT_BYTES));
    budget.bytes += text.len();
    let source = canonical.to_string_lossy().into_owned();
    if profiles {
        if truncated {
            catalog
                .warnings
                .push(format!("Profile preview is limited: {source}"));
        }
        let fields = profile_fields(&text);
        let fallback = canonical
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        catalog.profiles.push(AgentProfile {
            id: format!("profile:{source}"),
            name: fields
                .get("name")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or(fallback),
            description: fields.get("description").cloned().unwrap_or_default(),
            model: fields.get("model").cloned().unwrap_or_default(),
            workdir: fields
                .get("workdir")
                .or_else(|| fields.get("working_directory"))
                .cloned()
                .unwrap_or_default(),
            instructions: fields
                .get("instructions")
                .or_else(|| fields.get("developer_instructions"))
                .cloned()
                .unwrap_or_default(),
            scope: scope.into(),
            path: source,
        });
    } else {
        let (name, description) = skill_frontmatter(&text);
        let fallback = canonical
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (package, version) = plugin_identity(root, &canonical, scope);
        catalog.skills.push(SkillEntry {
            id: format!("skill:{source}"),
            name: if name.is_empty() { fallback } else { name },
            description,
            scope: scope.into(),
            path: source,
            text,
            package,
            version,
            truncated,
        });
    }
}

fn collect_guidance(
    home: &Path,
    project: &Path,
    catalog: &mut Catalog,
    budget: &mut Budget,
    seen: &mut HashSet<PathBuf>,
) {
    let mut locations = vec![(home.join(".codex"), "Global", "Global instruction location")];
    if project.is_absolute() {
        let mut ancestors: Vec<_> = project
            .ancestors()
            .take(32)
            .map(Path::to_path_buf)
            .collect();
        ancestors.reverse();
        for ancestor in ancestors {
            let description = if ancestor == project {
                "Current workspace instruction location"
            } else {
                "Ancestor of the current workspace"
            };
            locations.push((ancestor, "Project", description));
        }
    }
    for (directory, scope, description) in locations {
        for filename in ["AGENTS.md", "AGENTS.override.md"] {
            let path = directory.join(filename);
            let available = fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
            catalog.roots.push(CatalogRoot {
                path: path.to_string_lossy().into_owned(),
                scope: scope.into(),
                available,
            });
            if !available || budget.exhausted() {
                continue;
            }
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !seen.insert(canonical.clone()) {
                continue;
            }
            budget.documents += 1;
            let limit = TOTAL_BYTES.saturating_sub(budget.bytes).min(DOCUMENT_BYTES);
            let Ok((raw, mut truncated)) = read_document(&canonical, limit) else {
                continue;
            };
            let mut text = redact(&raw);
            truncated |= text.len() > limit;
            truncate_bytes(&mut text, limit);
            budget.bytes += text.len();
            let source = canonical.to_string_lossy().into_owned();
            catalog.guidance.push(CatalogGuidance {
                id: format!("instruction:{source}"),
                name: filename.into(),
                scope: scope.into(),
                path: source,
                directory: directory.to_string_lossy().into_owned(),
                description: format!(
                    "{description}. Current file on disk; not proof a recorded agent read it."
                ),
                text,
                truncated,
                order: catalog.guidance.len(),
            });
        }
    }
}

#[cfg(test)]
fn collect_at(home: &Path, project: &Path, duration: Duration) -> Catalog {
    collect_with_admin(home, project, &home.join("administration/skills"), duration)
}

fn read_document(path: &Path, limit: usize) -> std::io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let oversized = file.metadata()?.len() > limit as u64;
    file.take(limit as u64).read_to_end(&mut bytes)?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), oversized))
}
fn truncate_bytes(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut boundary = limit;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
    }
}
fn plugin_identity(root: &Path, path: &Path, scope: &str) -> (String, String) {
    if scope != "Plugin" {
        return (String::new(), String::new());
    }
    let components: Vec<_> = path
        .strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    // Cache is publisher/package/version/...; preserve versions instead of silently picking one.
    let package = components
        .iter()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    let version = components
        .get(2)
        .copied()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("")
        .to_owned();
    (package, version)
}
fn scalar(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str::<String>(value).unwrap_or_else(|_| value.trim_matches('"').into())
    } else {
        value.trim_matches('\'').to_owned()
    }
}
fn skill_frontmatter(text: &str) -> (String, String) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (String::new(), String::new());
    }
    let mut name = String::new();
    let mut description = String::new();
    let mut description_block = false;
    for line in lines.take(128) {
        if line.trim() == "---" {
            break;
        }
        if description_block && (line.starts_with(' ') || line.starts_with('\t')) {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(line.trim());
            continue;
        }
        description_block = false;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "name" => name = scalar(value),
            "description" => {
                description_block = matches!(value.trim(), "|" | ">" | "|-" | ">-");
                if !description_block {
                    description = scalar(value);
                }
            }
            _ => {}
        }
    }
    (redact(&name), redact(&description))
}
fn profile_fields(text: &str) -> std::collections::HashMap<String, String> {
    let mut fields = std::collections::HashMap::new();
    let mut multiline: Option<(String, &'static str, String)> = None;
    for line in text.lines() {
        if let Some((key, delimiter, value)) = multiline.as_mut() {
            if let Some((before, _)) = line.split_once(*delimiter) {
                value.push_str(before);
                fields.insert(key.clone(), value.clone());
                multiline = None;
            } else {
                value.push_str(line);
                value.push('\n');
            }
            continue;
        }
        // Only root-level whitelisted fields. Nested config and credentials are never exposed.
        if line.trim_start().starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !matches!(
            key,
            "name"
                | "description"
                | "model"
                | "workdir"
                | "working_directory"
                | "instructions"
                | "developer_instructions"
        ) {
            continue;
        }
        let value = value.trim();
        let delimiter = if value.starts_with("\"\"\"") {
            Some("\"\"\"")
        } else if value.starts_with("'''") {
            Some("'''")
        } else {
            None
        };
        if let Some(delimiter) = delimiter {
            let rest = &value[3..];
            if let Some((value, _)) = rest.split_once(delimiter) {
                fields.insert(key.into(), value.into());
            } else {
                multiline = Some((key.into(), delimiter, rest.to_owned()));
            }
        } else {
            fields.insert(key.into(), scalar(value));
        }
    }
    if let Some((key, _, value)) = multiline {
        fields.insert(key, value);
    }
    for value in fields.values_mut() {
        *value = redact(value);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agentwatcher-catalog-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scans_known_roots_and_linked_skill_documents_without_traversing_targets() {
        let fixture = Fixture::new();
        fixture.write(
            "home/.codex/skills/tool/SKILL.md",
            "---\nname: Tool\ndescription: Global instructions\n---\nGlobal body",
        );
        fixture.write(
            "project/.agents/skills/tool/SKILL.md",
            "---\nname: Tool\ndescription: Project instructions\n---\nProject body",
        );
        fixture.write(
            "home/.codex/plugins/cache/publisher/tool/1.0/skills/x/SKILL.md",
            "---\nname: Plugin tool\n---\nv1",
        );
        fixture.write(
            "home/.codex/plugins/cache/publisher/tool/2.0/skills/x/SKILL.md",
            "---\nname: Plugin tool\n---\nv2",
        );
        let outside = fixture.write("outside/SKILL.md", "---\nname: Linked instructions\n---");
        fixture.write(
            "outside/nested/SKILL.md",
            "---\nname: Must never appear\n---",
        );
        fixture.write(
            "home/.codex/config.toml",
            "instructions = 'Do not collect configuration'",
        );
        symlink(&outside, fixture.0.join("home/.codex/skills/escaped.md")).unwrap();
        symlink(
            outside.parent().unwrap(),
            fixture.0.join("home/.codex/skills/escape"),
        )
        .unwrap();
        let catalog = collect_at(
            &fixture.0.join("home"),
            &fixture.0.join("project"),
            Duration::from_secs(2),
        );
        assert_eq!(catalog.skills.len(), 5);
        assert_eq!(
            catalog
                .skills
                .iter()
                .filter(|s| s.scope == "Global")
                .count(),
            2
        );
        assert_eq!(
            catalog
                .skills
                .iter()
                .filter(|s| s.scope == "Project")
                .count(),
            1
        );
        assert!(catalog.skills.iter().any(|s| s.version == "1.0"));
        assert!(catalog.skills.iter().any(|s| s.version == "2.0"));
        assert!(
            catalog
                .skills
                .iter()
                .all(|s| !s.text.contains("Must never appear"))
        );
        assert!(catalog.profiles.is_empty());
        assert!(catalog.roots.iter().any(|root| root.scope == "Admin"));
        assert!(catalog.roots.iter().any(|root| root.scope == "System"));
    }

    #[test]
    fn instruction_hierarchy_is_current_source_evidence_in_directory_order() {
        let fixture = Fixture::new();
        let global = fixture.write(
            "home/.codex/AGENTS.md",
            "Global rules\nAPI_KEY=hidden-value",
        );
        let ancestor = fixture.write("project/AGENTS.md", "Repository rules");
        let local = fixture.write("project/sub/AGENTS.md", "Workspace rules");
        let override_file =
            fixture.write("project/sub/AGENTS.override.md", "Current override rules");
        fixture.write(
            "project/sub/unrelated/AGENTS.md",
            "Must not scan descendants",
        );
        let catalog = collect_at(
            &fixture.0.join("home"),
            &fixture.0.join("project/sub"),
            Duration::from_secs(2),
        );
        let paths: Vec<_> = catalog
            .guidance
            .iter()
            .map(|entry| PathBuf::from(&entry.path))
            .collect();
        let positions: Vec<_> = [&global, &ancestor, &local, &override_file]
            .iter()
            .map(|path| {
                let canonical = path.canonicalize().unwrap();
                paths
                    .iter()
                    .position(|candidate| candidate == &canonical)
                    .unwrap_or_else(|| {
                        panic!("Missing instruction source: {}", canonical.display())
                    })
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            catalog
                .guidance
                .iter()
                .all(|entry| entry.description.contains("not proof"))
        );
        assert!(
            !catalog
                .guidance
                .iter()
                .any(|entry| entry.text.contains("Must not scan descendants"))
        );
        assert!(
            !catalog
                .guidance
                .iter()
                .any(|entry| entry.text.contains("hidden-value"))
        );
    }

    #[test]
    fn system_admin_and_linked_skills_keep_scope_without_recursion_or_duplicates() {
        let fixture = Fixture::new();
        fixture.write(
            "home/.codex/skills/.system/internal/SKILL.md",
            "---\nname: System tool\n---",
        );
        fixture.write(
            "home/administration/skills/policy/SKILL.md",
            "---\nname: Admin tool\n---",
        );
        let external = fixture.write(
            "linked/tool/SKILL.md",
            "---\nname: Linked tool\n---\nLinked instructions",
        );
        fixture.write(
            "linked/tool/nested/SKILL.md",
            "---\nname: Nested external directory\n---",
        );
        fs::create_dir_all(fixture.0.join("home/.agents/skills")).unwrap();
        for alias in ["one", "two"] {
            symlink(
                external.parent().unwrap(),
                fixture.0.join(format!("home/.agents/skills/{alias}")),
            )
            .unwrap();
        }
        symlink(
            fixture.0.join("home/.agents/skills"),
            fixture.0.join("home/.agents/skills/cycle"),
        )
        .unwrap();
        let catalog = collect_at(
            &fixture.0.join("home"),
            Path::new(""),
            Duration::from_secs(2),
        );
        assert_eq!(catalog.skills.len(), 3);
        assert_eq!(
            catalog
                .skills
                .iter()
                .find(|entry| entry.name == "System tool")
                .unwrap()
                .scope,
            "System"
        );
        assert_eq!(
            catalog
                .skills
                .iter()
                .find(|entry| entry.name == "Admin tool")
                .unwrap()
                .scope,
            "Admin"
        );
        assert_eq!(
            catalog
                .skills
                .iter()
                .filter(|entry| entry.name == "Linked tool")
                .count(),
            1
        );
        assert!(
            !catalog
                .skills
                .iter()
                .any(|entry| entry.name == "Nested external directory")
        );
        assert!(catalog.warnings.is_empty());
    }

    #[test]
    fn parses_folded_descriptions_and_whitelisted_real_profile_instructions() {
        let (name, description) = skill_frontmatter(
            "---\nname: 'Safe tool'\ndescription: >-\n  First line\n  second line\nother: ignored\n---\nBody",
        );
        assert_eq!(name, "Safe tool");
        assert_eq!(description, "First line second line");
        let fields = profile_fields(
            "name = \"Reviewer\"\nmodel = \"local-model\"\ndeveloper_instructions = \"\"\"\nRead only.\nPreserve changes.\n\"\"\"\napi_key = \"not exposed\"\n[private]\ninstructions = \"ignored nested table\"",
        );
        assert_eq!(fields.get("name").unwrap(), "Reviewer");
        assert!(
            fields
                .get("developer_instructions")
                .unwrap()
                .contains("Preserve changes.")
        );
        assert!(!fields.contains_key("api_key"));
        assert!(!fields.contains_key("instructions"));
    }

    #[test]
    fn documents_are_bounded_redacted_and_deadline_is_visible() {
        let fixture = Fixture::new();
        fixture.write(
            "home/.codex/skills/large/SKILL.md",
            &format!(
                "---\nname: Large\n---\nAPI_KEY=private-value\n{}",
                "界".repeat(DOCUMENT_BYTES)
            ),
        );
        fixture.write("home/.codex/agents/reviewer.toml", "name = 'Reviewer'\ndeveloper_instructions = '''Never disclose API_KEY=private-value'''\npassword = 'excluded'");
        let catalog = collect_at(
            &fixture.0.join("home"),
            Path::new(""),
            Duration::from_secs(2),
        );
        assert_eq!(catalog.skills.len(), 1);
        assert!(catalog.skills[0].truncated);
        assert!(catalog.skills[0].text.len() <= DOCUMENT_BYTES);
        assert!(!catalog.skills[0].text.contains("private-value"));
        assert!(catalog.skills[0].text.contains("[redacted]"));
        assert_eq!(catalog.profiles.len(), 1);
        assert!(!catalog.profiles[0].instructions.contains("private-value"));
        assert!(!catalog.profiles[0].instructions.contains("excluded"));
        assert!(catalog.skills.iter().map(|s| s.text.len()).sum::<usize>() <= TOTAL_BYTES);
        let stopped = collect_at(&fixture.0.join("home"), Path::new(""), Duration::ZERO);
        assert!(stopped.skills.is_empty());
        assert!(!stopped.warnings.is_empty());
        assert!(stopped.roots.iter().any(|root| root.scope == "System"));
    }

    #[test]
    fn does_not_descend_beyond_the_declared_depth() {
        let fixture = Fixture::new();
        fixture.write(
            "home/.codex/skills/visible/SKILL.md",
            "---\nname: Visible\n---",
        );
        let deep = format!(
            "home/.codex/skills/{}SKILL.md",
            "nested/".repeat(MAX_DEPTH + 1)
        );
        fixture.write(&deep, "---\nname: Too deep\n---");
        let catalog = collect_at(
            &fixture.0.join("home"),
            Path::new(""),
            Duration::from_secs(2),
        );
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].name, "Visible");
    }

    #[test]
    fn rejects_file_symlinks_and_limits_utf8_content() {
        let fixture = Fixture::new();
        let source = fixture.write("source", "界界界");
        let link = fixture.0.join("link");
        symlink(&source, &link).unwrap();
        assert!(read_document(&link, DOCUMENT_BYTES).is_err());
        let (mut text, truncated) = read_document(&source, 5).unwrap();
        assert!(truncated);
        truncate_bytes(&mut text, 5);
        assert!(text.len() <= 5);
        assert!(text.is_char_boundary(text.len()));
    }
}
