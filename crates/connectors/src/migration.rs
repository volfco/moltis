use moltis_db::Db;

use crate::Result;

pub async fn run_migrations(db: &Db) -> Result<()> {
    if !db.is_postgres() {
        // SQLite enforces foreign keys per connection only when asked.
        moltis_db::execute!(db, "PRAGMA foreign_keys = ON")?;
    }
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
