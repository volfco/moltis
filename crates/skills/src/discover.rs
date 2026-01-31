use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::{
    parse,
    types::{SkillMetadata, SkillSource},
};

/// Discovers skills from filesystem paths.
#[async_trait]
pub trait SkillDiscoverer: Send + Sync {
    /// Scan configured paths and return metadata for all discovered skills.
    async fn discover(&self) -> anyhow::Result<Vec<SkillMetadata>>;
}

/// Default filesystem-based skill discoverer.
pub struct FsSkillDiscoverer {
    /// (path, source) pairs to scan, in priority order.
    search_paths: Vec<(PathBuf, SkillSource)>,
}

impl FsSkillDiscoverer {
    pub fn new(search_paths: Vec<(PathBuf, SkillSource)>) -> Self {
        Self { search_paths }
    }

    /// Build the default search paths for skill discovery.
    pub fn default_paths(cwd: &Path) -> Vec<(PathBuf, SkillSource)> {
        let mut paths = vec![(cwd.join(".moltis/skills"), SkillSource::Project)];

        if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
            paths.push((home.join(".moltis/skills"), SkillSource::Personal));
            paths.push((home.join(".moltis/installed-skills"), SkillSource::Registry));
        }

        paths
    }
}

#[async_trait]
impl SkillDiscoverer for FsSkillDiscoverer {
    async fn discover(&self) -> anyhow::Result<Vec<SkillMetadata>> {
        let mut skills = Vec::new();

        for (base_path, source) in &self.search_paths {
            if !base_path.is_dir() {
                continue;
            }
            let entries = match std::fs::read_dir(base_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let skill_dir = entry.path();
                if !skill_dir.is_dir() {
                    continue;
                }
                let skill_md = skill_dir.join("SKILL.md");
                if !skill_md.is_file() {
                    continue;
                }
                let content = match std::fs::read_to_string(&skill_md) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(?skill_md, %e, "failed to read SKILL.md");
                        continue;
                    },
                };
                match parse::parse_metadata(&content, &skill_dir) {
                    Ok(mut meta) => {
                        meta.source = Some(source.clone());
                        skills.push(meta);
                    },
                    Err(e) => {
                        tracing::warn!(?skill_dir, %e, "failed to parse SKILL.md");
                    },
                }
            }
        }

        Ok(skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discover_skills_in_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("my-skill")).unwrap();
        std::fs::write(
            skills_dir.join("my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: test\n---\nbody\n",
        )
        .unwrap();

        let discoverer = FsSkillDiscoverer::new(vec![(skills_dir.clone(), SkillSource::Project)]);
        let skills = discoverer.discover().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].source, Some(SkillSource::Project));
    }

    #[tokio::test]
    async fn test_discover_skips_missing_dirs() {
        let discoverer = FsSkillDiscoverer::new(vec![(
            PathBuf::from("/nonexistent/path"),
            SkillSource::Personal,
        )]);
        let skills = discoverer.discover().await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_discover_skips_dirs_without_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("not-a-skill")).unwrap();
        std::fs::write(skills_dir.join("not-a-skill/README.md"), "hello").unwrap();

        let discoverer = FsSkillDiscoverer::new(vec![(skills_dir, SkillSource::Project)]);
        let skills = discoverer.discover().await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_discover_skips_invalid_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("bad-skill")).unwrap();
        std::fs::write(skills_dir.join("bad-skill/SKILL.md"), "no frontmatter here").unwrap();

        let discoverer = FsSkillDiscoverer::new(vec![(skills_dir, SkillSource::Project)]);
        let skills = discoverer.discover().await.unwrap();
        assert!(skills.is_empty());
    }
}
