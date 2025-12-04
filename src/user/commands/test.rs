//! Test command - run kernel tests.

use crate::TinyResult;
use crate::user::{Command, CommandContext};

/// Test command instance.
pub static TEST: TestCommand = TestCommand;

/// Test command implementation.
pub struct TestCommand;

impl Command for TestCommand {
    fn name(&self) -> &'static str {
        "test"
    }

    fn description(&self) -> &'static str {
        "Run kernel tests"
    }

    fn usage(&self) -> &'static str {
        "Usage: test\r\n\
         \r\n\
         Runs the kernel test suite (rstiny_tests) in a separate thread.\r\n\
         Waits for completion before returning."
    }

    fn category(&self) -> &'static str {
        "system"
    }

    fn execute(&self, _ctx: &CommandContext) -> TinyResult<()> {
        println!("Starting kernel tests...");

        // Spawn a new thread to run tests
        let handle = crate::task::thread::spawn("test_runner", || {
            crate::tests::rstiny_tests();
        });

        // Wait for the test thread to complete
        match handle.join() {
            Ok(_) => {
                println!("Kernel tests completed.");
                Ok(())
            }
            Err(e) => {
                println!("Test thread join failed: {:?}", e);
                Err(e)
            }
        }
    }
}
