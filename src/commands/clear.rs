//! `debug clear`: turn every dot of the panels to its dark side, for a
//! known start or a blank display.

use anyhow::{Context, Result};

use crate::cli::ClearArgs;
use crate::commands::shutdown::Shutdown;

pub fn run(args: ClearArgs) -> Result<()> {
    // Ctrl-C no longer ends the program by itself, which could happen in the
    // middle of a pulse. Clearing takes a fraction of a second anyway.
    let _signals = Shutdown::listen()?;
    let mut panel = args.panels.open().context("cannot drive the panels")?;
    panel.clear()?;
    let (width, height) = args.panels.display_size();
    eprintln!("adsb-flipdot: {} dots cleared", width * height);
    Ok(())
}
