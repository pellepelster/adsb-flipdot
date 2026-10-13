//! Where the aircraft database comes from.
//!
//! The database maintained for tar1090/readsb is downloaded once, kept in a
//! cache directory and read from there. Nothing is sent anywhere per
//! aircraft, and a Raspberry Pi without network keeps working from the
//! cached file.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

use crate::domain::registry::{AircraftDb, AircraftInfo, Registry};

/// Semicolon separated, gzip compressed: `icao;registration;type;flags;name;…`.
const DATABASE_URL: &str =
    "https://raw.githubusercontent.com/wiedehopf/tar1090-db/csv/aircraft.csv.gz";
const DATABASE_FILE: &str = "aircraft.csv.gz";
/// Download a new copy when the cached one is older than this.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Refuse absurdly large downloads. The file is about 8 MB.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// `$XDG_CACHE_HOME/adsb-flipdot`, else `~/.cache/adsb-flipdot`, else a
/// directory under the system temp dir.
pub fn default_cache_dir() -> PathBuf {
    let non_empty = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let base = non_empty("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("adsb-flipdot")
}

/// Read a gzip compressed database file.
fn read_file(path: &Path) -> Result<AircraftDb> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let db = AircraftDb::parse(BufReader::new(GzDecoder::new(file)))
        .with_context(|| format!("cannot read {}", path.display()))?;
    if db.is_empty() {
        bail!("{} contains no aircraft", path.display());
    }
    Ok(db)
}

/// True if `path` exists and was written less than `max_age` ago.
fn is_fresh(path: &Path, max_age: Duration) -> bool {
    path.metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|modified| {
            // A timestamp in the future counts as fresh.
            SystemTime::now()
                .duration_since(modified)
                .map_or(true, |age| age < max_age)
        })
}

