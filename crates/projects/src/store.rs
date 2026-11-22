use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use {
    crate::{Result, types::Project},
    async_trait::async_trait,
    moltis_db::{Db, execute, fetch_all, fetch_optional},
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Trait for persisting projects. Implementations can be TOML-file-backed,
/// SQLite, etc.
#[async_trait]
pub trait ProjectStore: Send + Sync {
    async fn list(&self) -> Result<Vec<Project>>;
    async fn get(&self, id: &str) -> Result<Option<Project>>;
    async fn upsert(&self, project: Project) -> Result<()>;
    async fn delete(&self, id: &str) -> Result<()>;
}

// ── TOML file-backed implementation ──────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct TomlFile {
    #[serde(default)]
    projects: Vec<Project>,
}

/// Stores projects in a TOML file at the given path.
pub struct TomlProjectStore {
    path: PathBuf,
}

impl TomlProjectStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_file(&self) -> Result<TomlFile> {
        if self.path.exists() {
            let data = fs::read_to_string(&self.path)?;
            Ok(toml::from_str(&data)?)
        } else {
            Ok(TomlFile::default())
        }
    }

    fn write_file(&self, file: &TomlFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = toml::to_string_pretty(file)?;
        fs::write(&self.path, data)?;
        Ok(())
    }
}

#[async_trait]
impl ProjectStore for TomlProjectStore {
    async fn list(&self) -> Result<Vec<Project>> {
        let mut projects = self.read_file()?.projects;
        projects.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
        Ok(projects)
    }

    async fn get(&self, id: &str) -> Result<Option<Project>> {
        Ok(self.read_file()?.projects.into_iter().find(|p| p.id == id))
    }

    async fn upsert(&self, project: Project) -> Result<()> {
        let mut file = self.read_file()?;
        if let Some(existing) = file.projects.iter_mut().find(|p| p.id == project.id) {
            *existing = project;
        } else {
            file.projects.push(project);
        }
        self.write_file(&file)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut file = self.read_file()?;
        file.projects.retain(|p| p.id != id);
        self.write_file(&file)
    }
}

// ── Database-backed implementation (SQLite / PostgreSQL) ────────────────────────────────────

/// Stores projects in a SQLite or PostgreSQL database.
pub struct DbProjectStore {
    db: Db,
}

impl DbProjectStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Initialize the projects table schema.
    ///
    /// Runs the crate's own migrations (SQLite and PostgreSQL dialects), so
    /// tests using in-memory databases get the same schema as production.
    #[doc(hidden)]
    pub async fn init(db: &Db) -> Result<()> {
        crate::run_migrations(db).await
    }
}

