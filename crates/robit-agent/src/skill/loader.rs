//! Skill loading — each skill is a directory containing a `SKILL.md` file
//! with YAML frontmatter + Markdown body.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::skill::{Skill, SkillFrontmatter};

/// The expected main file name inside each skill directory.
const SKILL_FILE_NAME: &str = "SKILL.md";

// ============================================================================
// SkillLoadError
// ============================================================================

/// Error types that can occur during skill loading.
#[derive(Debug)]
pub enum SkillLoadError {
    IoError { path: PathBuf, source: io::Error },
    NoFrontmatter { path: PathBuf },
    NoClosingDelimiter { path: PathBuf },
    YamlParseError { path: PathBuf, source: serde_yaml::Error },
    MissingSkillFile { dir: PathBuf },
}

impl std::fmt::Display for SkillLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillLoadError::IoError { path, source } => {
                write!(f, "Failed to read file {}: {}", path.display(), source)
            }
            SkillLoadError::NoFrontmatter { path } => {
                write!(f, "File {} is missing YAML frontmatter (must start with ---)", path.display())
            }
            SkillLoadError::NoClosingDelimiter { path } => {
                write!(f, "File {} is missing closing --- delimiter", path.display())
            }
            SkillLoadError::YamlParseError { path, source } => {
                write!(f, "Failed to parse YAML in file {}: {}", path.display(), source)
            }
            SkillLoadError::MissingSkillFile { dir } => {
                write!(
                    f,
                    "Skill directory {} does not contain a {} file",
                    dir.display(),
                    SKILL_FILE_NAME
                )
            }
        }
    }
}

// ============================================================================
// Parse a single file
// ============================================================================

/// Parse a single skill `SKILL.md` file into a `Skill`.
///
/// Expected format:
/// ```text
/// ---
/// name: my-skill
/// description: ...
/// ---
///
/// # Markdown body...
/// ```
pub fn parse_skill_file(path: &Path) -> Result<Skill, SkillLoadError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| SkillLoadError::IoError { path: path.to_path_buf(), source: e })?;

    // Opening delimiter must be at the start of the file
    let rest = content
        .strip_prefix("---")
        .ok_or_else(|| SkillLoadError::NoFrontmatter { path: path.to_path_buf() })?;

    // Allow optional newline after opening ---
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);

    // Find closing delimiter --- on its own line
    let closing_pos = find_closing_delimiter(rest)
        .ok_or_else(|| SkillLoadError::NoClosingDelimiter { path: path.to_path_buf() })?;

    let yaml_text = &rest[..closing_pos];
    // Skip past "---" and any trailing whitespace
    let markdown_body = &rest[closing_pos + 3..];
    let markdown_body = markdown_body.strip_prefix('\r').unwrap_or(markdown_body);
    let markdown_body = markdown_body.strip_prefix('\n').unwrap_or(markdown_body);

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_text.trim())
        .map_err(|e| SkillLoadError::YamlParseError { path: path.to_path_buf(), source: e })?;

    Ok(Skill {
        frontmatter,
        content: markdown_body.to_string(),
        source_path: path.to_path_buf(),
    })
}

/// Find the position of a closing `---` delimiter that is on its own line.
fn find_closing_delimiter(text: &str) -> Option<usize> {
    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find('\n') {
        let abs_pos = search_start + pos;
        if let Some(line_start) = text.get(abs_pos + 1..) {
            let line = line_start.lines().next().unwrap_or("");
            if line.trim_end() == "---" {
                return Some(abs_pos + 1);
            }
        }
        search_start = abs_pos + 1;
        if search_start >= text.len() {
            break;
        }
    }
    if text.starts_with("---") {
        return Some(0);
    }
    None
}

// ============================================================================
// Directory loading
// ============================================================================

