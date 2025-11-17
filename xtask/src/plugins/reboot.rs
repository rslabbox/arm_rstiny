use crate::utils::TaskResult;
use std::thread;
use std::time::Duration;

pub struct RebootTask;

impl RebootTask {
    pub fn new() -> TaskResult<Self> {
        Ok(RebootTask)
    }

    pub fn execute(&self) -> TaskResult<()> {
        info!("==> Rebooting device...");

        // Send OFF request
        info!("    Sending OFF request to http://192.168.1.24:8080/off");
        match ureq::get("http://192.168.1.24:8080/off").call() {
            Ok(_) => info!("    OFF request successful"),
            Err(e) => {
                log::error!("    OFF request failed: {}", e);
                return Err(crate::utils::TaskError::Network(format!("OFF request failed: {}", e)));
            }
        }

        // Wait 0.3 seconds
        info!("    Waiting 0.3 seconds...");
        thread::sleep(Duration::from_millis(300));

        // Send ON request
        info!("    Sending ON request to http://192.168.1.24:8080/on");
        match ureq::get("http://192.168.1.24:8080/on").call() {
            Ok(_) => info!("    ON request successful"),
            Err(e) => {
                log::error!("    ON request failed: {}", e);
                return Err(crate::utils::TaskError::Network(format!("ON request failed: {}", e)));
            }
        }

        info!("    Reboot sequence completed!");
        Ok(())
    }
}