/// Download `url` to `path`. The file appears only once complete, so an
/// interrupted download never leaves a broken database behind.
fn download(url: &str, path: &Path) -> Result<()> {
    let dir = path.parent().context("cache file has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;

    let partial = path.with_extension("part");
    let result = (|| -> Result<()> {
        let mut file = File::create(&partial)
            .with_context(|| format!("cannot write {}", partial.display()))?;
        let mut body = response.body_mut().as_reader().take(MAX_DOWNLOAD_BYTES + 1);
        let bytes = std::io::copy(&mut body, &mut file)
            .with_context(|| format!("download of {url} was interrupted"))?;
        if bytes > MAX_DOWNLOAD_BYTES {
            bail!("{url} is larger than {MAX_DOWNLOAD_BYTES} bytes");
        }
        file.sync_all()?;
        // Reject a truncated or wrong file before it replaces a good one.
        read_file(&partial).context("downloaded file is not a valid database")?;
        std::fs::rename(&partial, path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Load the database from `cache_dir`, downloading from `url` when the
/// cached file is missing or older than `max_age`.
///
/// A failed refresh is not fatal: the old file keeps being used.
fn load(cache_dir: &Path, url: &str, max_age: Duration) -> Result<(AircraftDb, Option<String>)> {
    let path = cache_dir.join(DATABASE_FILE);
    let mut warning = None;
    if !is_fresh(&path, max_age)
        && let Err(e) = download(url, &path)
    {
        if !path.exists() {
            return Err(e);
        }
        warning = Some(format!("refresh failed, using old copy ({e:#})"));
    }
    Ok((read_file(&path)?, warning))
}

#[derive(Debug)]
enum State {
    Ready {
        db: AircraftDb,
        warning: Option<String>,
    },
    Failed(String),
}

/// A database that loads in the background. Lookups simply find nothing
/// until it is ready, so nothing waits for a download.
///
/// The loader thread never prints: commands with a full-screen UI show
/// the status themselves.
#[derive(Debug, Clone)]
pub struct Lookup {
    state: Arc<OnceLock<State>>,
}

impl Lookup {
    fn with(state: Option<State>) -> Self {
        let cell = OnceLock::new();
        if let Some(state) = state {
            let _ = cell.set(state);
        }
        Self {
            state: Arc::new(cell),
        }
    }

    /// Load from `cache_dir` on a background thread, downloading if needed.
    pub fn start(cache_dir: PathBuf) -> Self {
        let lookup = Self::with(None);
        let state = Arc::clone(&lookup.state);
        std::thread::spawn(move || {
            let loaded = match load(&cache_dir, DATABASE_URL, MAX_AGE) {
                Ok((db, warning)) => State::Ready { db, warning },
                Err(e) => State::Failed(format!("{e:#}")),
            };
            let _ = state.set(loaded);
        });
        lookup
    }
}

impl Registry for Lookup {
    fn get(&self, icao: u32) -> Option<AircraftInfo> {
        match self.state.get()? {
            State::Ready { db, .. } => db.get(icao),
            State::Failed(_) => None,
        }
    }

    fn status(&self) -> String {
        match self.state.get() {
            None => "aircraft db loading".to_string(),
            Some(State::Ready { db, warning: None }) => db.status(),
            Some(State::Ready {
                db,
                warning: Some(warning),
            }) => format!("{}, {warning}", db.status()),
            Some(State::Failed(error)) => format!("aircraft db unavailable: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::registry::tests::{SAMPLE, sample_db};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    /// A fresh directory under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("adsb-flipdot-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_gz(path: &Path, content: &str) {
        let mut encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        encoder.write_all(content.as_bytes()).unwrap();
        encoder.finish().unwrap();
    }

    /// A URL nothing listens on, so a download attempt fails at once.
    const DEAD_URL: &str = "http://127.0.0.1:9/aircraft.csv.gz";

    #[test]
    fn gzip_file_round_trips() {
        let dir = TempDir::new("roundtrip");
        let path = dir.0.join(DATABASE_FILE);
        write_gz(&path, SAMPLE);
        let db = read_file(&path).unwrap();
        assert_eq!(db.len(), 6);
        assert_eq!(db.get(0x3c0ac5).unwrap().typecode.as_deref(), Some("A321"));
    }

    #[test]
    fn broken_files_are_rejected() {
        let dir = TempDir::new("broken");
        let path = dir.0.join(DATABASE_FILE);
        assert!(read_file(&path).is_err(), "missing file");
        std::fs::write(&path, b"this is not gzip").unwrap();
        assert!(read_file(&path).is_err(), "not gzip");
        write_gz(&path, "no;aircraft;here\n");
        assert!(read_file(&path).is_err(), "no usable line");
    }

    #[test]
    fn fresh_cache_is_used_without_downloading() {
        let dir = TempDir::new("fresh");
        write_gz(&dir.0.join(DATABASE_FILE), SAMPLE);
        // The URL is dead: this only works if no download is attempted.
        let (db, warning) = load(&dir.0, DEAD_URL, MAX_AGE).unwrap();
        assert_eq!(db.len(), 6);
        assert_eq!(warning, None);
    }

    #[test]
    fn stale_cache_is_kept_when_the_refresh_fails() {
        let dir = TempDir::new("stale");
        write_gz(&dir.0.join(DATABASE_FILE), SAMPLE);
        // Max age zero: the file counts as too old and a refresh is tried.
        let (db, warning) = load(&dir.0, DEAD_URL, Duration::ZERO).unwrap();
        assert_eq!(db.len(), 6, "old copy still used");
        assert!(
            warning
                .unwrap()
                .starts_with("refresh failed, using old copy")
        );
        assert!(
            !dir.0.join("aircraft.csv.part").exists(),
            "no partial file left"
        );
    }

    #[test]
    fn no_cache_and_no_network_is_an_error() {
        let dir = TempDir::new("empty");
        let error = load(&dir.0.join("nested"), DEAD_URL, MAX_AGE).unwrap_err();
        assert!(
            format!("{error:#}").contains("cannot download"),
            "{error:#}"
        );
        assert!(dir.0.join("nested").is_dir(), "cache directory was created");
    }

    #[test]
    fn lookup_states_report_their_status() {
        let ready = Lookup::with(Some(State::Ready {
            db: sample_db(),
            warning: None,
        }));
        assert_eq!(ready.status(), "6 aircraft known");
        assert_eq!(
            ready.get(0x405f12).unwrap().typecode.as_deref(),
            Some("A320")
        );

        let stale = Lookup::with(Some(State::Ready {
            db: sample_db(),
            warning: Some("refresh failed".to_string()),
        }));
        assert_eq!(stale.status(), "6 aircraft known, refresh failed");

        let loading = Lookup::with(None);
        assert_eq!(loading.status(), "aircraft db loading");
        assert_eq!(loading.get(0x405f12), None, "finds nothing until loaded");

        let failed = Lookup::with(Some(State::Failed("no network".to_string())));
        assert_eq!(failed.status(), "aircraft db unavailable: no network");
        assert_eq!(failed.get(0x405f12), None);
    }

    #[test]
    fn background_load_reads_the_cache_directory() {
        let dir = TempDir::new("background");
        write_gz(&dir.0.join(DATABASE_FILE), SAMPLE);
        let lookup = Lookup::start(dir.0.clone());
        for _ in 0..200 {
            if lookup.state.get().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lookup.status(), "6 aircraft known");
        assert_eq!(
            lookup.get(0x48ada7).unwrap().registration.as_deref(),
            Some("SP-LNH")
        );
    }

    #[test]
    fn default_cache_dir_ends_with_the_program_name() {
        assert!(default_cache_dir().ends_with("adsb-flipdot"));
    }
}
