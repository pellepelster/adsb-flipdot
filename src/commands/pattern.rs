//! `debug pattern`: show test patterns one after another, in the terminal
//! and on the panels, to see that every dot flips both ways and how the
//! panels are mounted.

use std::time::Duration;

use anyhow::{Context, Result};

use crate::cli::PatternArgs;
use crate::commands::shutdown::Shutdown;
use crate::domain::pattern::Pattern;
use crate::output::Note;
use crate::output::terminal::Stacking;

pub async fn run(args: PatternArgs) -> Result<()> {
    let patterns = if args.patterns.is_empty() {
        Pattern::ALL.to_vec()
    } else {
        args.patterns.clone()
    };
    let hold = Duration::from_secs(args.hold);
    let (width, height) = args.panels.display_size();
    let mut shutdown = Shutdown::listen()?;
    let mut display = args
        .panels
        .open_display(Stacking::InPlace, !args.dry_run)
        .context("cannot drive the panels (--dry-run only shows the patterns in the terminal)")?;

    for (count, pattern) in patterns.iter().cycle().enumerate() {
        if args.once && count == patterns.len() {
            break;
        }
        let caption = format!(
            "{} ({}/{})",
            pattern.name(),
            count % patterns.len() + 1,
            patterns.len()
        );
        let note = Note {
            caption: Some(&caption),
            new_text: true,
        };
        display.show(&pattern.frame(width, height), note)?;
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = tokio::time::sleep(hold) => {}
        }
    }
    Ok(())
}
