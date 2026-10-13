//! `debug dump`: print every received frame until Ctrl-C or end of file.

use anyhow::Result;
use clap::ValueEnum;
use rs1090::decode::{Message, TimedMessage};

use crate::commands::shutdown::Shutdown;
use crate::source::stream::{FrameStream, SourceArgs};

#[derive(clap::Args, Debug)]
pub struct DumpArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One `*<hex>;` line per frame, as dump1090 emits on port 30002.
    Raw,
    /// Human readable decode of every frame.
    Text,
    /// One JSON object per line with frame, timestamp, RSSI and decode.
    Json,
}

pub async fn run(args: DumpArgs) -> Result<()> {
    let mut shutdown = Shutdown::listen()?;
    let mut stream = FrameStream::open(&args.source).await?;
    eprintln!("adsb-flipdot: {}", stream.description());

    let mut count: u64 = 0;
    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                eprintln!("adsb-flipdot: stopping");
                break;
            }
            msg = stream.recv() => match msg {
                Some(msg) => {
                    count += 1;
                    print_message(msg, args.format);
                }
                None => break,
            },
        }
    }

    let result = stream.close().await;
    eprintln!("adsb-flipdot: {count} frames received");
    result
}

/// One frame as a single line of JSON: time, raw frame, signal level and
/// everything that could be decoded from it.
pub fn to_json(mut msg: TimedMessage) -> serde_json::Result<String> {
    msg.message = Message::try_from(msg.frame.as_slice()).ok();
    serde_json::to_string(&msg)
}

fn print_message(msg: TimedMessage, format: Format) {
    let hex = hex::encode(&msg.frame);

    match format {
        Format::Raw => println!("*{hex};"),
        Format::Text => {
            let rssi = msg
                .metadata
                .first()
                .and_then(|m| m.rssi)
                .map(|r| format!(" rssi {r:.1} dB"))
                .unwrap_or_default();
            println!("{:.3} {hex}{rssi}", msg.timestamp);
            match Message::try_from(msg.frame.as_slice()) {
                // Some Display impls in rs1090 end with a newline, some with
                // trailing spaces. Normalise so every block ends cleanly.
                Ok(decoded) => println!("{}", decoded.to_string().trim_end()),
                Err(e) => println!("  (undecodable: {e})"),
            }
        }
        Format::Json => match to_json(msg) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("adsb-flipdot: json error: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::stream::tests::frame;

    #[test]
    fn long_frame_decodes() {
        let f = frame("8da2c1bd587ba2adb31799cb802b");
        let msg = Message::try_from(f.as_slice()).expect("decodes");
        let text = msg.to_string();
        assert!(text.contains("a2c1bd"), "ICAO address missing in:\n{text}");
    }
}
