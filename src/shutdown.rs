//! Being asked to end: Ctrl-C, `kill`, a closed terminal.

use anyhow::{Context, Result};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

/// The signals that ask the program to end.
const SIGNALS: [SignalKind; 3] = [
    SignalKind::interrupt(),
    SignalKind::terminate(),
    SignalKind::hangup(),
];

/// Waits for a signal that asks the program to end.
///
/// From the moment it exists those signals no longer end the program by
/// themselves. That could leave the terminal in full-screen mode, or happen
/// in the middle of a pulse and leave a coil of the panel energised. The
/// command ends its loop instead and cleans up.
pub struct Shutdown(mpsc::Receiver<()>);

impl Shutdown {
    /// Start listening. Needs a running tokio runtime.
    pub fn listen() -> Result<Self> {
        Self::listen_for(&SIGNALS)
    }

    fn listen_for(kinds: &[SignalKind]) -> Result<Self> {
        let (tx, rx) = mpsc::channel(1);
        for kind in kinds {
            let mut signal = signal(*kind).context("cannot listen for signals")?;
            let tx = tx.clone();
            tokio::spawn(async move {
                if signal.recv().await.is_some() {
                    // Full: another signal asked already.
                    let _ = tx.try_send(());
                }
            });
        }
        Ok(Self(rx))
    }

    /// Wait until the program is asked to end. Cancel safe.
    pub async fn recv(&mut self) {
        self.0.recv().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn kill_and_a_closed_terminal_count_like_ctrl_c() {
        for kind in [
            SignalKind::interrupt(),
            SignalKind::terminate(),
            SignalKind::hangup(),
        ] {
            assert!(SIGNALS.contains(&kind), "{kind:?}");
        }
    }

    #[tokio::test]
    async fn a_signal_ends_the_wait() {
        // A signal no command listens for, so that tests running in the
        // same process are left alone.
        let mut shutdown = Shutdown::listen_for(&[SignalKind::user_defined1()]).unwrap();
        let waiting = tokio::time::timeout(Duration::from_millis(50), shutdown.recv()).await;
        assert!(waiting.is_err(), "nothing asked yet");

        // SAFETY: sends a signal to this process, which handles it by now.
        unsafe { libc::raise(libc::SIGUSR1) };
        tokio::time::timeout(Duration::from_secs(5), shutdown.recv())
            .await
            .expect("the signal ends the wait");
    }
}
