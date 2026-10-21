//! Health checks for `tern doctor`. Every message is plain language and says
//! what to do next when something is wrong.

use serde::Serialize;
use tern_platform::disk::human_bytes;
use tern_platform::{AppPaths, DiskSpace, SecretStore, SecretString};
use tern_store::{SCHEMA_VERSION, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub os: &'static str,
    pub checks: Vec<Check>,
}

impl Report {
    /// The worst status across all checks.
    pub fn overall(&self) -> Status {
        if self.checks.iter().any(|c| c.status == Status::Fail) {
            Status::Fail
        } else if self.checks.iter().any(|c| c.status == Status::Warn) {
            Status::Warn
        } else {
            Status::Ok
        }
    }
}

/// What the doctor inspects. Injected so tests never touch the real keychain.
#[derive(Debug)]
pub struct Inputs<'a> {
    pub paths: Result<AppPaths, String>,
    pub secrets: Result<&'a dyn SecretStore, String>,
}

const PROBE: &str = "doctor.probe";

/// Runs every check against the real data directory and OS keychain.
pub fn run_on_this_machine() -> Report {
    let paths = AppPaths::discover().map_err(|e| e.to_string());
    match tern_platform::default_secret_store() {
        Ok(store) => run(Inputs {
            paths,
            secrets: Ok(store.as_ref()),
        }),
        Err(e) => run(Inputs {
            paths,
            secrets: Err(e.to_string()),
        }),
    }
}

pub fn run(inputs: Inputs<'_>) -> Report {
    let mut checks = Vec::new();
    checks.push(keychain(inputs.secrets));
    match inputs.paths {
        Ok(paths) => {
            let dir = data_dir(&paths);
            let dir_ok = dir.status == Status::Ok;
            checks.push(dir);
            if dir_ok {
                checks.push(disk(&paths));
                checks.push(database(&paths));
            }
        }
        Err(e) => checks.push(Check {
            name: "Data directory",
            status: Status::Fail,
            detail: format!(
                "Could not find where to keep Tern's data: {e}. Set TERN_HOME to a folder you own."
            ),
        }),
    }
    Report {
        version: env!("CARGO_PKG_VERSION"),
        os: tern_platform::os_name(),
        checks,
    }
}

fn keychain(store: Result<&dyn SecretStore, String>) -> Check {
    let name = "Keychain";
    let store = match store {
        Ok(s) => s,
        Err(e) => {
            return Check {
                name,
                status: Status::Fail,
                detail: format!("{}. API keys cannot be saved on this machine yet.", capitalise(&e)),
            };
        }
    };
    let value = SecretString::new("tern-doctor-probe");
    let result = store
        .set(PROBE, &value)
        .and_then(|()| store.get(PROBE))
        .and_then(|got| store.delete(PROBE).map(|_| got));
    match result {
        Ok(Some(got)) if got == value => Check {
            name,
            status: Status::Ok,
            detail: format!("{} stored, read, and deleted a test key.", store.backend()),
        },
        Ok(_) => Check {
            name,
            status: Status::Fail,
            detail: format!(
                "{} accepted a test key but returned something else.",
                store.backend()
            ),
        },
        Err(e) => {
            let _ = store.delete(PROBE);
            Check {
                name,
                status: Status::Fail,
                detail: format!("{} refused a test key: {e}.", store.backend()),
            }
        }
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn data_dir(paths: &AppPaths) -> Check {
    let name = "Data directory";
    match paths.check_writable() {
        Ok(()) => Check {
            name,
            status: Status::Ok,
            detail: format!("{} is writable.", paths.data_dir.display()),
        },
        Err(e) => Check {
            name,
            status: Status::Fail,
            detail: format!(
                "{} is not writable: {e}. Check its permissions or set TERN_HOME.",
                paths.data_dir.display()
            ),
        },
    }
}

fn disk(paths: &AppPaths) -> Check {
    let name = "Disk space";
    match DiskSpace::of(&paths.data_dir) {
        Ok(d) if d.is_low() => Check {
            name,
            status: Status::Warn,
            detail: format!(
                "Only {} free of {}. Sources and caches need room; free some space soon.",
                human_bytes(d.available),
                human_bytes(d.total)
            ),
        },
        Ok(d) => Check {
            name,
            status: Status::Ok,
            detail: format!(
                "{} free of {}.",
                human_bytes(d.available),
                human_bytes(d.total)
            ),
        },
        Err(e) => Check {
            name,
            status: Status::Warn,
            detail: format!("Could not read free space: {e}."),
        },
    }
}

fn database(paths: &AppPaths) -> Check {
    let name = "Database";
    let path = paths.database();
    match Store::open(&path).and_then(|s| Ok((s.schema_version()?, s.integrity_ok()?))) {
        Ok((v, true)) => Check {
            name,
            status: Status::Ok,
            detail: format!(
                "{} opens, schema version {v} of {SCHEMA_VERSION}, integrity ok.",
                path.display()
            ),
        },
        Ok((_, false)) => Check {
            name,
            status: Status::Fail,
            detail: format!(
                "{} failed SQLite's integrity check. Keep a copy before changing anything.",
                path.display()
            ),
        },
        Err(e) => Check {
            name,
            status: Status::Fail,
            detail: format!("{} could not be opened: {e}.", path.display()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_platform::MemorySecretStore;

    #[test]
    fn healthy_machine_reports_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemorySecretStore::default();
        let r = run(Inputs {
            paths: Ok(AppPaths::at(tmp.path())),
            secrets: Ok(&store),
        });
        let names: Vec<_> = r.checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            ["Keychain", "Data directory", "Disk space", "Database"]
        );
        assert!(r.checks.iter().all(|c| c.status != Status::Fail), "{r:#?}");
        assert!(store.get(PROBE).unwrap().is_none(), "probe key left behind");
        assert!(tmp.path().join("tern.sqlite3").exists());
    }

    #[test]
    fn missing_keychain_fails_in_plain_words() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run(Inputs {
            paths: Ok(AppPaths::at(tmp.path())),
            secrets: Err("not supported".into()),
        });
        assert_eq!(r.checks[0].status, Status::Fail);
        assert_eq!(r.overall(), Status::Fail);
        assert!(r.checks[0].detail.contains("cannot be saved"));
    }

    #[test]
    fn report_serialises_for_json_output() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemorySecretStore::default();
        let r = run(Inputs {
            paths: Ok(AppPaths::at(tmp.path())),
            secrets: Ok(&store),
        });
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["checks"][0]["status"], "ok");
    }
}
