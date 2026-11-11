//! Waits for a child process while honouring a cancel flag. Used by every adapter that runs an external program.

use super::contract::{Cancel, ProviderError};
use std::process::Child;
use std::time::Duration;

/// Waits for `child` to exit. If `cancel` is set first, the child is killed and Cancelled is returned.
pub fn wait_cancellable(child: &mut Child, cancel: &Cancel, what: &str) -> Result<(), ProviderError> {
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(ProviderError::Other(format!("{what} exited with {status}"))),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(ProviderError::Other(format!("lost the {what} process: {e}"))),
        }
    }
}
