//! Skill registry — stores loaded skills and provides trigger matching.

use std::collections::{HashMap, HashSet};

use crate::skill::Skill;

/// Registry of loaded skills with trigger command matching.
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    /// Maps trigger command (e.g. "/review") → skill name.
    trigger_map: HashMap<String, String>,
}

impl SkillRegistry {
    /// Create a registry from a list of skills.
    ///
    /// Builds an internal trigger map for fast lookup and checks
    /// `tools_required` against the available tools, logging warnings for missing tools.
    pub fn new(skills: Vec<Skill>, available_tools: &[&str]) -> Self {
        let mut skill_map = HashMap::new();
        let mut trigger_map = HashMap::new();
        let tool_set: HashSet<&str> = available_tools.iter().copied().collect();

        for skill in skills {
            // Warn about missing required tools
            for required in &skill.frontmatter.tools_required {
                if !tool_set.contains(required.as_str()) {
                    tracing::warn!(
                        "技能 '{}' 需要工具 '{}' 但该工具未启用",
                        skill.frontmatter.name,
                        required
                    );
                }
            }

            // Build trigger map — warn on duplicate triggers
            for trigger in &skill.frontmatter.triggers {
                if let Some(existing_name) = trigger_map.get(trigger) {
                    tracing::warn!(
                        "技能触发 '{}' 已被技能 '{}' 使用，现被 '{}' 覆盖",
                        trigger,
                        existing_name,
                        skill.frontmatter.name
                    );
                }
                trigger_map.insert(trigger.clone(), skill.frontmatter.name.clone());
            }

            skill_map.insert(skill.frontmatter.name.clone(), skill);
        }

        Self {
            skills: skill_map,
            trigger_map,
        }
    }

    /// Get a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Match a user input against registered trigger commands.
    ///
    /// Uses longest-match priority: if both `/review` and `/review-quick` are registered,
    /// `/review-quick src/main.rs` matches the longer trigger.
    ///
    /// Trigger must be followed by whitespace or end-of-string.
    /// Returns the matched skill and any remaining arguments.
    pub fn match_trigger(&self, input: &str) -> Option<(&Skill, String)> {
        let mut best_trigger: Option<&str> = None;

        for trigger in self.trigger_map.keys() {
            if input.starts_with(trigger.as_str()) {
                let rest = &input[trigger.len()..];
                // Must be followed by whitespace or end-of-string
                if rest.is_empty() || rest.starts_with(' ') {
                    // Pick the longest matching trigger
                    if best_trigger.is_none() || trigger.len() > best_trigger.unwrap().len() {
                        best_trigger = Some(trigger);
                    }
                }
            }
        }

        best_trigger.and_then(|trigger| {
            let skill_name = self.trigger_map.get(trigger)?;
            let skill = self.skills.get(skill_name.as_str())?;
            let args = input[trigger.len()..].trim().to_string();
            Some((skill, args))
        })
    }

    /// Get all skill names and descriptions for system prompt injection.
    pub fn skill_descriptions(&self) -> Vec<(&str, &str)> {
        self.skills
            .values()
            .map(|s| {
                (
                    s.frontmatter.name.as_str(),
                    s.frontmatter.description.as_str(),
                )
            })
            .collect()
    }

    /// Get all skill names.
    pub fn skill_names(&self) -> Vec<&str> {
        self.skills.keys().map(|s| s.as_str()).collect()
    }

    /// Number of enabled skills.
    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// Get references to all skills.
    pub fn skills(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::SkillFrontmatter;
    use std::path::PathBuf;

    fn make_skill(name: &str, triggers: Vec<&str>) -> Skill {
        Skill {
            frontmatter: SkillFrontmatter {
                name: name.to_string(),
                description: format!("Description for {}", name),
                version: "1.0.0".to_string(),
                triggers: triggers.into_iter().map(String::from).collect(),
                tools_required: vec![],
                enabled: true,
            },
            content: format!("Content for {}", name),
            source_path: PathBuf::from("test.md"),
        }
    }

    #[test]
    fn test_match_trigger_exact() {
        let skills = vec![
            make_skill("code-review", vec!["/review"]),
            make_skill("refactor", vec!["/refactor"]),
        ];
        let registry = SkillRegistry::new(skills, &[]);

        let (skill, args) = registry.match_trigger("/review").unwrap();
        assert_eq!(skill.frontmatter.name, "code-review");
        assert_eq!(args, "");
    }

    #[test]
    fn test_match_trigger_with_args() {
        let skills = vec![make_skill("code-review", vec!["/review"])];
        let registry = SkillRegistry::new(skills, &[]);

        let (skill, args) = registry.match_trigger("/review src/main.rs").unwrap();
        assert_eq!(skill.frontmatter.name, "code-review");
        assert_eq!(args, "src/main.rs");
    }

    #[test]
    fn test_match_trigger_longest_match() {
        let skills = vec![
            make_skill("review", vec!["/review"]),
            make_skill("review-quick", vec!["/review-quick"]),
        ];
        let registry = SkillRegistry::new(skills, &[]);

        let (skill, _) = registry.match_trigger("/review-quick src/main.rs").unwrap();
        assert_eq!(skill.frontmatter.name, "review-quick");
    }

    #[test]
    fn test_match_trigger_partial_not_matched() {
        let skills = vec![make_skill("review", vec!["/review"])];
        let registry = SkillRegistry::new(skills, &[]);

        // /reviews should NOT match /review (no space after trigger)
        assert!(registry.match_trigger("/reviews").is_none());
    }

    #[test]
    fn test_match_trigger_no_match() {
        let skills = vec![make_skill("review", vec!["/review"])];
        let registry = SkillRegistry::new(skills, &[]);

        assert!(registry.match_trigger("/unknown").is_none());
        assert!(registry.match_trigger("plain text").is_none());
    }

    #[test]
    fn test_skill_descriptions() {
        let skills = vec![
            make_skill("review", vec!["/review"]),
            make_skill("refactor", vec!["/refactor"]),
        ];
        let registry = SkillRegistry::new(skills, &["read", "bash"]);

        let descs = registry.skill_descriptions();
        assert_eq!(descs.len(), 2);
        assert!(descs.iter().any(|(n, d)| *n == "review" && d.contains("review")));
    }

    #[test]
    fn test_warns_missing_tools() {
        // This test just verifies the warn! path is exercised.
        // In practice, tracing::warn! to stderr during tests is fine.
        let skills = vec![make_skill("review", vec!["/review"])];
        let mut front = skills[0].frontmatter.clone();
        front.tools_required = vec!["bash".to_string(), "nonexistent_tool".to_string()];
        let skill = Skill {
            frontmatter: front,
            ..skills[0].clone()
        };

        let _registry = SkillRegistry::new(vec![skill], &["bash", "read"]);
        // If no panic, the test passes — warnings are logged
    }
}
