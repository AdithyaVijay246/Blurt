//! Vault lifecycle — `MODULE_02_SCHEMA.md` §3 and §5.
//!
//! The app starts locked with no database handle at all, so until one of these
//! runs, every other command returns [`CommandError::Locked`]. This is the
//! policy layer Module 2 deliberately does not own: `blurt-schema` provides
//! key-wrapping and storage primitives, and *deciding when to unwrap* belongs
//! here (PROGRESS.md decision #10).
//!
//! ## What lives where on disk
//!
//! Two files in the app data directory. `keyring.json` holds only wrapped
//! blobs — useless without a passphrase or the recovery key — which is why it
//! can sit outside the encrypted database it unlocks (decision #2).
//! `blurt.db` is the SQLCipher file.
//!
//! ## Scope
//!
//! App-level unlock only. §5's *sensitive-destination* rule is a different
//! policy and is not implemented here: every access to a sensitive destination
//! needs a fresh, uncached check that no unlock state can satisfy. The
//! `Sensitive` keyslot is created at setup so that gate has something to check
//! against, but nothing yet calls it.
//!
//! The idle timer that §5 says expires app-level unlock is also not here —
//! [`lock_impl`] is the mechanism it will call, but owning the timer is Module
//! 6's job.

use std::path::{Path, PathBuf};

use tauri::{Manager, State};

use blurt_schema::keyring::SlotKind;
use blurt_schema::repository::secrets;
use blurt_schema::{Database, Keyring, MasterKey, RecoveryKey};

use crate::error::{CommandError, CommandResult};
use crate::state::{AppState, RagPaths};

/// Wrapped key blobs. Not secret on its own — see the module docs.
const KEYRING_FILE: &str = "keyring.json";
/// The SQLCipher database.
const DATABASE_FILE: &str = "blurt.db";

fn keyring_path(data_dir: &Path) -> PathBuf {
    data_dir.join(KEYRING_FILE)
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DATABASE_FILE)
}

/// Creates a new vault and leaves it unlocked.
///
/// Returns the recovery key as its grouped display string, which is exactly
/// what `MODULE_06_UI_SHELL.md` §B4 shows the user and then asks them to verify
/// two groups of. **This is the only time it is returned from a plain unlock
/// path** — afterwards it is readable only from inside the database, behind a
/// fresh auth check (§D2).
///
/// `sensitive_passphrase` may be the same string as `master_passphrase`; §5
/// leaves that to the user. Each slot has its own salt, so reuse is not
/// detectable from the keyring file.
///
/// ## Write order
///
/// The keyring is saved **before** the database is created, and the asymmetry
/// matters. Interrupted after the keyring is written, the next launch finds a
/// vault whose passphrase already works and whose database is created on first
/// unlock — recoverable. The reverse order would leave a database encrypted
/// under a master key that was never wrapped anywhere, which is unrecoverable
/// by construction.
fn initialize_vault_impl(
    state: &AppState,
    data_dir: &Path,
    master_passphrase: &str,
    sensitive_passphrase: &str,
) -> CommandResult<String> {
    if keyring_path(data_dir).exists() {
        return Err(CommandError::AlreadyInitialized);
    }
    std::fs::create_dir_all(data_dir).map_err(|e| CommandError::Io(e.to_string()))?;

    let master = MasterKey::generate();
    let recovery = RecoveryKey::generate();

    let mut keyring = Keyring::new();
    keyring.add_passphrase_slot(SlotKind::Passphrase, &master, master_passphrase)?;
    keyring.add_recovery_slot(&master, &recovery)?;
    keyring.add_passphrase_slot(SlotKind::Sensitive, &master, sensitive_passphrase)?;
    keyring.save(&keyring_path(data_dir))?;

    let db = open_and_migrate(data_dir, &master)?;
    secrets::put_recovery_key(db.conn(), &recovery)?;

    *state.db.lock().unwrap() = Some(db);
    *state.keyring.lock().unwrap() = Some(keyring);

    Ok(recovery.to_grouped_string())
}

/// Unwraps the master key with the day-to-day passphrase and opens the database.
fn unlock_impl(state: &AppState, data_dir: &Path, passphrase: &str) -> CommandResult<()> {
    let path = keyring_path(data_dir);
    if !path.exists() {
        return Err(CommandError::NotInitialized);
    }

    let keyring = Keyring::load(&path)?;
    let master = keyring.unwrap_with_passphrase(SlotKind::Passphrase, passphrase)?;
    let db = open_and_migrate(data_dir, &master)?;

    *state.db.lock().unwrap() = Some(db);
    *state.keyring.lock().unwrap() = Some(keyring);
    Ok(())
}

