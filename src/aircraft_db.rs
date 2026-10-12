//! Aircraft database: registration and type for an ICAO address.
//!
//! ADS-B itself does not say what kind of aircraft is flying. This module
//! downloads the database maintained for tar1090/readsb once, keeps it in a
//! cache directory and looks aircraft up locally. Nothing is sent anywhere
//! per aircraft, and a Raspberry Pi without network keeps working from the
//! cached file.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

/// Semicolon separated, gzip compressed: `icao;registration;type;flags;name;…`.
const DATABASE_URL: &str =
    "https://raw.githubusercontent.com/wiedehopf/tar1090-db/csv/aircraft.csv.gz";
const DATABASE_FILE: &str = "aircraft.csv.gz";
/// Download a new copy when the cached one is older than this.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Refuse absurdly large downloads. The file is about 8 MB.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Options for the aircraft database. Shared by the commands that use it.
#[derive(clap::Args, Debug, Clone)]
pub struct DbArgs {
    /// Directory for downloaded data. Defaults to $XDG_CACHE_HOME/adsb-flipdot
    /// or ~/.cache/adsb-flipdot.
    #[arg(long, value_name = "DIR", env = "ADSB_FLIPDOT_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Do not download or use the aircraft database. Aircraft type and
    /// registration are then not shown.
    #[arg(long)]
    pub no_aircraft_db: bool,
}

impl DbArgs {
    /// Start loading the database in the background, unless disabled.
    pub fn start(&self) -> Lookup {
        if self.no_aircraft_db {
            return Lookup::disabled();
        }
        Lookup::start(self.cache_dir.clone().unwrap_or_else(default_cache_dir))
    }
}

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

/// What the database knows about one aircraft.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AircraftInfo {
    /// Tail number, e.g. `G-EZTB`.
    pub registration: Option<String>,
    /// ICAO type designator, e.g. `A320`.
    pub typecode: Option<String>,
    /// Type in words, e.g. `AIRBUS A-320`.
    pub type_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    icao: u32,
    /// Start of this aircraft's text in `AircraftDb::text`.
    offset: u32,
    registration_len: u8,
    typecode_len: u8,
    type_name_len: u8,
}

/// The whole database in memory, about 20 MB for 600,000 aircraft: an index
/// sorted by address plus one block of text, instead of three strings per
/// aircraft.
#[derive(Debug, Default)]
pub struct AircraftDb {
    entries: Vec<Entry>,
    text: String,
}

impl AircraftDb {
    /// Parse the uncompressed CSV. Malformed lines and aircraft without any
    /// information are skipped.
    pub fn parse(reader: impl BufRead) -> Result<Self> {
        let mut db = Self::default();
        for line in reader.lines() {
            let line = line.context("cannot read aircraft database")?;
            let mut fields = line.split(';');
            let Some(icao) = fields
                .next()
                .and_then(|hex| u32::from_str_radix(hex.trim(), 16).ok())
            else {
                continue;
            };
            let registration = fields.next().unwrap_or("").trim();
            let typecode = fields.next().unwrap_or("").trim();
            let _flags = fields.next();
            let type_name = fields.next().unwrap_or("").trim();
            if registration.is_empty() && typecode.is_empty() {
                continue;
            }
            // Lengths are stored in one byte; real values are far shorter.
            let (Ok(registration_len), Ok(typecode_len), Ok(type_name_len), Ok(offset)) = (
                u8::try_from(registration.len()),
                u8::try_from(typecode.len()),
                u8::try_from(type_name.len()),
                u32::try_from(db.text.len()),
            ) else {
                continue;
            };
            db.text.push_str(registration);
            db.text.push_str(typecode);
            db.text.push_str(type_name);
            db.entries.push(Entry {
                icao,
                offset,
                registration_len,
                typecode_len,
                type_name_len,
            });
        }
        // The file is sorted already; do not rely on it. Stable, so the
        // first of two lines for one address wins.
        db.entries.sort_by_key(|e| e.icao);
        Ok(db)
    }

    /// Read a gzip compressed database file.
    pub fn read_file(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let db = Self::parse(BufReader::new(GzDecoder::new(file)))
            .with_context(|| format!("cannot read {}", path.display()))?;
        if db.is_empty() {
            bail!("{} contains no aircraft", path.display());
        }
        Ok(db)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, icao: u32) -> Option<AircraftInfo> {
        let index = self.entries.partition_point(|e| e.icao < icao);
        let entry = self.entries.get(index).filter(|e| e.icao == icao)?;
        let mut at = entry.offset as usize;
        let mut take = |len: u8| {
            let text = &self.text[at..at + len as usize];
            at += len as usize;
            (!text.is_empty()).then(|| text.to_string())
        };
        Some(AircraftInfo {
            registration: take(entry.registration_len),
            typecode: take(entry.typecode_len),
            type_name: take(entry.type_name_len),
        })
    }
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
        AircraftDb::read_file(&partial).context("downloaded file is not a valid database")?;
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
    Ok((AircraftDb::read_file(&path)?, warning))
}

#[derive(Debug)]
enum State {
    Disabled,
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
/// [`Lookup::status`] themselves.
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

