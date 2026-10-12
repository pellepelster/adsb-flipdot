//! Shared frame source: RTL-SDR dongle or IQ file -> demodulated Mode S frames.
//!
//! Pipeline: rs-rtl (pure Rust USB driver) -> desperado (IQ source) ->
//! rs1090 demodulator at 2.4 MS/s.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::ValueEnum;
use desperado::{IqAsyncSource, IqFormat};
use rs1090::decode::TimedMessage;
use rs1090::source::iqread;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// ADS-B downlink frequency in Hz.
const CENTER_FREQ_HZ: u32 = 1_090_000_000;
/// Sample rate the rs1090 demodulator expects. 6 MS/s also exists but is too
/// heavy for a Raspberry Pi, so it is deliberately not exposed.
const SAMPLE_RATE_HZ: u32 = 2_400_000;
/// Samples per chunk when replaying a file. Roughly 55 ms of signal.
const FILE_CHUNK_SAMPLES: usize = 131_072;
/// Long (112 bit) and short (56 bit) Mode S frame sizes in bytes.
const LONG_FRAME_BYTES: usize = 14;
const SHORT_FRAME_BYTES: usize = 7;
/// The demodulator can lock onto one transmission at two adjacent sample
/// offsets. A real retransmission is never this close (a frame lasts 120 us).
const DUPLICATE_WINDOW_S: f64 = 0.001;

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

/// A running receiver yielding de-duplicated frames cut to their real length.
pub struct FrameStream {
    rx: mpsc::Receiver<TimedMessage>,
    shutdown_tx: watch::Sender<bool>,
    handle: JoinHandle<Result<(), desperado::Error>>,
    live: bool,
    description: String,
    last: Option<(Vec<u8>, f64)>,
}

impl FrameStream {
    /// Open the source and start demodulating on a background task.
    pub async fn open(args: &SourceArgs) -> Result<Self> {
        let live = args.file.is_none();
        let mut source = open_source(args).await?;

        let (tx, rx) = mpsc::channel::<TimedMessage>(1024);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // `receiver` borrows `source` only while it runs, so after the select
        // the SDR can still be stopped.
        let handle = tokio::spawn(async move {
            let result = tokio::select! {
                r = iqread::receiver(tx, &mut source, 0, SAMPLE_RATE_HZ as f64, None) => r,
                _ = shutdown_rx.changed() => Ok(()),
            };
            if live {
                // Tear down USB transfers deterministically before exit.
                let _ = tokio::time::timeout(Duration::from_secs(2), source.stop()).await;
            }
            result
        });

        let description = match &args.file {
            Some(path) => format!("replaying {}", path.display()),
            None => format!(
                "{:.3} MHz at {:.1} MS/s, gain {}",
                CENTER_FREQ_HZ as f64 / 1e6,
                SAMPLE_RATE_HZ as f64 / 1e6,
                if args.agc {
                    "auto".to_string()
                } else {
                    format!("{:.1} dB", args.gain as f64 / 10.0)
                }
            ),
        };

        Ok(Self {
            rx,
            shutdown_tx,
            handle,
            live,
            description,
            last: None,
        })
    }

    /// Human readable description of the source, for status lines.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Next frame, or `None` once the source has ended. Cancel safe.
    pub async fn recv(&mut self) -> Option<TimedMessage> {
        loop {
            let mut msg = self.rx.recv().await?;
            msg.frame.truncate(frame_len(&msg.frame));
            if is_duplicate(&self.last, &msg) {
                continue;
            }
            self.last = Some((msg.frame.clone(), msg.timestamp));
            return Some(msg);
        }
    }

    /// Stop the receiver and report why it ended, if it failed.
    pub async fn close(self) -> Result<()> {
        drop(self.rx);
        let _ = self.shutdown_tx.send(true);
        match self.handle.await.context("receiver task panicked")? {
            Ok(()) => Ok(()),
            // A file source ends with a "stream terminated" error. That is EOF.
            Err(_) if !self.live => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context("receiver failed")),
        }
    }
}

async fn open_source(args: &SourceArgs) -> Result<IqAsyncSource> {
    match &args.file {
        Some(path) => IqAsyncSource::from_file(
            path,
            CENTER_FREQ_HZ,
            SAMPLE_RATE_HZ,
            FILE_CHUNK_SAMPLES,
            args.iq_format.into(),
        )
        .await
        .with_context(|| format!("cannot open IQ file {}", path.display())),
        None => {
            let gain = if args.agc { None } else { Some(args.gain) };
            IqAsyncSource::from_rtlsdr(args.device, CENTER_FREQ_HZ, SAMPLE_RATE_HZ, gain)
                .await
                .with_context(|| {
                    format!(
                        "cannot open RTL-SDR device {}. Is the dongle plugged in, \
                         the DVB kernel driver blacklisted and the udev rule installed? \
                         (`mise run setup-udev`)",
                        args.device
                    )
                })
        }
    }
}

/// The demodulator always yields 14 bytes. Short downlink formats (DF < 16)
/// only carry 7 meaningful bytes; the rest is padding that must be dropped
/// before decoding.
fn frame_len(frame: &[u8]) -> usize {
    match frame.first() {
        Some(b) if b >> 3 < 16 => SHORT_FRAME_BYTES.min(frame.len()),
        _ => LONG_FRAME_BYTES.min(frame.len()),
    }
}

/// True when `msg` is the same transmission as the previously yielded one.
fn is_duplicate(last: &Option<(Vec<u8>, f64)>, msg: &TimedMessage) -> bool {
    matches!(last, Some((frame, ts))
        if *frame == msg.frame && (msg.timestamp - ts).abs() < DUPLICATE_WINDOW_S)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rs1090::decode::Message;

    pub(crate) fn frame(hex_str: &str) -> Vec<u8> {
        hex::decode(hex_str).unwrap()
    }

    pub(crate) fn timed(hex_str: &str, timestamp: f64) -> TimedMessage {
        TimedMessage {
            timestamp,
            frame: frame(hex_str),
            message: None,
            metadata: vec![],
            decode_time: None,
        }
    }

    #[test]
    fn long_frame_keeps_14_bytes() {
        let f = frame("8da2c1bd587ba2adb31799cb802b");
        assert_eq!(frame_len(&f), 14);
    }

    #[test]
    fn short_frame_is_truncated_to_7_bytes() {
        // DF11 all-call reply padded to 14 bytes like the demodulator does.
        let f = frame("5d4ca4ab3b2f5c00000000000000");
        assert_eq!(frame_len(&f), 7);
    }

    #[test]
    fn padded_short_frame_decodes_after_truncation() {
        let mut f = frame("5d4ca4ab3b2f5c00000000000000");
        f.truncate(frame_len(&f));
        Message::try_from(f.as_slice()).expect("DF11 decodes after truncation");
    }

    #[test]
    fn same_frame_within_window_is_duplicate() {
        let hex = "8da2c1bd587ba2adb31799cb802b";
        let last = Some((frame(hex), 10.0));
        assert!(is_duplicate(&last, &timed(hex, 10.0002)));
        assert!(!is_duplicate(&last, &timed(hex, 10.5)));
        assert!(!is_duplicate(&last, &timed("5d4ca4ab3b2f5c", 10.0002)));
        assert!(!is_duplicate(&None, &timed("5d4ca4ab3b2f5c", 10.0)));
    }
}
