//! Shared frame source: demodulated Mode S frames from the RTL-SDR dongle,
//! an IQ file, or a recording made with `debug record`.
//!
//! Live pipeline: rs-rtl (pure Rust USB driver) -> desperado (IQ source) ->
//! rs1090 demodulator at 2.4 MS/s. A recording skips all of that and replays
//! the frames with their original timing, so every command can be used
//! without the dongle.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use desperado::{IqAsyncSource, IqFormat};
use rs1090::decode::{SensorMetadata, TimedMessage};
use rs1090::source::iqread;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::parse;

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
        let (tx, rx) = mpsc::channel::<TimedMessage>(1024);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let live = args.device_record_file.is_none() && args.file.is_none();
        let (handle, description) = match &args.device_record_file {
            Some(path) => start_replay(args, path, tx, shutdown_rx)?,
            None => start_receiver(args, live, tx, shutdown_rx).await?,
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

/// The background task that feeds the frames, and a description of where
/// they come from.
type Started = (JoinHandle<Result<(), desperado::Error>>, String);

/// Replay the recording at `path` until it ends or `shutdown` changes.
fn start_replay(
    args: &SourceArgs,
    path: &Path,
    tx: mpsc::Sender<TimedMessage>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Started> {
    let frames = read_recording(path)?;
    let description = format!(
        "replaying {} ({} frames, {:.0} s{}{})",
        path.display(),
        frames.len(),
        recording_span(&frames),
        if args.device_record_speed == 1.0 {
            String::new()
        } else {
            format!(", {}x speed", args.device_record_speed)
        },
        if args.device_record_loop {
            ", looping"
        } else {
            ""
        },
    );
    let (speed, looping) = (args.device_record_speed, args.device_record_loop);
    let handle = tokio::spawn(async move {
        tokio::select! {
            _ = replay(tx, frames, speed, looping) => {}
            _ = shutdown.changed() => {}
        }
        Ok(())
    });
    Ok((handle, description))
}

/// Demodulate the dongle, or an IQ file if `live` is false, until the source
/// ends or `shutdown` changes.
async fn start_receiver(
    args: &SourceArgs,
    live: bool,
    tx: mpsc::Sender<TimedMessage>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Started> {
    let mut source = open_source(args).await?;

    // `receiver` borrows `source` only while it runs, so after the select
    // the SDR can still be stopped.
    let handle = tokio::spawn(async move {
        let result = tokio::select! {
            r = iqread::receiver(tx, &mut source, 0, SAMPLE_RATE_HZ as f64, None) => r,
            _ = shutdown.changed() => Ok(()),
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
    Ok((handle, description))
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
                .map_err(|e| {
                    let connected = desperado::rtlsdr::list_devices().ok().map(|d| d.len());
                    anyhow::anyhow!(device_error(args.device, connected, &e.to_string()))
                })
        }
    }
}

/// Explain why the dongle could not be opened. `connected` is the number of
/// dongles on the bus, if they could be counted; it tells a missing dongle
/// from one that is there but cannot be used.
fn device_error(device: usize, connected: Option<usize>, cause: &str) -> String {
    const REPLAY_HINT: &str = "or replay a recording with `--device-record-file <PATH>`";
    match connected {
        Some(0) => {
            format!("no RTL-SDR dongle found\n  Plug the dongle in and try again, {REPLAY_HINT}.")
        }
        Some(count) if device >= count => format!(
            "RTL-SDR device {device} not found, {}\n  Pick one with `--device <INDEX>`, {REPLAY_HINT}.",
            if count == 1 {
                "only device 0 is connected".to_string()
            } else {
                format!("only devices 0 to {} are connected", count - 1)
            }
        ),
        _ => format!(
            "cannot open RTL-SDR device {device}: {cause}\n  Is the DVB kernel driver blacklisted, the udev rule installed \
             (`mise run setup-udev`) and no other program using the dongle?"
        ),
    }
}

/// One frame of a recording.
#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    /// Receive time in seconds, as recorded.
    timestamp: f64,
    frame: Vec<u8>,
    rssi: Option<f32>,
}

/// Parse one line written by `debug record`. Only the raw frame, its time
/// and signal level are used; the decoded fields in the line are for people
/// and other tools, and are decoded again from the frame on replay.
fn parse_recorded(line: &str) -> Result<Recorded> {
    let json: serde_json::Value = serde_json::from_str(line).context("not valid JSON")?;
    let timestamp = json
        .get("timestamp")
        .and_then(serde_json::Value::as_f64)
        .context("no numeric `timestamp`")?;
    let hex = json
        .get("frame")
        .and_then(serde_json::Value::as_str)
        .context("no `frame`")?;
    let frame = hex::decode(hex).context("`frame` is not hex")?;
    if frame.len() != SHORT_FRAME_BYTES && frame.len() != LONG_FRAME_BYTES {
        bail!("`frame` is {} bytes, expected 7 or 14", frame.len());
    }
    let rssi = json
        .pointer("/metadata/0/rssi")
        .and_then(serde_json::Value::as_f64)
        .map(|v| v as f32);
    Ok(Recorded {
        timestamp,
        frame,
        rssi,
    })
}

/// Read a whole recording. The file is checked completely before anything
/// is replayed, so a broken line is reported at start and not hours in.
fn read_recording(path: &Path) -> Result<Vec<Recorded>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open recording {}", path.display()))?;
    let mut frames = Vec::new();
    for (number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("cannot read {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        frames.push(
            parse_recorded(&line)
                .with_context(|| format!("{} line {}", path.display(), number + 1))?,
        );
    }
    if frames.is_empty() {
        bail!("recording {} contains no frames", path.display());
    }
    Ok(frames)
}

/// Seconds between the first and the last frame.
fn recording_span(frames: &[Recorded]) -> f64 {
    match (frames.first(), frames.last()) {
        (Some(first), Some(last)) => (last.timestamp - first.timestamp).max(0.0),
        _ => 0.0,
    }
}

/// Send `frames` with their recorded spacing, `speed` times faster.
///
/// Timestamps are passed on as recorded, so position decoding sees the same
/// time differences as it did live. When looping, each round is shifted
/// past the previous one to keep time moving forward.
async fn replay(tx: mpsc::Sender<TimedMessage>, frames: Vec<Recorded>, speed: f64, looping: bool) {
    let Some(first) = frames.first().map(|f| f.timestamp) else {
        return;
    };
    // One second of silence between two rounds.
    let round = recording_span(&frames) + 1.0;
    let mut shift = 0.0;
    loop {
        let started = tokio::time::Instant::now();
        for recorded in &frames {
            let due = (recorded.timestamp - first).max(0.0) / speed;
            tokio::time::sleep_until(started + Duration::from_secs_f64(due)).await;
            let timestamp = recorded.timestamp + shift;
            let message = TimedMessage {
                timestamp,
                frame: recorded.frame.clone(),
                message: None,
                metadata: vec![SensorMetadata {
                    system_timestamp: timestamp,
                    gnss_timestamp: None,
                    nanoseconds: None,
                    rssi: recorded.rssi,
                    serial: 0,
                    name: None,
                }],
                decode_time: None,
            };
            if tx.send(message).await.is_err() {
                return;
            }
        }
        if !looping {
            return;
        }
        tokio::time::sleep_until(started + Duration::from_secs_f64(round / speed)).await;
        shift += round;
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
    use crate::tests::{parse_args, try_parse_args};
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

    /// A source that replays `path`.
    fn replay_args(path: &Path, extra: &[&str]) -> SourceArgs {
        let path = path.to_str().unwrap();
        parse_args(&[&["--device-record-file", path], extra].concat())
    }

    /// A file under the system temp dir, removed when dropped.
    pub(crate) struct TempFile(pub PathBuf);

    impl TempFile {
        pub(crate) fn new(name: &str, content: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "adsb-flipdot-test-{name}-{}.jsonl",
                std::process::id()
            ));
            std::fs::write(&path, content).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    const LINE_LONG: &str = r#"{"timestamp":100.5,"frame":"8d4840d6202cc371c32ce0576098","df":"17","icao24":"4840d6","callsign":"KLM1023_","metadata":[{"system_timestamp":100.5,"rssi":-12.5,"serial":0}]}"#;
    const LINE_SHORT: &str = r#"{"timestamp":100.75,"frame":"5d4ca4ab3b2f5c","df":"11"}"#;

    #[test]
    fn recorded_line_gives_frame_time_and_signal() {
        let recorded = parse_recorded(LINE_LONG).unwrap();
        assert_eq!(recorded.timestamp, 100.5);
        assert_eq!(recorded.frame, frame("8d4840d6202cc371c32ce0576098"));
        assert_eq!(recorded.rssi, Some(-12.5));

        let short = parse_recorded(LINE_SHORT).unwrap();
        assert_eq!(short.frame.len(), 7);
        assert_eq!(short.rssi, None, "metadata is optional");
    }

    #[test]
    fn broken_recorded_lines_are_rejected() {
        for (line, problem) in [
            ("not json", "not valid JSON"),
            (r#"{"frame":"5d4ca4ab3b2f5c"}"#, "no numeric `timestamp`"),
            (
                r#"{"timestamp":"1","frame":"5d4ca4ab3b2f5c"}"#,
                "no numeric `timestamp`",
            ),
            (r#"{"timestamp":1.0}"#, "no `frame`"),
            (r#"{"timestamp":1.0,"frame":"zz"}"#, "`frame` is not hex"),
            (r#"{"timestamp":1.0,"frame":"5d4c"}"#, "`frame` is 2 bytes"),
        ] {
            let error = format!("{:#}", parse_recorded(line).unwrap_err());
            assert!(error.contains(problem), "{line}: {error}");
        }
    }

    #[test]
    fn recording_file_is_read_and_checked_completely() {
        let good = TempFile::new("read-good", &format!("{LINE_LONG}\n\n{LINE_SHORT}\n"));
        let frames = read_recording(&good.0).unwrap();
        assert_eq!(frames.len(), 2, "blank lines are skipped");
        assert_eq!(recording_span(&frames), 0.25);

        let bad = TempFile::new("read-bad", &format!("{LINE_LONG}\n{{\"oops\":1}}\n"));
        let error = format!("{:#}", read_recording(&bad.0).unwrap_err());
        assert!(error.contains("line 2"), "{error}");

        let empty = TempFile::new("read-empty", "\n");
        let error = format!("{:#}", read_recording(&empty.0).unwrap_err());
        assert!(error.contains("contains no frames"), "{error}");

        let missing = std::env::temp_dir().join("adsb-flipdot-test-does-not-exist.jsonl");
        let error = format!("{:#}", read_recording(&missing).unwrap_err());
        assert!(error.contains("cannot open recording"), "{error}");
    }

    #[tokio::test]
    async fn recording_replays_all_frames_then_ends() {
        let file = TempFile::new("replay", &format!("{LINE_LONG}\n{LINE_SHORT}\n"));
        let args = replay_args(&file.0, &["--device-record-speed", "100"]);
        let mut stream = FrameStream::open(&args).await.unwrap();
        assert!(
            stream.description().contains("2 frames"),
            "{}",
            stream.description()
        );
        assert!(
            stream.description().contains("100x speed"),
            "{}",
            stream.description()
        );

        let first = stream.recv().await.unwrap();
        assert_eq!(first.timestamp, 100.5, "recorded time is kept");
        assert_eq!(first.frame, frame("8d4840d6202cc371c32ce0576098"));
        assert_eq!(first.metadata[0].rssi, Some(-12.5));
        let second = stream.recv().await.unwrap();
        assert_eq!(second.frame, frame("5d4ca4ab3b2f5c"));
        assert!(
            stream.recv().await.is_none(),
            "source ends with the recording"
        );
        stream.close().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn recording_replays_with_the_recorded_spacing() {
        let lines: String = [0.0, 2.0, 7.0]
            .iter()
            .map(|t| format!(r#"{{"timestamp":{},"frame":"5d4ca4ab3b2f5c"}}"#, 50.0 + t) + "\n")
            .collect();
        let file = TempFile::new("spacing", &lines);

        for (speed, expected) in [("1", [0.0, 2.0, 7.0]), ("2", [0.0, 1.0, 3.5])] {
            let args = replay_args(&file.0, &["--device-record-speed", speed]);
            let mut stream = FrameStream::open(&args).await.unwrap();
            let started = tokio::time::Instant::now();
            for delay in expected {
                stream.recv().await.unwrap();
                let elapsed = started.elapsed().as_secs_f64();
                assert!(
                    (elapsed - delay).abs() < 0.01,
                    "speed {speed}: {elapsed} vs {delay}"
                );
            }
            assert!(stream.recv().await.is_none());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn looping_recording_keeps_time_moving_forward() {
        let lines = format!(
            "{}\n{}\n",
            r#"{"timestamp":10.0,"frame":"5d4ca4ab3b2f5c"}"#,
            r#"{"timestamp":14.0,"frame":"5d4ca4ab3b2f5c"}"#
        );
        let file = TempFile::new("loop", &lines);
        let args = replay_args(&file.0, &["--device-record-loop"]);
        let mut stream = FrameStream::open(&args).await.unwrap();
        assert!(stream.description().contains("looping"));

        let started = tokio::time::Instant::now();
        let mut seen = Vec::new();
        for _ in 0..5 {
            let msg = stream.recv().await.unwrap();
            seen.push((msg.timestamp, started.elapsed().as_secs_f64().round()));
        }
        // 4 s of recording plus 1 s pause: each round is 5 s later, both in
        // the timestamps and on the clock.
        assert_eq!(
            seen,
            [
                (10.0, 0.0),
                (14.0, 4.0),
                (15.0, 5.0),
                (19.0, 9.0),
                (20.0, 10.0)
            ]
        );
        stream.close().await.unwrap();
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

    #[test]
    fn device_error_tells_missing_dongle_from_unusable_one() {
        let none = device_error(0, Some(0), "RTL-SDR device not found");
        assert!(none.starts_with("no RTL-SDR dongle found\n"), "{none}");
        assert!(none.contains("--device-record-file"), "{none}");

        let index = device_error(1, Some(1), "RTL-SDR device not found");
        assert!(index.contains("device 1 not found"), "{index}");
        assert!(index.contains("only device 0 is connected"), "{index}");
        let index = device_error(5, Some(3), "RTL-SDR device not found");
        assert!(index.contains("only devices 0 to 2"), "{index}");

        for connected in [Some(1), None] {
            let busy = device_error(0, connected, "failed to claim USB interface");
            assert!(busy.contains("cannot open RTL-SDR device 0"), "{busy}");
            assert!(busy.contains("failed to claim USB interface"), "{busy}");
            assert!(busy.contains("setup-udev"), "{busy}");
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
