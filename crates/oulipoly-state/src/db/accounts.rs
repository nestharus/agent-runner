//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration }
//!
//! Provider account persistence methods for `StateDb`.

use super::*;

struct AccountQueryVariant<'a> {
    sql: &'static str,
    provider: Option<&'a str>,
}

impl StateDb {
    /// Insert a new account. Fails if (id, provider) already exists.
    pub fn insert_account(&self, account: &AccountRecord) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO accounts (id, provider, profile_name, auth_method, auth_status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                sqlite::params![
                    &account.id,
                    &account.provider,
                    &account.profile_name,
                    &account.auth_method.to_db_string(),
                    account.auth_status.as_str(),
                    &account.created_at,
                ],
            )
            .map_err(Self::format_account_insert_error)?;
        Ok(())
    }

    /// List all accounts, optionally filtered by provider.
    pub fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
        self.load_account_rows(provider)
    }

    fn select_account_query_variant(provider: Option<&str>) -> AccountQueryVariant<'_> {
        match provider {
            Some(provider) => AccountQueryVariant {
                sql: "SELECT id, provider, profile_name, auth_method, auth_status, created_at
                      FROM accounts WHERE provider = ?1 ORDER BY id",
                provider: Some(provider),
            },
            None => AccountQueryVariant {
                sql: "SELECT id, provider, profile_name, auth_method, auth_status, created_at
                      FROM accounts ORDER BY provider, id",
                provider: None,
            },
        }
    }

    fn load_account_rows(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
        let query = Self::select_account_query_variant(provider);
        self.query_account_rows(query)
    }

    fn query_account_rows(
        &self,
        query: AccountQueryVariant<'_>,
    ) -> Result<Vec<AccountRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(query.sql)
            .map_err(Self::format_account_query_prepare_error)?;

        let rows = if let Some(provider) = query.provider {
            stmt.query_map(sqlite::params![provider], Self::account_row_mapper)
                .map_err(Self::format_accounts_query_error)?
        } else {
            stmt.query_map([], Self::account_row_mapper)
                .map_err(Self::format_accounts_query_error)?
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(Self::format_account_row_read_error)?);
        }
        Ok(result)
    }

    /// Delete an account by (id, provider).
    pub fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM accounts WHERE id = ?1 AND provider = ?2",
                sqlite::params![id, provider],
            )
            .map_err(Self::format_account_delete_error)?;
        Ok(Self::account_rows_changed(changed))
    }

    fn account_rows_changed(changed: usize) -> bool {
        changed > 0
    }

    /// Helper: map a rusqlite row to an AccountRecord.
    fn account_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<AccountRecord> {
        let auth_method = Self::account_auth_method(row.get(3)?);
        let auth_status = Self::account_auth_status(row.get(4)?);
        Ok(AccountRecord {
            id: row.get(0)?,
            provider: row.get(1)?,
            profile_name: row.get(2)?,
            auth_method,
            auth_status,
            created_at: row.get(5)?,
        })
    }

    fn account_auth_method(raw: String) -> AuthMethod {
        AuthMethod::from_db_string(&raw)
    }

    fn account_auth_status(raw: String) -> AuthStatus {
        AuthStatus::from_str(&raw)
    }

    fn format_account_insert_error(e: sqlite::Error) -> String {
        format!("Failed to insert account: {e}")
    }

    fn format_account_query_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare query: {e}")
    }

    fn format_accounts_query_error(e: sqlite::Error) -> String {
        format!("Failed to query accounts: {e}")
    }

    fn format_account_row_read_error(e: sqlite::Error) -> String {
        format!("Failed to read account row: {e}")
    }

    fn format_account_delete_error(e: sqlite::Error) -> String {
        format!("Failed to delete account: {e}")
    }
}
