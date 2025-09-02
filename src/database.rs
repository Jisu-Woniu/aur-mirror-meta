use crate::types::{DatabasePackageDetails, DatabasePackageInfo, SearchType};
use anyhow::Result;
use futures::stream::TryStreamExt;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::collections::HashMap;

#[derive(Clone)]
pub struct DatabaseOps {
    pool: SqlitePool,
}

impl DatabaseOps {
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(db_path)
                .create_if_missing(true),
        )
        .await?;
        let result = Self { pool };
        result.init_index_tables().await?;
        Ok(result)
    }

    async fn init_index_tables(&self) -> Result<()> {
        let tables = vec![
            r#"CREATE TABLE IF NOT EXISTS branch_commits (
                branch TEXT NOT NULL PRIMARY KEY,
                commit_id TEXT NOT NULL
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_info (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                pkg_desc TEXT,
                version TEXT NOT NULL,
                url TEXT,
                commit_id TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_depends (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                depend TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, depend)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_make_depends (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                make_depend TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, make_depend)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_opt_depends (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                opt_depend TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, opt_depend)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_check_depends (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                check_depend TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, check_depend)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_provides (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                provide TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, provide)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_conflicts (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                conflict TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, conflict)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_replaces (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                replace TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, replace)
            )"#,
            r#"CREATE TABLE IF NOT EXISTS pkg_groups (
                branch TEXT NOT NULL,
                pkg_name TEXT NOT NULL,
                group_name TEXT NOT NULL,
                PRIMARY KEY (branch, pkg_name, group_name)
            )"#,
        ];

        for table_sql in tables {
            sqlx::query(table_sql).execute(&self.pool).await?;
        }

        let indexes = vec![
            // Query based on pkg name
            "CREATE INDEX IF NOT EXISTS idx_pkg_info_name ON pkg_info(pkg_name)",
            // Query based on branch
            "CREATE INDEX IF NOT EXISTS idx_pkg_info_branch ON pkg_info(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_depends_branch ON pkg_depends(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_make_depends_branch ON pkg_make_depends(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_opt_depends_branch ON pkg_opt_depends(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_check_depends_branch ON pkg_check_depends(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_provides_branch ON pkg_provides(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_conflicts_branch ON pkg_conflicts(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_replaces_branch ON pkg_replaces(branch)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_groups_branch ON pkg_groups(branch)",
            // For reverse lookups
            "CREATE INDEX IF NOT EXISTS idx_pkg_depends_depend ON pkg_depends(depend)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_make_depends_make_depend ON pkg_make_depends(make_depend)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_opt_depends_opt_depend ON pkg_opt_depends(opt_depend)",
            "CREATE INDEX IF NOT EXISTS idx_pkg_check_depends_check_depend ON pkg_check_depends(check_depend)",
        ];

        for index_sql in indexes {
            sqlx::query(index_sql).execute(&self.pool).await?;
        }

        Ok(())
    }

    pub async fn get_existing_commits(&self) -> Result<HashMap<String, String>> {
        let mut rows =
            sqlx::query("SELECT branch, commit_id FROM branch_commits").fetch(&self.pool);
        let mut commits = HashMap::new();
        while let Some(row) = rows.try_next().await? {
            let branch: String = row.get("branch");
            let commit_id: String = row.get("commit_id");
            commits.insert(branch, commit_id);
        }
        Ok(commits)
    }

    pub async fn begin_transaction(&self) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>> {
        Ok(self.pool.begin().await?)
    }

    pub async fn update_branch_commit_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        branch: &str,
        commit_id: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO branch_commits (branch, commit_id) 
            VALUES (?, ?)
        "#,
        )
        .bind(branch)
        .bind(commit_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn clear_index_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        branch: &str,
    ) -> Result<()> {
        let tables = vec![
            "pkg_info",
            "pkg_depends",
            "pkg_make_depends",
            "pkg_opt_depends",
            "pkg_check_depends",
            "pkg_provides",
            "pkg_conflicts",
            "pkg_replaces",
            "pkg_groups",
        ];
        for table in tables {
            let query = format!("DELETE FROM {} WHERE branch = ?", table);
            sqlx::query(&query).bind(branch).execute(&mut **tx).await?;
        }
        Ok(())
    }

    pub async fn update_index_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        packages: &[DatabasePackageDetails],
    ) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        for pkg in packages {
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO pkg_info 
                (branch, pkg_name, pkg_desc, version, url, commit_id) 
                VALUES (?, ?, ?, ?, ?, ?)
            "#,
            )
            .bind(&pkg.info.branch)
            .bind(&pkg.info.pkg_name)
            .bind(&pkg.info.pkg_desc)
            .bind(&pkg.info.version)
            .bind(&pkg.info.url)
            .bind(&pkg.info.commit_id)
            .execute(&mut **tx)
            .await?;

            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_depends",
                "depend",
                &pkg.depends,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_make_depends",
                "make_depend",
                &pkg.make_depends,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_opt_depends",
                "opt_depend",
                &pkg.opt_depends,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_check_depends",
                "check_depend",
                &pkg.check_depends,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_provides",
                "provide",
                &pkg.provides,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_conflicts",
                "conflict",
                &pkg.conflicts,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_replaces",
                "replace",
                &pkg.replaces,
            )
            .await?;
            self.store_array_tx(
                tx,
                &pkg.info.branch,
                &pkg.info.pkg_name,
                "pkg_groups",
                "group_name",
                &pkg.groups,
            )
            .await?;
        }

        Ok(())
    }

    async fn store_array_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        branch: &str,
        pkg_name: &str,
        table: &str,
        column: &str,
        items: &[String],
    ) -> Result<()> {
        for item in items {
            let query = format!(
                "INSERT OR IGNORE INTO {} (branch, pkg_name, {}) VALUES (?, ?, ?)",
                table, column
            );
            sqlx::query(&query)
                .bind(branch)
                .bind(pkg_name)
                .bind(item)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

    pub async fn search_packages(
        &self,
        search_type: SearchType,
        keyword: &str,
    ) -> Result<Vec<DatabasePackageInfo>> {
        let (query, param, count) = match search_type {
            SearchType::Name => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p 
                    WHERE p.pkg_name LIKE ?
                "#,
                format!("%{}%", keyword),
                1,
            ),
            SearchType::NameDesc => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p 
                    WHERE (p.pkg_name LIKE ? OR p.pkg_desc LIKE ?)
                "#,
                format!("%{}%", keyword),
                2,
            ),
            SearchType::Depends => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p
                    JOIN pkg_depends d ON p.pkg_name = d.pkg_name AND p.branch = d.branch
                    WHERE d.depend = ?
                "#,
                keyword.to_string(),
                1,
            ),
            SearchType::MakeDepends => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p
                    JOIN pkg_make_depends md ON p.pkg_name = md.pkg_name AND p.branch = md.branch
                    WHERE md.make_depend = ?
                "#,
                keyword.to_string(),
                1,
            ),
            SearchType::OptDepends => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p
                    JOIN pkg_opt_depends od ON p.pkg_name = od.pkg_name AND p.branch = od.branch
                    WHERE od.opt_depend = ?
                "#,
                keyword.to_string(),
                1,
            ),
            SearchType::CheckDepends => (
                r#"
                    SELECT DISTINCT p.* FROM pkg_info p
                    JOIN pkg_check_depends cd ON p.pkg_name = cd.pkg_name AND p.branch = cd.branch
                    WHERE cd.check_depend = ?
                "#,
                keyword.to_string(),
                1,
            ),
        };

        let mut query_builder = sqlx::query(query);
        for _ in 0..count {
            query_builder = query_builder.bind(&param);
        }
        query_builder
            .fetch(&self.pool)
            .map_ok(|row| DatabasePackageInfo {
                commit_id: row.get("commit_id"),
                branch: row.get("branch"),
                pkg_name: row.get("pkg_name"),
                pkg_desc: row.get("pkg_desc"),
                version: row.get("version"),
                url: row.get("url"),
            })
            .try_collect::<Vec<_>>()
            .await
            .map_err(Into::into)
    }

    pub async fn get_package_details(
        &self,
        package_names: &[String],
    ) -> Result<Vec<DatabasePackageDetails>> {
        if package_names.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = package_names.iter().map(|_| "?".to_string()).collect();
        let placeholders_str = placeholders.join(",");

        let query = format!(
            r#"SELECT * FROM pkg_info WHERE pkg_name IN ({})"#,
            placeholders_str
        );

        let mut query_builder = sqlx::query(&query);
        for name in package_names {
            query_builder = query_builder.bind(name);
        }

        query_builder
            .fetch(&self.pool)
            .and_then(async |row| -> sqlx::Result<DatabasePackageDetails> {
                let info = DatabasePackageInfo {
                    commit_id: row.get("commit_id"),
                    branch: row.get("branch"),
                    pkg_name: row.get("pkg_name"),
                    pkg_desc: row.get("pkg_desc"),
                    version: row.get("version"),
                    url: row.get("url"),
                };

                let package_name: String = row.get("pkg_name");
                let pkg_branch: String = row.get("branch");

                let tables = vec![
                    ("pkg_depends", "depend"),
                    ("pkg_make_depends", "make_depend"),
                    ("pkg_opt_depends", "opt_depend"),
                    ("pkg_check_depends", "check_depend"),
                    ("pkg_provides", "provide"),
                    ("pkg_conflicts", "conflict"),
                    ("pkg_replaces", "replace"),
                    ("pkg_groups", "group_name"),
                ];

                let mut depends = Vec::new();
                let mut make_depends = Vec::new();
                let mut opt_depends = Vec::new();
                let mut check_depends = Vec::new();
                let mut provides = Vec::new();
                let mut conflicts = Vec::new();
                let mut replaces = Vec::new();
                let mut groups = Vec::new();

                for (table, column) in tables {
                    let query = format!(
                        "SELECT {} FROM {} WHERE pkg_name = ? AND branch = ?",
                        column, table
                    );
                    let values = sqlx::query(&query)
                        .bind(&package_name)
                        .bind(&pkg_branch)
                        .fetch(&self.pool)
                        .map_ok(|row| row.get::<String, _>(column))
                        .try_collect()
                        .await?;

                    match column {
                        "depend" => depends = values,
                        "make_depend" => make_depends = values,
                        "opt_depend" => opt_depends = values,
                        "check_depend" => check_depends = values,
                        "provide" => provides = values,
                        "conflict" => conflicts = values,
                        "replace" => replaces = values,
                        "group_name" => groups = values,
                        _ => {}
                    }
                }
                Ok(DatabasePackageDetails {
                    info,
                    depends,
                    make_depends,
                    opt_depends,
                    check_depends,
                    provides,
                    conflicts,
                    replaces,
                    groups,
                })
            })
            .try_collect()
            .await
            .map_err(Into::into)
    }

    pub async fn get_branch_commit_id(&self, branch: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT commit_id FROM branch_commits WHERE branch = ? LIMIT 1")
            .bind(branch)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.get("commit_id")))
    }
}
