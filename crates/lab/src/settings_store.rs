//! Durable settings storage.
//!
//! # Why settings get their own store
//!
//! Settings and experiment results share a SQLite file but nothing else. They are stored in separate
//! tables and reached through a separate type for three reasons:
//!
//! 1. **Reset must not be able to touch results.** If settings and results shared a table, a
//!    reset-to-defaults would be a statement that could plausibly match either. A separate store
//!    makes "reset deletes exactly one row" a structural fact rather than a promise.
//! 2. **Results are immutable; settings are not.** [`SqliteResultRepository`] enforces an
//!    immutability trigger on results, which is the right guarantee for evidence and the wrong one
//!    for a preference. Overloading that repository would have meant either relaxing the trigger or
//!    lying about it.
//! 3. **A corrupt settings row must not be able to look like a measurement.** They now cannot even
//!    be read through the same API.
//!
//! # What is stored
//!
//! One JSON document, validated on the way in and on the way out. A single document rather than a
//! key-value table, because the settings are a versioned struct with a schema version: storing them
//! as loose keys would make it possible to write half a settings change and leave the rest at
//! defaults, which is a state no validation can describe.
//!
//! # Absence is the default
//!
//! A store with no row returns [`LabSettings::default`], not an error. A first run has no settings
//! written yet, and treating that as a failure would mean the settings page could not open before
//! you had visited it once.
//!
//! # A stored document is validated on read
//!
//! Not only on write. A row that was hand-edited, or written by a build with a different rule, is
//! validated when it is read; if it is not legal under the current build's rules, the read fails
//! loudly and the caller falls back to defaults. A settings file that fails to parse must not
//! silently become a document with fields missing.

use std::path::Path;
use std::sync::Mutex;

use crate::settings::{LabSettings, SettingsError, SETTINGS_SCHEMA_VERSION};
use rusqlite::Connection;

/// Where settings are persisted.
///
/// Deliberately not taking the telemetry repository's connection. Sharing a `Mutex<Connection>`
/// would mean a settings write could block behind a long telemetry read, and it would put settings
/// and results in the same lock, which is the coupling this type exists to avoid.
pub struct SqliteSettingsStore {
    connection: Mutex<Connection>,
}

impl SqliteSettingsStore {
    /// Opens (or creates) the settings store at `path`.
    ///
    /// Creates the parent directory if it is missing, because the application data directory is
    /// created at start-up but a settings path that fails to open because of a missing intermediate
    /// would be a confusing way to lose every preference. Returning an error here would silently
    /// fall back to defaults, which is the failure this avoids.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SettingsError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| SettingsError::Storage(error.to_string()))?;
            }
        }
        let connection =
            Connection::open(path).map_err(|error| SettingsError::Storage(error.to_string()))?;
        Self::from_connection(connection)
    }

    /// Builds a store over an existing connection, for tests and for embedding.
    pub fn from_connection(connection: Connection) -> Result<Self, SettingsError> {
        connection
            .execute_batch(
                r#"
                -- One row per schema version, so a future migration can keep the previous document
                -- rather than having to reconstruct it.
                CREATE TABLE IF NOT EXISTS settings (
                    schema_version INTEGER NOT NULL,
                    document TEXT NOT NULL,
                    updated_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY (schema_version),
                    CHECK (json_valid(document))
                );
                "#,
            )
            .map_err(|error| SettingsError::Storage(error.to_string()))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Reads the current settings, or the defaults when nothing is stored.
    ///
    /// A stored document that fails validation is an error, not a silent fallback: a settings file
    /// that cannot be read is a fact worth surfacing, and quietly replacing it with defaults would
    /// lose whatever the user had chosen without telling them.
    pub fn load(&self) -> Result<LabSettings, SettingsError> {
        let connection = self.connection.lock().map_err(|_| {
            SettingsError::Storage("the settings store lock was poisoned".to_owned())
        })?;
        let document: Option<String> = connection
            .query_row(
                "SELECT document FROM settings WHERE schema_version = ?1 LIMIT 1",
                rusqlite::params![SETTINGS_SCHEMA_VERSION],
                |row| row.get(0),
            )
            .ok();
        let Some(document) = document else {
            return Ok(LabSettings::default());
        };
        let settings: LabSettings = serde_json::from_str(&document).map_err(|error| {
            SettingsError::Storage(format!("stored settings are unreadable: {error}"))
        })?;
        settings.validated()
    }

    /// Validates and stores settings, replacing the previous document.
    ///
    /// Validation happens before the write, so a rejected change leaves the previous settings in
    /// place rather than storing something unusable.
    pub fn save(&self, settings: &LabSettings) -> Result<LabSettings, SettingsError> {
        let validated = settings.clone().validated()?;
        let document = serde_json::to_string(&validated)
            .map_err(|error| SettingsError::Storage(error.to_string()))?;
        let now = now_unix_ms();
        let connection = self.connection.lock().map_err(|_| {
            SettingsError::Storage("the settings store lock was poisoned".to_owned())
        })?;
        connection
            .execute(
                "INSERT INTO settings (schema_version, document, updated_unix_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(schema_version) DO UPDATE SET
                    document = excluded.document,
                    updated_unix_ms = excluded.updated_unix_ms",
                rusqlite::params![validated.schema_version, document, now],
            )
            .map_err(|error| SettingsError::Storage(error.to_string()))?;
        Ok(validated)
    }

    /// Replaces the stored settings with the defaults.
    ///
    /// Returns the defaults that were written, so a caller can report what the reset produced rather
    /// than leaving the caller to assume. It touches one row in one table: no results, no telemetry,
    /// no experiment lifecycle, and no replay data are involved, because none of them are stored here.
    pub fn reset_to_defaults(&self) -> Result<LabSettings, SettingsError> {
        self.save(&LabSettings::default())
    }
}

/// Milliseconds since the Unix epoch, saturating rather than panicking if the clock is before it.
fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
