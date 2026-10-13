//! `debug record`: save every received frame to a JSON lines file.
//!
//! Each line is what `debug dump --format json` prints. The file can be
//! replayed by any command with `--device-record-file`, which makes it
//! possible to work on the display without the dongle or without traffic.

use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::dump::to_json;
use crate::source::{FrameStream, SourceArgs};

#[derive(clap::Args, Debug)]
pub struct RecordArgs {
    /// File to write, one JSON object per line.
    #[arg(value_name = "FILE")]
    pub output: PathBuf,

    /// Replace the file if it already exists.
    #[arg(long, conflicts_with = "append")]
    pub overwrite: bool,

    /// Add to the file if it already exists.
    #[arg(long)]
    pub append: bool,

    /// Stop after this many seconds. Without it, record until Ctrl-C.
    #[arg(long, value_name = "SECONDS")]
    pub duration: Option<u64>,

    #[command(flatten)]
    pub source: SourceArgs,
}

/// Open the output file. An existing file is only touched when asked to.
fn create(path: &Path, overwrite: bool, append: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    if append {
        options.append(true).create(true);
    } else if overwrite {
        options.write(true).create(true).truncate(true);
    } else {
        // Fails if the file exists, without a gap between check and create.
        options.write(true).create_new(true);
    }
    options.open(path).with_context(|| {
        if path.exists() && !overwrite && !append {
            format!(
                "{} already exists. Use --overwrite to replace it or --append to add to it",
                path.display()
            )
        } else {
            format!("cannot write {}", path.display())
        }
    })
}

pub async fn run(args: RecordArgs) -> Result<()> {
    // Create the file first: a bad path should fail before the dongle opens.
    let file = create(&args.output, args.overwrite, args.append)?;
    // One line per frame, written out at once, so nothing is lost when the
    // process is killed.
    let mut writer = LineWriter::new(file);

    let mut stream = FrameStream::open(&args.source).await?;
    eprintln!("adsb-flipdot: {}", stream.description());
    eprintln!("adsb-flipdot: recording to {}", args.output.display());

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let deadline = async {
        match args.duration {
            Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);

    let progress = std::io::stderr().is_terminal();
    let started = Instant::now();
    let mut last_progress = Instant::now();
    let mut count: u64 = 0;
    let mut failure = None;
    loop {
        tokio::select! {
            _ = &mut ctrl_c => break,
            _ = &mut deadline => break,
            msg = stream.recv() => match msg {
                Some(msg) => {
                    let written = to_json(msg)
                        .context("cannot encode frame")
                        .and_then(|json| writeln!(writer, "{json}").context("cannot write"));
                    if let Err(e) = written {
                        failure = Some(e.context(format!("recording to {} failed", args.output.display())));
                        break;
                    }
                    count += 1;
                    if progress && last_progress.elapsed() >= Duration::from_secs(1) {
                        last_progress = Instant::now();
                        eprint!("\r{count} frames, {} s ", started.elapsed().as_secs());
                    }
                }
                None => break,
            },
        }
    }
    if progress {
        eprint!("\r\x1b[K");
    }

    let closed = stream.close().await;
    writer.flush().context("cannot finish the recording")?;
    eprintln!(
        "adsb-flipdot: {count} frames in {} s written to {}",
        started.elapsed().as_secs(),
        args.output.display()
    );
    match failure {
        Some(e) => Err(e),
        None => closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::tests::{TempFile, timed};
    use crate::tests::parse_args;
    use std::io::Read;

    fn read(path: &Path) -> String {
        let mut text = String::new();
        File::open(path).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn existing_file_is_not_touched_by_default() {
        let file = TempFile::new("record-exists", "precious\n");
        let error = format!("{:#}", create(&file.0, false, false).unwrap_err());
        assert!(error.contains("already exists"), "{error}");
        assert!(error.contains("--overwrite"), "{error}");
        assert_eq!(read(&file.0), "precious\n");
    }

    #[test]
    fn overwrite_replaces_and_append_adds() {
        let file = TempFile::new("record-modes", "old\n");
        writeln!(create(&file.0, false, true).unwrap(), "more").unwrap();
        assert_eq!(read(&file.0), "old\nmore\n");
        writeln!(create(&file.0, true, false).unwrap(), "new").unwrap();
        assert_eq!(read(&file.0), "new\n");
    }

    #[test]
    fn new_file_is_created() {
        let file = TempFile::new("record-new", "");
        std::fs::remove_file(&file.0).unwrap();
        writeln!(create(&file.0, false, false).unwrap(), "x").unwrap();
        assert_eq!(read(&file.0), "x\n");

        let missing_dir = file.0.join("no/such/dir.jsonl");
        let error = format!("{:#}", create(&missing_dir, false, false).unwrap_err());
        assert!(error.contains("cannot write"), "{error}");
    }

    #[test]
    fn json_line_has_frame_time_and_decoded_fields() {
        let json = to_json(timed("8d4840d6202cc371c32ce0576098", 100.5)).unwrap();
        assert!(!json.contains('\n'), "one line");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["timestamp"], 100.5);
        assert_eq!(value["frame"], "8d4840d6202cc371c32ce0576098");
        assert_eq!(value["icao24"], "4840d6");
        assert_eq!(value["callsign"], "KLM1023");
    }

    /// Record a replayed recording and compare: what comes out of a
    /// recording is exactly what went in.
    #[tokio::test]
    async fn recording_a_replay_reproduces_the_frames() {
        use crate::live::tests::{IDENT, POS_EVEN, POS_ODD, VELOCITY};

        let original: String = [IDENT, VELOCITY, POS_ODD, POS_EVEN, "5d4ca4ab3b2f5c"]
            .iter()
            .enumerate()
            .map(|(i, hex)| to_json(timed(hex, 1000.0 + i as f64 * 0.01)).unwrap() + "\n")
            .collect();
        let input = TempFile::new("roundtrip-in", &original);
        let output = TempFile::new("roundtrip-out", "");

        let args: RecordArgs = parse_args(&[
            output.0.to_str().unwrap(),
            "--overwrite",
            "--device-record-file",
            input.0.to_str().unwrap(),
            "--device-record-speed",
            "50",
        ]);
        run(args).await.unwrap();

        let recorded = read(&output.0);
        assert_eq!(recorded.lines().count(), 5);
        // Replay adds receiver metadata; frame, time and decode are equal.
        for (a, b) in original.lines().zip(recorded.lines()) {
            let mut a: serde_json::Value = serde_json::from_str(a).unwrap();
            let mut b: serde_json::Value = serde_json::from_str(b).unwrap();
            a.as_object_mut().unwrap().remove("metadata");
            b.as_object_mut().unwrap().remove("metadata");
            assert_eq!(a, b);
        }
    }
}
