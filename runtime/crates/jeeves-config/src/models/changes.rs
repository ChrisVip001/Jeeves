//! Profile metadata is the commit point for a credential replacement. The previous key stays
//! usable until then. A pointer-only intent lets the next process retire the unused key.
use super::{find, remove, save_models, upsert, ModelProfile, ModelsFile, SecretStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io,
    path::Path,
    sync::Arc,
};

pub const JOURNAL: &str = ".jeeves-model-change.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialSlot {
    #[default]
    Legacy,
    A,
    B,
}

impl CredentialSlot {
    pub(super) fn is_legacy(&self) -> bool {
        *self == Self::Legacy
    }

    pub fn account(self, id: &str) -> String {
        if self == Self::Legacy {
            return id.to_owned();
        }
        let tag = if self == Self::A { 'a' } else { 'b' };
        let hash = Sha256::digest(id.as_bytes());
        format!("model-slot-{tag}-{hash:x}")
    }

    fn next(self) -> Self {
        if self == Self::A {
            Self::B
        } else {
            Self::A
        }
    }
}

fn stored(profile: &ModelProfile) -> Option<CredentialSlot> {
    profile
        .key_source
        .as_ref()
        .filter(|s| !s.starts_with("env:"))
        .map(|_| profile.credential_slot)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Change {
    Switch(Switch),
    Remove { id: String, slot: CredentialSlot },
}

#[derive(Debug, Serialize, Deserialize)]
struct Switch {
    id: String,
    previous: Option<CredentialSlot>,
    previous_source: Option<String>,
    next: Option<CredentialSlot>,
    file_only: bool,
}

fn prepare(home: &Path, change: &Change) -> Result<(), String> {
    crate::json_file::write(&home.join(JOURNAL), change)
        .map_err(|e| format!("could not prepare changes to models.json: {e}"))
}

fn lock_record(file: File) -> Result<Arc<File>, String> {
    file.try_lock()
        .map_err(|e| format!("could not lock the model change record: {e}"))?;
    Ok(Arc::new(file))
}

fn publish(home: &Path, file: &ModelsFile) -> Result<(), String> {
    save_models(home, file).map_err(|e| format!("could not write models.json: {e}"))
}

fn clear(home: &Path) -> Result<(), String> {
    fs::remove_file(home.join(JOURNAL))
        .map_err(|e| format!("could not clear the model change record: {e}"))
}

fn in_use(file: &ModelsFile, account: &str, except: Option<&str>) -> bool {
    file.profiles.iter().any(|p| {
        Some(p.id.as_str()) != except
            && stored(p).is_some_and(|slot| slot.account(&p.id) == account)
    })
}

impl Switch {
    fn clean(
        &self,
        home: &Path,
        file: &ModelsFile,
        store: &mut SecretStore,
        guard: &Arc<File>,
    ) -> Result<(), String> {
        let committed = find(file, &self.id).is_some_and(|p| stored(p) == self.next);
        let (slot, source) = if committed {
            (self.previous, self.previous_source.as_deref())
        } else {
            (self.next, self.file_only.then_some("file"))
        };
        if let Some(slot) = slot {
            let account = slot.account(&self.id);
            if !in_use(file, &account, None) {
                store.remove_model_credential(home, &account, slot, source, guard)?;
            }
        }
        clear(home)
    }
}

/// Called under the runtime's model writer lock before each mutation, and before model resolution at boot.
/// On a failed final save of a deletion, `file` still describes the effective (removed) profile.
pub fn recover_change(
    home: &Path,
    file: &mut ModelsFile,
    store: &mut SecretStore,
) -> Result<Option<String>, String> {
    let path = home.join(JOURNAL);
    let record = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read the model change record: {e}")),
    };
    let guard = lock_record(record)?;
    let change = serde_json::from_reader(&*guard)
        .map_err(|_| "invalid model change record; credential cleanup stopped".to_string())?;
    match change {
        Change::Switch(switch) => {
            switch.clean(home, file, store, &guard)?;
            Ok(None)
        }
        Change::Remove { id, slot } => {
            if let Some(p) = find(file, &id) {
                if stored(p) != Some(slot) {
                    return Err(
                        "model profile was edited during removal; credential cleanup stopped"
                            .into(),
                    );
                }
                let account = slot.account(&id);
                if !in_use(file, &account, Some(&id)) {
                    store.remove_model_credential(
                        home,
                        &account,
                        slot,
                        p.key_source.as_deref(),
                        &guard,
                    )?;
                }
                remove(file, &id);
                publish(home, file)?;
            }
            clear(home)?;
            Ok(Some(id))
        }
    }
}

pub fn save_profile(
    home: &Path,
    file: &mut ModelsFile,
    store: &mut SecretStore,
    mut profile: ModelProfile,
    key: Option<&str>,
    make_active: bool,
) -> Result<String, String> {
    let id = profile.id.clone();
    let previous = find(file, &id).and_then(stored);
    let previous_source = find(file, &id).and_then(|p| p.key_source.clone());
    let next = match key {
        Some(_) => Some(previous.unwrap_or_default().next()),
        None => stored(&profile),
    };
    let switching = next != previous;
    let file_only = key.is_some() && store.model_writes_to_file();
    if switching {
        if next.is_some_and(|slot| in_use(file, &slot.account(&id), None)) {
            return Err("a model credential account is already in use".into());
        }
        prepare(
            home,
            &Change::Switch(Switch {
                id: id.clone(),
                previous,
                previous_source,
                next,
                file_only,
            }),
        )?;
    }
    let mut candidate = file.clone();
    let result = (|| {
        if let (Some(key), Some(slot)) = (key, next) {
            let account = slot.account(&id);
            let source = if file_only {
                super::write_file_secret(home, &account, key)?;
                "file"
            } else {
                let record = File::open(home.join(JOURNAL))
                    .map_err(|e| format!("could not open the model change record: {e}"))?;
                let guard = lock_record(record)?;
                store.put_guarded(home, &account, key, Some(&guard))?
            };
            profile.key_source = Some(source.into());
            profile.credential_slot = slot;
        }
        let source = profile.key_source.clone().unwrap_or_else(|| "none".into());
        upsert(&mut candidate, profile);
        if make_active {
            candidate.active = Some(id.clone());
        }
        publish(home, &candidate)?;
        *file = candidate;
        Ok(source)
    })();
    if switching {
        if let Err(e) = recover_change(home, file, store) {
            eprintln!("(jeeves) model credential cleanup pending: {e}");
        }
    }
    result
}

pub fn remove_profile(
    home: &Path,
    file: &mut ModelsFile,
    store: &mut SecretStore,
    id: &str,
) -> Result<bool, String> {
    let Some(profile) = find(file, id) else {
        return Ok(false);
    };
    if let Some(slot) = stored(profile) {
        prepare(
            home,
            &Change::Remove {
                id: id.into(),
                slot,
            },
        )?;
        recover_change(home, file, store)?;
    } else {
        let mut candidate = file.clone();
        remove(&mut candidate, id);
        publish(home, &candidate)?;
        *file = candidate;
    }
    Ok(true)
}
