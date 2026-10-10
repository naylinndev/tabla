//! Database accounts and privileges (Sequel Ace's "Users" window).
//! Reads only; changes are generated as SQL in the UI and previewed first.

use serde::Serialize;

use crate::{
    config::Driver,
    db::Session,
    error::{AppError, AppResult},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub name: String,
    /// MySQL account host ("%" , "localhost"…); empty for Postgres roles.
    pub host: String,
    pub can_login: bool,
    pub superuser: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub locked: bool,
    pub valid_until: Option<String>,
    pub member_of: Vec<String>,
}

fn truthy(v: Option<String>) -> bool {
    matches!(v.as_deref(), Some("t" | "1" | "Y" | "true"))
}

impl Session {
    pub async fn list_users(&self) -> AppResult<Vec<UserInfo>> {
        let rows = match self.driver {
            Driver::Postgres => {
                self.run_script(
                    "SELECT r.rolname, '', r.rolcanlogin, r.rolsuper, r.rolcreatedb, r.rolcreaterole, 'f', r.rolvaliduntil::text, \
                     (SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) FROM pg_auth_members m JOIN pg_roles g ON g.oid = m.roleid WHERE m.member = r.oid) \
                     FROM pg_roles r WHERE r.rolname !~ '^pg_' ORDER BY r.rolcanlogin DESC, r.rolname",
                    10_000,
                )
                .await?
            }
            Driver::Mysql => self
                .run_script(
                    "SELECT User, Host, 'Y', IF(Super_priv = 'Y' OR Grant_priv = 'Y', 'Y', 'N'), Create_priv, Create_user_priv, \
                     account_locked, IF(password_expired = 'Y', 'expired', NULL), NULL FROM mysql.user ORDER BY User, Host",
                    10_000,
                )
                .await
                .map_err(|e| AppError::msg(format!("Can't list accounts: {e}. Your user needs SELECT on mysql.user.")))?,
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => return Err(AppError::msg("SQLite has no user accounts")),
        };
        Ok(rows
            .into_iter()
            .next()
            .map(|r| r.rows)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                Some(UserInfo {
                    name: n()?,
                    host: n().unwrap_or_default(),
                    can_login: truthy(n()),
                    superuser: truthy(n()),
                    create_db: truthy(n()),
                    create_role: truthy(n()),
                    locked: truthy(n()),
                    valid_until: n(),
                    member_of: n().map(|s| s.split(", ").map(str::to_string).collect()).unwrap_or_default(),
                })
            })
            .collect())
    }

    /// Human-readable privilege lines for one account.
    pub async fn user_grants(&self, name: &str, host: &str) -> AppResult<Vec<String>> {
        let d = self.driver;
        let sql = match d {
            Driver::Postgres => {
                let r = d.quote_literal(name);
                format!(
                    "SELECT 'Database ' || datname || ': ' || concat_ws(', ', \
                       CASE WHEN has_database_privilege({r}, datname, 'CONNECT') THEN 'CONNECT' END, \
                       CASE WHEN has_database_privilege({r}, datname, 'CREATE') THEN 'CREATE' END, \
                       CASE WHEN has_database_privilege({r}, datname, 'TEMP') THEN 'TEMP' END) \
                     FROM pg_database WHERE datallowconn AND NOT datistemplate \
                     UNION ALL \
                     SELECT 'Schema ' || nspname || ': ' || concat_ws(', ', \
                       CASE WHEN has_schema_privilege({r}, nspname, 'USAGE') THEN 'USAGE' END, \
                       CASE WHEN has_schema_privilege({r}, nspname, 'CREATE') THEN 'CREATE' END) \
                     FROM pg_namespace WHERE nspname !~ '^pg_' AND nspname <> 'information_schema' \
                     UNION ALL \
                     SELECT * FROM (SELECT 'Table ' || table_schema || '.' || table_name || ': ' || string_agg(privilege_type, ', ' ORDER BY privilege_type) \
                       FROM information_schema.role_table_grants WHERE grantee = {r} GROUP BY table_schema, table_name ORDER BY 1 LIMIT 300) t"
                )
            }
            Driver::Mysql => format!("SHOW GRANTS FOR {}@{}", d.quote_literal(name), d.quote_literal(host)),
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => return Err(AppError::msg("SQLite has no user accounts")),
        };
        let rows = self.run_script(&sql, 10_000).await?;
        Ok(rows
            .into_iter()
            .next()
            .map(|r| r.rows)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .filter(|l| !l.trim_end().ends_with(':'))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionConfig;

    fn cfg(var: &str, driver: Driver) -> Option<ConnectionConfig> {
        let v = std::env::var(var).ok()?;
        let p: Vec<&str> = v.split(':').collect();
        Some(
            serde_json::from_value(serde_json::json!({
                "id": "users-test", "name": "u", "driver": driver, "sslMode": "disable",
                "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
            }))
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn postgres_users_and_grants() {
        let _serial = crate::test_lock::PG.lock().await;
        let Some(c) = cfg("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        // Idempotent cleanup, even after an interrupted earlier run.
        let cleanup = "DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'tabla_ro_test') THEN \
                       EXECUTE 'DROP OWNED BY tabla_ro_test'; EXECUTE 'DROP ROLE tabla_ro_test'; END IF; END $$";
        s.run_script(cleanup, 1).await.unwrap();
        s.run_script(
            "CREATE ROLE tabla_ro_test LOGIN PASSWORD 'pw-123'; GRANT USAGE ON SCHEMA public TO tabla_ro_test; \
             GRANT SELECT ON ALL TABLES IN SCHEMA public TO tabla_ro_test",
            1,
        )
        .await
        .unwrap();
        let users = s.list_users().await.unwrap();
        let u = users.iter().find(|u| u.name == "tabla_ro_test").expect("role listed");
        assert!(u.can_login && !u.superuser);
        let g = s.user_grants("tabla_ro_test", "").await.unwrap();
        assert!(g.iter().any(|l| l.starts_with("Schema public: USAGE")), "{g:?}");
        assert!(g.iter().any(|l| l.starts_with("Table public.users: SELECT")), "{g:?}");
        assert!(!g.iter().any(|l| l.contains("INSERT")), "{g:?}");
        // The password never reaches the console log.
        assert!(!s.log_since(0).iter().any(|e| e.sql.contains("pw-123")));
        s.run_script(cleanup, 1).await.unwrap();
    }

    #[tokio::test]
    async fn mysql_users_and_grants() {
        let _serial = crate::test_lock::MYSQL.lock().await;
        let Some(c) = cfg("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        s.run_script("DROP USER IF EXISTS 'tabla_ro_test'@'localhost'", 1).await.unwrap();
        s.run_script(
            "CREATE USER 'tabla_ro_test'@'localhost' IDENTIFIED BY 'pw-123'; GRANT SELECT ON tabla_test.* TO 'tabla_ro_test'@'localhost'",
            1,
        )
        .await
        .unwrap();
        let users = s.list_users().await.unwrap();
        assert!(users.iter().any(|u| u.name == "tabla_ro_test" && u.host == "localhost"));
        let g = s.user_grants("tabla_ro_test", "localhost").await.unwrap();
        assert!(g.iter().any(|l| l.contains("GRANT SELECT ON `tabla_test`.*")), "{g:?}");
        assert!(!s.log_since(0).iter().any(|e| e.sql.contains("pw-123")));
        s.run_script("DROP USER 'tabla_ro_test'@'localhost'", 1).await.unwrap();
    }
}
