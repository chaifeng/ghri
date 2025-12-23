//! User interaction operations (confirmation prompts).

use anyhow::Result;

use super::RealRuntime;

impl RealRuntime {
    pub(crate) fn confirm_impl(&self, prompt: &str) -> Result<bool> {
        use std::io::{self, Write};
        print!("{} [y/N] ", prompt);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let response = input.trim().to_lowercase();
        Ok(response == "y" || response == "yes")
    }
}