/// Drops the database handle, returning the app to its locked state.
///
/// §5's app-level lock: on app close, reboot, or the idle timer expiring. The
/// unwrapped master key only ever existed inside the `Database`, so dropping it
/// is what actually re-locks — there is no separate key to zero here.
fn lock_impl(state: &AppState) {
    *state.db.lock().unwrap() = None;
    *state.keyring.lock().unwrap() = None;
}

fn is_unlocked_impl(state: &AppState) -> bool {
    state.db.lock().unwrap().is_some()
}

/// Opens the database under `master` and brings its schema up to date.
///
/// Migrations run on unlock, not only on creation: a build that ships a new
/// migration has to apply it to the vault that already exists, and unlock is
/// the first moment a key is available to do so.
fn open_and_migrate(data_dir: &Path, master: &MasterKey) -> CommandResult<Database> {
    let db = Database::open(&database_path(data_dir), master)?;
    blurt_schema::migrations::run(db.conn())?;
    Ok(db)
}

/// The app data directory, created by Tauri per-platform.
fn app_data_dir(app: &tauri::AppHandle) -> CommandResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| CommandError::Io(format!("no app data directory: {e}")))
}

/// Where Module 4's files live for this install.
///
/// Two different roots, deliberately. The vector store is derived data that is
/// rebuilt if lost, so it sits beside the database in the writable data
/// directory. The two model files ship *with the app* and are read-only, so
/// they come from Tauri's bundled resources (PROGRESS.md decision #51) —
/// `BLUEPRINT.md` §2 requires them bundled rather than downloaded.
///
/// Resolving a resource path does not require the file to exist; neither model
/// is bundled yet, so both currently point at paths that are absent. That
/// surfaces as a load error at ask time rather than a failure here, which is
/// what keeps search and capture working in the meantime.
pub(crate) fn rag_paths(app: &tauri::AppHandle) -> CommandResult<RagPaths> {
    let data = app_data_dir(app)?;
    let resource = |relative: &str| {
        app.path()
            .resolve(relative, tauri::path::BaseDirectory::Resource)
            .map_err(|e| CommandError::Io(format!("cannot resolve bundled {relative}: {e}")))
    };

    Ok(RagPaths {
        vectors: data.join("vectors"),
        embedding_cache: resource("models/embedding")?,
        gguf: resource("models/generative.gguf")?,
    })
}

#[tauri::command]
pub fn initialize_vault(
    app: tauri::AppHandle,
    state: State<AppState>,
    master_passphrase: String,
    sensitive_passphrase: String,
) -> CommandResult<String> {
    let dir = app_data_dir(&app)?;
    initialize_vault_impl(
        state.inner(),
        &dir,
        &master_passphrase,
        &sensitive_passphrase,
    )
}

#[tauri::command]
pub fn unlock(
    app: tauri::AppHandle,
    state: State<AppState>,
    passphrase: String,
) -> CommandResult<()> {
    let dir = app_data_dir(&app)?;
    unlock_impl(state.inner(), &dir, &passphrase)
}

#[tauri::command]
pub fn lock(state: State<AppState>) {
    lock_impl(state.inner());
}