    /// No database: every lookup finds nothing.
    pub fn disabled() -> Self {
        Self::with(Some(State::Disabled))
    }

    /// An already loaded database.
    #[cfg(test)]
    pub fn ready(db: AircraftDb) -> Self {
        Self::with(Some(State::Ready { db, warning: None }))
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

    pub fn get(&self, icao: u32) -> Option<AircraftInfo> {
        match self.state.get()? {
            State::Ready { db, .. } => db.get(icao),
            _ => None,
        }
    }

    /// Short description of the database state for status lines.
    pub fn status(&self) -> String {
        match self.state.get() {
            None => "aircraft db loading".to_string(),
            Some(State::Disabled) => "aircraft db off".to_string(),
            Some(State::Ready { db, warning: None }) => format!("{} aircraft known", db.len()),
            Some(State::Ready {
                db,
                warning: Some(warning),
            }) => format!("{} aircraft known, {warning}", db.len()),
            Some(State::Failed(error)) => format!("aircraft db unavailable: {error}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    /// Lines in the real file's format, deliberately out of order.
    pub(crate) const SAMPLE: &str = "\
405F12;G-EZTB;A320;00;AIRBUS A-320;;;
000001;;;10;;;Miscode - VARIOUS;
3C0AC5;D-ATCA;A321;00;AIRBUS A-321;;;
48ADA7;SP-LNH;E195;00;EMBRAER ERJ-190-200;;;
ABCDEF;N12345;;00;;;;
123456;;C172;00;CESSNA 172 Skyhawk;;;
not a line
;;;;
3C0AC5;D-XXXX;B744;00;DUPLICATE;;;
";

    pub(crate) fn sample_db() -> AircraftDb {
        AircraftDb::parse(SAMPLE.as_bytes()).unwrap()
    }

    fn info(reg: Option<&str>, code: Option<&str>, name: Option<&str>) -> AircraftInfo {
        AircraftInfo {
            registration: reg.map(str::to_string),
            typecode: code.map(str::to_string),
            type_name: name.map(str::to_string),
        }
    }

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
    fn lookup_returns_registration_type_and_name() {
        let db = sample_db();
        assert_eq!(
            db.get(0x405f12),
            Some(info(Some("G-EZTB"), Some("A320"), Some("AIRBUS A-320")))
        );
        assert_eq!(
            db.get(0x48ada7),
            Some(info(
                Some("SP-LNH"),
                Some("E195"),
                Some("EMBRAER ERJ-190-200")
            ))
        );
    }

    #[test]
    fn missing_fields_are_none() {
        let db = sample_db();
        assert_eq!(db.get(0xabcdef), Some(info(Some("N12345"), None, None)));
        assert_eq!(
            db.get(0x123456),
            Some(info(None, Some("C172"), Some("CESSNA 172 Skyhawk")))
        );
    }

    #[test]
    fn unknown_and_empty_aircraft_are_not_found() {
        let db = sample_db();
        assert_eq!(db.get(0x000000), None);
        assert_eq!(db.get(0xffffff), None);
        assert_eq!(db.get(0x405f13), None, "neighbouring address");
        assert_eq!(db.get(0x000001), None, "line without registration and type");
        assert_eq!(db.len(), 6, "malformed and empty lines are skipped");
    }

    #[test]
    fn first_line_wins_for_a_duplicate_address() {
        assert_eq!(
            sample_db().get(0x3c0ac5).unwrap().registration.as_deref(),
            Some("D-ATCA")
        );
    }

    #[test]
    fn gzip_file_round_trips() {
        let dir = TempDir::new("roundtrip");
        let path = dir.0.join(DATABASE_FILE);
        write_gz(&path, SAMPLE);
        let db = AircraftDb::read_file(&path).unwrap();
        assert_eq!(db.len(), 6);
        assert_eq!(db.get(0x3c0ac5).unwrap().typecode.as_deref(), Some("A321"));
    }

    #[test]
    fn broken_files_are_rejected() {
        let dir = TempDir::new("broken");
        let path = dir.0.join(DATABASE_FILE);
        assert!(AircraftDb::read_file(&path).is_err(), "missing file");
        std::fs::write(&path, b"this is not gzip").unwrap();
        assert!(AircraftDb::read_file(&path).is_err(), "not gzip");
        write_gz(&path, "no;aircraft;here\n");
        assert!(AircraftDb::read_file(&path).is_err(), "no usable line");
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
        let ready = Lookup::ready(sample_db());
        assert_eq!(ready.status(), "6 aircraft known");
        assert_eq!(
            ready.get(0x405f12).unwrap().typecode.as_deref(),
            Some("A320")
        );

        let disabled = Lookup::disabled();
        assert_eq!(disabled.status(), "aircraft db off");
        assert_eq!(disabled.get(0x405f12), None);

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
    fn disabled_by_flag() {
        let args = DbArgs {
            cache_dir: None,
            no_aircraft_db: true,
        };
        assert_eq!(args.start().status(), "aircraft db off");
    }

    #[test]
    fn default_cache_dir_ends_with_the_program_name() {
        assert!(default_cache_dir().ends_with("adsb-flipdot"));
    }
}
