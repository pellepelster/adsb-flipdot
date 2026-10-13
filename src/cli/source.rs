//! Options selecting where frames come from.

use std::path::PathBuf;

use clap::ValueEnum;
use desperado::IqFormat;

use crate::cli::parse;
use crate::source::stream::Source;

/// Options selecting where frames come from. Shared by all subcommands.
#[derive(clap::Args, Debug)]
pub struct SourceArgs {
    /// RTL-SDR device index.
    #[arg(long, default_value_t = 0)]
    pub device: usize,

    /// Tuner gain in tenths of a dB (496 = 49.6 dB). Ignored with --agc.
    #[arg(long, default_value_t = 496)]
    pub gain: i32,

    /// Use the tuner's automatic gain control instead of a fixed gain.
    #[arg(long)]
    pub agc: bool,

    /// Replay a recorded IQ file (2.4 MS/s) instead of opening the dongle.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Sample format of the IQ file given with --file.
    #[arg(long, value_enum, default_value_t = IqFileFormat::Cu8)]
    pub iq_format: IqFileFormat,

    /// Replay frames recorded with `debug record` instead of opening the
    /// dongle. Frames arrive with the timing they were recorded with.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["device", "file"])]
    pub device_record_file: Option<PathBuf>,

    /// Replay the recording this many times faster than it was recorded.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0,
          value_parser = parse::positive, requires = "device_record_file")]
    pub device_record_speed: f64,

    /// Start the recording over when it ends, instead of stopping.
    #[arg(long, requires = "device_record_file")]
    pub device_record_loop: bool,
}

impl SourceArgs {
    /// The source these options select: a recording if one is given, else
    /// an IQ file if one is given, else the dongle.
    pub fn source(&self) -> Source {
        if let Some(path) = &self.device_record_file {
            Source::Recording {
                path: path.clone(),
                speed: self.device_record_speed,
                looping: self.device_record_loop,
            }
        } else if let Some(path) = &self.file {
            Source::IqFile {
                path: path.clone(),
                format: self.iq_format.into(),
            }
        } else {
            Source::Dongle {
                device: self.device,
                gain: (!self.agc).then_some(self.gain),
            }
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum IqFileFormat {
    /// Unsigned 8-bit I/Q pairs (what rtl_sdr writes).
    Cu8,
    /// Signed 8-bit I/Q pairs.
    Cs8,
    /// Signed 16-bit little-endian I/Q pairs.
    Cs16,
}

impl From<IqFileFormat> for IqFormat {
    fn from(f: IqFileFormat) -> Self {
        match f {
            IqFileFormat::Cu8 => IqFormat::Cu8,
            IqFileFormat::Cs8 => IqFormat::Cs8,
            IqFileFormat::Cs16 => IqFormat::Cs16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tests::{parse_args, try_parse_args};

    fn source(extra: &[&str]) -> Source {
        parse_args::<SourceArgs>(extra).source()
    }

    #[test]
    fn the_dongle_is_the_source_unless_a_file_is_given() {
        assert_eq!(
            source(&[]),
            Source::Dongle {
                device: 0,
                gain: Some(496)
            }
        );
        assert_eq!(
            source(&["--device", "2", "--gain", "200"]),
            Source::Dongle {
                device: 2,
                gain: Some(200)
            }
        );
        assert_eq!(
            source(&["--agc"]),
            Source::Dongle {
                device: 0,
                gain: None
            },
            "automatic gain control: no fixed gain"
        );
        assert_eq!(
            source(&["--file", "iq.bin", "--iq-format", "cs16"]),
            Source::IqFile {
                path: "iq.bin".into(),
                format: IqFormat::Cs16
            }
        );
        assert_eq!(
            source(&[
                "--device-record-file",
                "a.jsonl",
                "--device-record-speed",
                "4",
                "--device-record-loop"
            ]),
            Source::Recording {
                path: "a.jsonl".into(),
                speed: 4.0,
                looping: true
            }
        );
    }

    #[test]
    fn record_file_excludes_device_and_iq_file() {
        let parse = |extra: &[&str]| try_parse_args::<SourceArgs>(extra);
        assert!(parse(&["--device-record-file", "a.jsonl"]).is_ok());
        assert!(parse(&["--device-record-file", "a.jsonl", "--device", "1"]).is_err());
        assert!(parse(&["--device-record-file", "a.jsonl", "--file", "iq.bin"]).is_err());
        assert!(
            parse(&["--device-record-speed", "2"]).is_err(),
            "needs a recording"
        );
        assert!(
            parse(&["--device-record-loop"]).is_err(),
            "needs a recording"
        );
        assert!(parse(&["--device-record-file", "a", "--device-record-speed", "0"]).is_err());
    }
}
