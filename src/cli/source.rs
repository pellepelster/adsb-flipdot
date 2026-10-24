//! Options selecting where frames come from.

use std::path::PathBuf;

use crate::cli::parse;
use crate::source::stream::Source;

/// Options selecting where frames come from. Shared by all subcommands.
#[derive(clap::Args, Debug)]
pub struct SourceArgs {
    /// RTL-SDR device index.
    #[arg(long, default_value_t = 0)]
    pub device: usize,

    /// Fixed tuner gain in tenths of a dB (496 = 49.6 dB). Without it the
    /// tuner's automatic gain control sets the gain.
    #[arg(long, value_name = "TENTHS_DB")]
    pub gain: Option<i32>,

    /// Replay a recorded IQ file instead of opening the dongle: 2.4 MS/s of
    /// unsigned 8-bit I/Q pairs, as `rtl_sdr` writes them.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

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
            Source::IqFile { path: path.clone() }
        } else {
            Source::Dongle {
                device: self.device,
                gain: self.gain,
            }
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
                gain: None
            },
            "automatic gain control unless a gain is given"
        );
        assert_eq!(
            source(&["--device", "2", "--gain", "200"]),
            Source::Dongle {
                device: 2,
                gain: Some(200)
            }
        );
        assert!(
            try_parse_args::<SourceArgs>(&["--agc"]).is_err(),
            "automatic gain control is the default"
        );
        assert_eq!(
            source(&["--file", "iq.bin"]),
            Source::IqFile {
                path: "iq.bin".into()
            }
        );
        assert!(
            try_parse_args::<SourceArgs>(&["--file", "iq.bin", "--iq-format", "cs16"]).is_err(),
            "the format is always cu8"
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
