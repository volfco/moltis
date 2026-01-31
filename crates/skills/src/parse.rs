use std::path::Path;

use anyhow::{Context, bail};

use crate::types::{SkillContent, SkillMetadata};

/// Validate a skill name: lowercase ASCII, hyphens, 1-64 chars.
pub fn validate_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Parse a SKILL.md file into metadata only (frontmatter).
pub fn parse_metadata(content: &str, skill_dir: &Path) -> anyhow::Result<SkillMetadata> {
    let (frontmatter, _body) = split_frontmatter(content)?;
    let mut meta: SkillMetadata =
        serde_yaml::from_str(&frontmatter).context("invalid SKILL.md frontmatter")?;

    if !validate_name(&meta.name) {
        bail!(
            "invalid skill name '{}': must be 1-64 lowercase alphanumeric/hyphen chars",
            meta.name
        );
    }

    meta.path = skill_dir.to_path_buf();
    Ok(meta)
}

/// Parse a SKILL.md file into full content (metadata + body).
pub fn parse_skill(content: &str, skill_dir: &Path) -> anyhow::Result<SkillContent> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let mut meta: SkillMetadata =
        serde_yaml::from_str(&frontmatter).context("invalid SKILL.md frontmatter")?;

    if !validate_name(&meta.name) {
        bail!(
            "invalid skill name '{}': must be 1-64 lowercase alphanumeric/hyphen chars",
            meta.name
        );
    }

    meta.path = skill_dir.to_path_buf();
    Ok(SkillContent {
        metadata: meta,
        body: body.to_string(),
    })
}

/// Split SKILL.md content at `---` delimiters into (frontmatter, body).
fn split_frontmatter(content: &str) -> anyhow::Result<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        bail!("SKILL.md must start with YAML frontmatter delimited by ---");
    }

    // Skip the opening ---
    let after_open = &trimmed[3..];
    let close_pos = after_open
        .find("\n---")
        .context("SKILL.md missing closing --- for frontmatter")?;

    let frontmatter = after_open[..close_pos].trim().to_string();
    let body = after_open[close_pos + 4..].trim().to_string();
    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_name() {
        assert!(validate_name("my-skill"));
        assert!(validate_name("a"));
        assert!(validate_name("skill123"));
        assert!(!validate_name(""));
        assert!(!validate_name("-bad"));
        assert!(!validate_name("bad-"));
        assert!(!validate_name("Bad"));
        assert!(!validate_name("has space"));
        assert!(!validate_name(&"a".repeat(65)));
    }

    #[test]
    fn test_parse_metadata() {
        let content = r#"---
name: my-skill
description: A test skill
license: MIT
allowed_tools:
  - exec
  - read
---

# My Skill

Instructions here.
"#;
        let meta = parse_metadata(content, Path::new("/tmp/my-skill")).unwrap();
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.description, "A test skill");
        assert_eq!(meta.license, Some("MIT".into()));
        assert_eq!(meta.allowed_tools, vec!["exec", "read"]);
        assert_eq!(meta.path, Path::new("/tmp/my-skill"));
    }

    #[test]
    fn test_parse_skill_full() {
        let content = r#"---
name: commit
description: Create git commits
---

When asked to commit, run `git add` then `git commit`.
"#;
        let skill = parse_skill(content, Path::new("/skills/commit")).unwrap();
        assert_eq!(skill.metadata.name, "commit");
        assert!(skill.body.contains("git add"));
    }

    #[test]
    fn test_invalid_name_rejected() {
        let content = "---\nname: Bad-Name\n---\nbody\n";
        assert!(parse_metadata(content, Path::new("/tmp")).is_err());
    }

    #[test]
    fn test_missing_frontmatter() {
        let content = "# No frontmatter\nJust markdown.";
        assert!(parse_metadata(content, Path::new("/tmp")).is_err());
    }

    #[test]
    fn test_missing_closing_delimiter() {
        let content = "---\nname: test\nno closing\n";
        assert!(parse_metadata(content, Path::new("/tmp")).is_err());
    }
}
