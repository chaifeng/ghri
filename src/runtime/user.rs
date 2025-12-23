//! User interaction operations (confirmation prompts).

use anyhow::Result;

use super::RealRuntime;

use std::io::{self, BufRead, Write};

/// Core, testable implementation that reads from any BufRead and writes to any Write.
/// This is intentionally free-standing so tests can exercise it without needing a RealRuntime.
pub(crate) fn confirm_with_io<R: BufRead, W: Write>(prompt: &str, input: &mut R, output: &mut W) -> Result<bool> {
    write!(output, "{} [y/N] ", prompt)?;
    output.flush()?;

    let mut line = String::new();
    input.read_line(&mut line)?;

    let response = line.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

impl RealRuntime {
    pub(crate) fn confirm_impl(&self, prompt: &str) -> Result<bool> {
        // Wire the generic implementation to real stdin/stdout.
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut stdin_lock = stdin.lock();
        confirm_with_io(prompt, &mut stdin_lock, &mut stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::confirm_with_io;
    use anyhow::Result;
    use std::io::Cursor;

    #[test]
    fn confirms_yes_and_short_y() -> Result<()> {
        let cases = vec!["y\n", "Y\n", "yes\n", " YES \n", "  y  \n"];
        for case in cases {
            let mut input = Cursor::new(case.as_bytes());
            let mut output = Vec::new();
            let ok = confirm_with_io("Proceed?", &mut input, &mut output)?;
            assert!(ok, "expected '{}' to be accepted as yes", case);
            let out = String::from_utf8(output)?;
            assert!(out.contains("Proceed? [y/N]"));
        }
        Ok(())
    }

    #[test]
    fn rejects_no_and_empty() -> Result<()> {
        let cases = vec!["n\n", "no\n", "\n", "  \n", "other\n"];
        for case in cases {
            let mut input = Cursor::new(case.as_bytes());
            let mut output = Vec::new();
            let ok = confirm_with_io("Delete?", &mut input, &mut output)?;
            assert!(!ok, "expected '{}' to be rejected as no", case);
            let out = String::from_utf8(output)?;
            assert!(out.contains("Delete? [y/N]"));
        }
        Ok(())
    }

    #[test]
    fn prompt_is_written_before_reading() -> Result<()> {
        // Use input that would block if we actually waited; since we provide all data,
        // we just assert the prompt is emitted correctly.
        let mut input = Cursor::new(b"n\n");
        let mut output = Vec::new();
        let _ = confirm_with_io("Are you sure", &mut input, &mut output)?;
        let out = String::from_utf8(output)?;
        assert_eq!(out, "Are you sure [y/N] ");
        Ok(())
    }
}
