use std::path::{Path, PathBuf};

use crate::{parse, types::SkillMetadata};

/// Install a skill from a GitHub repository into the target directory.
///
/// Tries `git clone --depth=1` first, falls back to HTTP tarball fetch.
/// The `source` should be `owner/repo` format (e.g. `vercel-labs/agent-skills`).
pub async fn install_skill(source: &str, install_dir: &Path) -> anyhow::Result<SkillMetadata> {
    let (owner, repo) = parse_source(source)?;
    let target = install_dir.join(&repo);

    if target.exists() {
        anyhow::bail!(
            "skill directory already exists: {}. Remove it first with `skills remove`.",
            target.display()
        );
    }

    tokio::fs::create_dir_all(install_dir).await?;

    // Try git clone first
    let git_url = format!("https://github.com/{owner}/{repo}");
    let git_result = tokio::process::Command::new("git")
        .args(["clone", "--depth=1", &git_url, &target.to_string_lossy()])
        .output()
        .await;

    match git_result {
        Ok(output) if output.status.success() => {
            tracing::info!(%source, "installed skill via git clone");
        },
        _ => {
            // Fallback: HTTP fetch of the default branch tarball
            return install_via_http(&owner, &repo, &target).await;
        },
    }

    // Validate the installed skill
    validate_installed_skill(&target).await
}

/// Install by fetching a tarball from GitHub's API.
async fn install_via_http(owner: &str, repo: &str, target: &Path) -> anyhow::Result<SkillMetadata> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/tarball");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "moltis-skills")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("failed to fetch {}/{}: HTTP {}", owner, repo, resp.status());
    }

    let bytes = resp.bytes().await?;

    // Extract tarball to target
    tokio::fs::create_dir_all(target).await?;
    let target_owned = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(decoder);
        // GitHub tarballs have a top-level directory; strip it
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            // Strip the first component (e.g. "owner-repo-sha/")
            let stripped: PathBuf = path.components().skip(1).collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            let dest = target_owned.join(&stripped);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&dest)?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;

    tracing::info!(%owner, %repo, "installed skill via HTTP tarball");
    validate_installed_skill(target).await
}

/// Validate that a SKILL.md exists and parses correctly in the installed directory.
async fn validate_installed_skill(skill_dir: &Path) -> anyhow::Result<SkillMetadata> {
    let skill_md = skill_dir.join("SKILL.md");
    if !skill_md.exists() {
        // Clean up
        let _ = tokio::fs::remove_dir_all(skill_dir).await;
        anyhow::bail!(
            "installed repository does not contain a SKILL.md at {}",
            skill_md.display()
        );
    }

    let content = tokio::fs::read_to_string(&skill_md).await?;
    match parse::parse_metadata(&content, skill_dir) {
        Ok(mut meta) => {
            meta.source = Some(crate::types::SkillSource::Registry);
            Ok(meta)
        },
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(skill_dir).await;
            Err(e)
        },
    }
}

/// Parse `owner/repo` from a source string.
fn parse_source(source: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<&str> = source.trim().split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!(
            "invalid skill source '{}': expected 'owner/repo' format",
            source
        );
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Get the default installation directory.
pub fn default_install_dir() -> anyhow::Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.home_dir().join(".moltis/installed-skills"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source_valid() {
        let (owner, repo) = parse_source("vercel-labs/agent-skills").unwrap();
        assert_eq!(owner, "vercel-labs");
        assert_eq!(repo, "agent-skills");
    }

    #[test]
    fn test_parse_source_invalid() {
        assert!(parse_source("noslash").is_err());
        assert!(parse_source("too/many/parts").is_err());
        assert!(parse_source("/empty-owner").is_err());
        assert!(parse_source("empty-repo/").is_err());
    }
}
