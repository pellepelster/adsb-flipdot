//! Registration and type of an aircraft, from its ICAO address.
//!
//! ADS-B itself does not say what kind of aircraft is flying. An
//! [`AircraftDb`] holds the database maintained for tar1090/readsb in
//! memory. Where the database comes from is not decided here: anything that
//! can answer for an address is a [`Registry`].

use std::io::BufRead;

use anyhow::{Context, Result};

/// What is known about one aircraft.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AircraftInfo {
    /// Tail number, e.g. `G-EZTB`.
    pub registration: Option<String>,
    /// ICAO type designator, e.g. `A320`.
    pub typecode: Option<String>,
    /// Type in words, e.g. `AIRBUS A-320`.
    pub type_name: Option<String>,
}

/// Something that knows aircraft by their address.
pub trait Registry {
    fn get(&self, icao: u32) -> Option<AircraftInfo>;

    /// What it knows, in a few words for a status line.
    fn status(&self) -> String;
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
    /// Parse the uncompressed CSV: `icao;registration;type;flags;name;…`.
    /// Malformed lines and aircraft without any information are skipped.
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

impl Registry for AircraftDb {
    fn get(&self, icao: u32) -> Option<AircraftInfo> {
        AircraftDb::get(self, icao)
    }

    fn status(&self) -> String {
        format!("{} aircraft known", self.len())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
    fn a_database_is_a_registry() {
        let registry: Box<dyn Registry> = Box::new(sample_db());
        assert_eq!(registry.status(), "6 aircraft known");
        assert_eq!(
            registry.get(0x405f12).unwrap().typecode.as_deref(),
            Some("A320")
        );
        assert_eq!(registry.get(0x000000), None);
    }
}