#[tauri::command]
pub fn is_unlocked(state: State<AppState>) -> bool {
    is_unlocked_impl(state.inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations;

    const MASTER: &str = "correct horse battery staple";
    const SENSITIVE: &str = "a different one entirely";

    fn vault() -> (AppState, tempfile::TempDir) {
        (AppState::default(), tempfile::tempdir().unwrap())
    }

    #[test]
    fn initializing_leaves_the_vault_unlocked_and_usable() {
        let (state, dir) = vault();

        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        assert!(is_unlocked_impl(&state));
        // Migrations ran, so the seeded destinations are queryable.
        let guard = state.db.lock().unwrap();
        let db = guard.as_ref().unwrap();
        assert!(destinations::get_by_id(db.conn(), destinations::UNSORTED_ID)
            .unwrap()
            .is_some());
    }

    #[test]
    fn initializing_returns_the_recovery_key_in_the_form_onboarding_displays() {
        let (state, dir) = vault();

        let recovery = initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        // §B4 shows it in readable groups and asks for two of them back.
        assert!(recovery.contains('-'), "expected grouped form, got {recovery}");
        assert!(RecoveryKey::parse(&recovery).is_ok(), "must round-trip");
    }

    #[test]
    fn the_recovery_key_is_also_stored_inside_the_database_for_re_display() {
        // §D2 re-displays it from Settings; decision #1 puts it in app_secrets.
        let (state, dir) = vault();
        let returned = initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        let guard = state.db.lock().unwrap();
        let stored = secrets::get_recovery_key(guard.as_ref().unwrap().conn())
            .unwrap()
            .expect("onboarding should have stored it");

        assert_eq!(stored.to_grouped_string(), returned);
    }

    #[test]
    fn initializing_twice_is_refused_rather_than_clobbering_the_vault() {
        // The dangerous case: a second init would generate a new master key and
        // orphan every byte already written under the old one.
        let (state, dir) = vault();
        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        let error = initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap_err();
        assert_eq!(error, CommandError::AlreadyInitialized);
    }

    #[test]
    fn a_vault_can_be_locked_and_unlocked_again() {
        let (state, dir) = vault();
        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        lock_impl(&state);
        assert!(!is_unlocked_impl(&state), "lock must drop the handle");

        unlock_impl(&state, dir.path(), MASTER).unwrap();
        assert!(is_unlocked_impl(&state));
    }

    #[test]
    fn unlocking_a_fresh_install_reports_that_setup_has_not_happened() {
        // Distinct from a wrong passphrase: §B6's unlock screen and onboarding
        // are different screens, and the frontend picks between them on this.
        let (state, dir) = vault();

        let error = unlock_impl(&state, dir.path(), MASTER).unwrap_err();
        assert_eq!(error, CommandError::NotInitialized);
        assert!(!is_unlocked_impl(&state));
    }

    #[test]
    fn a_wrong_passphrase_is_refused_and_leaves_the_vault_locked() {
        let (state, dir) = vault();
        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();
        lock_impl(&state);

        let error = unlock_impl(&state, dir.path(), "not the passphrase").unwrap_err();
        assert_eq!(error, CommandError::WrongSecret);
        assert!(!is_unlocked_impl(&state), "a failed unlock must not open anything");
    }

    #[test]
    fn the_sensitive_passphrase_is_a_separate_slot_that_does_not_unlock_the_app() {
        // §5: the sensitive passphrase gates sensitive destinations, not
        // app-level unlock. Accepting it here would collapse two deliberately
        // different policies into one.
        let (state, dir) = vault();
        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();
        lock_impl(&state);

        let error = unlock_impl(&state, dir.path(), SENSITIVE).unwrap_err();
        assert_eq!(error, CommandError::WrongSecret);
    }

    #[test]
    fn reusing_the_master_passphrase_for_sensitive_access_is_allowed() {
        // §5 explicitly leaves this to the user; each slot has its own salt, so
        // reuse is not detectable from the keyring file.
        let (state, dir) = vault();

        assert!(initialize_vault_impl(&state, dir.path(), MASTER, MASTER).is_ok());
    }

    #[test]
    fn data_written_before_locking_survives_the_next_unlock() {
        // The real point of the whole file: the same master key must come back
        // out of the keyring, or the database is unreadable.
        let (state, dir) = vault();
        initialize_vault_impl(&state, dir.path(), MASTER, SENSITIVE).unwrap();

        let created = {
            let guard = state.db.lock().unwrap();
            destinations::create(
                guard.as_ref().unwrap().conn(),
                "Shopping",
                "shopping",
                destinations::DestinationKind::List,
                None,
                false,
                false,
                0,
            )
            .unwrap()
        };

        lock_impl(&state);
        unlock_impl(&state, dir.path(), MASTER).unwrap();

        let guard = state.db.lock().unwrap();
        let found = destinations::get_by_id(guard.as_ref().unwrap().conn(), created.id).unwrap();
        assert_eq!(found.expect("destination should survive").name, "Shopping");
    }

    #[test]
    fn a_vault_initialized_in_one_session_unlocks_in_a_completely_fresh_state() {
        // Simulates an app restart: nothing in memory carries over, only the
        // two files on disk.
        let (first, dir) = vault();
        initialize_vault_impl(&first, dir.path(), MASTER, SENSITIVE).unwrap();
        drop(first);

        let second = AppState::default();
        unlock_impl(&second, dir.path(), MASTER).unwrap();

        assert!(is_unlocked_impl(&second));
    }
}
