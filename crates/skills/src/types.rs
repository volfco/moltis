use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a skill was discovered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// Project-local: `<cwd>/.moltis/skills/`
    Project,
    /// Personal: `~/.moltis/skills/`
    Personal,
    /// Bundled inside a plugin directory.
    Plugin,
    /// Installed from a registry (e.g. skills.sh).
    Registry,
}

/// Lightweight metadata parsed from SKILL.md frontmatter.
/// Loaded at startup for all discovered skills (cheap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name — lowercase, hyphens allowed, 1-64 chars.
    pub name: String,
    /// Short human-readable description.
    #[serde(default)]
    pub description: String,
    /// SPDX license identifier.
    #[serde(default)]
    pub license: Option<String>,
    /// Tools this skill is allowed to use.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Filesystem path to the skill directory.
    #[serde(skip)]
    pub path: PathBuf,
    /// Where this skill was discovered.
    #[serde(skip)]
    pub source: Option<SkillSource>,
}

/// Full skill content: metadata + markdown body.
/// Loaded on demand when a skill is activated.
#[derive(Debug, Clone)]
pub struct SkillContent {
    pub metadata: SkillMetadata,
    pub body: String,
}