#[async_trait]
impl ProjectStore for DbProjectStore {
    async fn list(&self) -> Result<Vec<Project>> {
        let rows = fetch_all!(
            &self.db,
            ProjectRow,
            "SELECT * FROM projects ORDER BY updated_at DESC",
        )?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Project>> {
        let row = fetch_optional!(
            &self.db,
            ProjectRow,
            "SELECT * FROM projects WHERE id = ?",
            id
        )?;
        Ok(row.map(Into::into))
    }

    async fn upsert(&self, project: Project) -> Result<()> {
        execute!(
            &self.db,
            r#"INSERT INTO projects (id, label, directory, system_prompt, auto_worktree, setup_command, teardown_command, branch_prefix, sandbox_image, detected, code_index_enabled, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                 label = excluded.label,
                 directory = excluded.directory,
                 system_prompt = excluded.system_prompt,
                 auto_worktree = excluded.auto_worktree,
                 setup_command = excluded.setup_command,
                 teardown_command = excluded.teardown_command,
                 branch_prefix = excluded.branch_prefix,
                 sandbox_image = excluded.sandbox_image,
                 detected = excluded.detected,
                 code_index_enabled = excluded.code_index_enabled,
                 updated_at = excluded.updated_at"#,
            &project.id,
            &project.label,
            project.directory.to_string_lossy().as_ref(),
            &project.system_prompt,
            project.auto_worktree,
            &project.setup_command,
            &project.teardown_command,
            &project.branch_prefix,
            &project.sandbox_image,
            project.detected,
            project.code_index_enabled,
            project.created_at as i64,
            project.updated_at as i64,
        )?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        execute!(&self.db, "DELETE FROM projects WHERE id = ?", id)?;
        Ok(())
    }
}

/// Internal row type for sqlx mapping (both backends decode the same types).
#[derive(sqlx::FromRow)]
struct ProjectRow {
    id: String,
    label: String,
    directory: String,
    system_prompt: Option<String>,
    auto_worktree: bool,
    setup_command: Option<String>,
    teardown_command: Option<String>,
    branch_prefix: Option<String>,
    sandbox_image: Option<String>,
    detected: bool,
    code_index_enabled: bool,
    created_at: i64,
    updated_at: i64,
}

impl From<ProjectRow> for Project {
    fn from(r: ProjectRow) -> Self {
        Self {
            id: r.id,
            label: r.label,
            directory: PathBuf::from(r.directory),
            system_prompt: r.system_prompt,
            auto_worktree: r.auto_worktree,
            setup_command: r.setup_command,
            teardown_command: r.teardown_command,
            branch_prefix: r.branch_prefix,
            sandbox_image: r.sandbox_image,
            detected: r.detected,
            code_index_enabled: r.code_index_enabled,
            created_at: r.created_at as u64,
            updated_at: r.updated_at as u64,
        }
    }
}

/// Create a new project with auto-derived fields.
pub fn new_project(id: String, label: String, directory: PathBuf) -> Project {
    let now = now_ms();
    Project {
        id,
        label,
        directory,
        system_prompt: None,
        auto_worktree: false,
        setup_command: None,
        teardown_command: None,
        branch_prefix: None,
        sandbox_image: None,
        detected: false,
        code_index_enabled: true,
        created_at: now,
        updated_at: now,
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_toml_store_crud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.toml");
        let store = TomlProjectStore::new(path);

        // Empty initially
        assert!(store.list().await.unwrap().is_empty());

        // Upsert
        let p = new_project("test".into(), "Test".into(), "/tmp/test".into());
        store.upsert(p).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);

        // Get
        let found = store.get("test").await.unwrap().unwrap();
        assert_eq!(found.label, "Test");

        // Update
        let mut updated = found;
        updated.label = "Updated".into();
        store.upsert(updated).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);
        assert_eq!(store.get("test").await.unwrap().unwrap().label, "Updated");

        // Delete
        store.delete("test").await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
    }

    async fn sqlite_db() -> Db {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db = Db::from(pool);
        DbProjectStore::init(&db).await.unwrap();
        db
    }

    async fn crud_suite(store: &DbProjectStore) {
        assert!(store.list().await.unwrap().is_empty());

        let p = new_project("test".into(), "Test".into(), "/tmp/test".into());
        store.upsert(p).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);

        let found = store.get("test").await.unwrap().unwrap();
        assert_eq!(found.label, "Test");

        let mut updated = found;
        updated.label = "Updated".into();
        store.upsert(updated).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);
        assert_eq!(store.get("test").await.unwrap().unwrap().label, "Updated");

        store.delete("test").await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_store_crud() {
        let store = DbProjectStore::new(sqlite_db().await);
        crud_suite(&store).await;
    }

    #[tokio::test]
    async fn test_postgres_store_crud() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let Some(db) = crate::test_support::pg_test_db().await else {
            return;
        };
        crud_suite(&DbProjectStore::new(db)).await;
    }

    #[tokio::test]
    async fn test_toml_store_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.toml");

        {
            let store = TomlProjectStore::new(path.clone());
            store
                .upsert(new_project("a".into(), "A".into(), "/a".into()))
                .await
                .unwrap();
        }

        // New store instance reads from disk
        let store = TomlProjectStore::new(path);
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
    }
}