/// Load all skills from both global and project skill directories.
///
/// Each skill is a subdirectory containing a `SKILL.md` file.
/// Project skills override global skills by name (same `name` field replaces).
/// Only skills with `enabled: true` (or default) are included.
///
/// Returns a tuple of (loaded skills, load errors). Errors are non-fatal —
/// the caller can log them and proceed with successfully loaded skills.
pub fn load_skills(
    global_dir: Option<PathBuf>,
    project_dir: Option<PathBuf>,
) -> (Vec<Skill>, Vec<SkillLoadError>) {
    let mut skills_by_name: HashMap<String, Skill> = HashMap::new();
    let mut errors: Vec<SkillLoadError> = Vec::new();

    // Load global skills first (lower priority)
    if let Some(dir) = global_dir {
        load_from_directory(&dir, &mut skills_by_name, &mut errors);
    }

    // Load project skills second (higher priority, overrides by name)
    if let Some(dir) = project_dir {
        load_from_directory(&dir, &mut skills_by_name, &mut errors);
    }

    // Filter out disabled skills
    let skills: Vec<Skill> = skills_by_name
        .into_values()
        .filter(|s| s.frontmatter.enabled)
        .collect();

    (skills, errors)
}

/// Load all skill subdirectories from a skills root directory.
///
/// Each subdirectory that contains a `SKILL.md` file is loaded as one skill.
/// Subdirectories are processed in sorted order for deterministic behavior.
fn load_from_directory(
    dir: &Path,
    skills_by_name: &mut HashMap<String, Skill>,
    errors: &mut Vec<SkillLoadError>,
) {
    if !dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(SkillLoadError::IoError {
                path: dir.to_path_buf(),
                source: e,
            });
            return;
        }
    };

    // Collect entries, then sort directories by name for deterministic loading
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for skill_dir in &dirs {
        let skill_file = skill_dir.join(SKILL_FILE_NAME);
        if !skill_file.exists() {
            errors.push(SkillLoadError::MissingSkillFile {
                dir: skill_dir.clone(),
            });
            continue;
        }

        match parse_skill_file(&skill_file) {
            Ok(skill) => {
                if skills_by_name.contains_key(&skill.frontmatter.name) {
                    tracing::info!(
                        "Skill '{}' ({}) overridden by {}",
                        skill.frontmatter.name,
                        skills_by_name[&skill.frontmatter.name]
                            .source_path
                            .display(),
                        skill_file.display()
                    );
                }
                skills_by_name.insert(skill.frontmatter.name.clone(), skill);
            }
            Err(e) => {
                errors.push(e);
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a skill directory with a SKILL.md file.
    fn write_skill_dir(parent: &Path, dir_name: &str, content: &str) -> PathBuf {
        let skill_dir = parent.join(dir_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let mut f = std::fs::File::create(skill_dir.join(SKILL_FILE_NAME)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        skill_dir
    }

    #[test]
    fn test_parse_valid_skill_file() {
        let content = r#"---
name: test-skill
description: A test skill
version: 2.0.0
triggers:
  - /test
enabled: true
---

# Test Skill

This is the body content."#;

        let temp_dir = std::env::temp_dir().join("robit_test_skills");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let skill_dir = write_skill_dir(&temp_dir, "test", content);
        let skill = parse_skill_file(&skill_dir.join(SKILL_FILE_NAME)).unwrap();

        assert_eq!(skill.frontmatter.name, "test-skill");
        assert_eq!(skill.frontmatter.description, "A test skill");
        assert_eq!(skill.frontmatter.version, "2.0.0");
        assert_eq!(skill.frontmatter.triggers, vec!["/test"]);
        assert!(skill.frontmatter.enabled);
        assert!(skill.content.contains("# Test Skill"));
        assert!(skill.content.contains("body content"));

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_parse_minimal_frontmatter() {
        let content = "---\nname: min\ndescription: minimal\n---\n\nBody";

        let temp_dir = std::env::temp_dir().join("robit_test_min");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let skill_dir = write_skill_dir(&temp_dir, "min", content);
        let skill = parse_skill_file(&skill_dir.join(SKILL_FILE_NAME)).unwrap();

        assert_eq!(skill.frontmatter.name, "min");
        assert_eq!(skill.frontmatter.description, "minimal");
        assert_eq!(skill.frontmatter.version, "1.0.0"); // default
        assert!(skill.frontmatter.triggers.is_empty()); // default
        assert!(skill.frontmatter.enabled); // default
        assert_eq!(skill.content, "\nBody");

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "# Just markdown, no frontmatter";

        let temp_dir = std::env::temp_dir().join("robit_test_nofm");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let skill_dir = write_skill_dir(&temp_dir, "nofm", content);
        let result = parse_skill_file(&skill_dir.join(SKILL_FILE_NAME));
        assert!(matches!(result, Err(SkillLoadError::NoFrontmatter { .. })));

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_parse_no_closing_delimiter() {
        let content = "---\nname: broken\n";

        let temp_dir = std::env::temp_dir().join("robit_test_nocl");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let skill_dir = write_skill_dir(&temp_dir, "nocl", content);
        let result = parse_skill_file(&skill_dir.join(SKILL_FILE_NAME));
        assert!(matches!(result, Err(SkillLoadError::NoClosingDelimiter { .. })));

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let content = "---\nname: [broken yaml\n---\n\nBody";

        let temp_dir = std::env::temp_dir().join("robit_test_badym");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let skill_dir = write_skill_dir(&temp_dir, "badym", content);
        let result = parse_skill_file(&skill_dir.join(SKILL_FILE_NAME));
        assert!(matches!(result, Err(SkillLoadError::YamlParseError { .. })));

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_skills_with_priority() {
        let temp = std::env::temp_dir().join("robit_test_priority");
        let global_dir = temp.join("global");
        let project_dir = temp.join("project");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::fs::create_dir_all(&project_dir).unwrap();

        // Global skill "shared"
        write_skill_dir(
            &global_dir,
            "shared",
            "---\nname: shared\ndescription: global version\n---\n\nglobal body",
        );
        // Another global skill
        write_skill_dir(
            &global_dir,
            "only-global",
            "---\nname: only-global\ndescription: only in global\n---\n\nonly body",
        );
        // Project skill overrides global "shared"
        write_skill_dir(
            &project_dir,
            "shared",
            "---\nname: shared\ndescription: project version\n---\n\nproject body",
        );

        let (skills, errors) = load_skills(Some(global_dir), Some(project_dir));
        assert!(errors.is_empty());
        assert_eq!(skills.len(), 2);

        let shared = skills.iter().find(|s| s.frontmatter.name == "shared").unwrap();
        assert_eq!(shared.frontmatter.description, "project version");
        assert!(shared.content.contains("project body"));

        let only_global = skills
            .iter()
            .find(|s| s.frontmatter.name == "only-global")
            .unwrap();
        assert_eq!(only_global.frontmatter.description, "only in global");

        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn test_load_skills_filters_disabled() {
        let temp = std::env::temp_dir().join("robit_test_disabled");
        std::fs::create_dir_all(&temp).unwrap();

        write_skill_dir(
            &temp,
            "disabled",
            "---\nname: disabled\ndescription: should be filtered\nenabled: false\n---\n\nbody",
        );
        write_skill_dir(
            &temp,
            "enabled",
            "---\nname: enabled\ndescription: should be kept\n---\n\nbody",
        );

        let (skills, errors) = load_skills(Some(temp.clone()), None);
        assert!(errors.is_empty());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].frontmatter.name, "enabled");

        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn test_load_skills_missing_file_warned() {
        let temp = std::env::temp_dir().join("robit_test_miss");
        std::fs::create_dir_all(&temp).unwrap();

        // Directory without SKILL.md
        std::fs::create_dir_all(temp.join("no-file")).unwrap();
        // Valid skill
        write_skill_dir(
            &temp,
            "valid",
            "---\nname: valid\ndescription: ok\n---\n\nbody",
        );

        let (skills, errors) = load_skills(Some(temp.clone()), None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].frontmatter.name, "valid");
        // The missing file should produce an error
        assert!(errors.iter().any(|e| matches!(e, SkillLoadError::MissingSkillFile { .. })));

        std::fs::remove_dir_all(temp).ok();
    }
}
