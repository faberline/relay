//! The shared output helper of the render verbs.

use anyhow::Result;

/// Write `body` to `out` (a file, or `default_file` inside a directory) or
/// print it to stdout.
pub(crate) fn write_or_print(
    out: Option<&std::path::Path>,
    default_file: &str,
    body: &str,
) -> Result<()> {
    let written = cli_std::artifact::write_or_print(out, default_file, body)?;
    if written.is_some() {
        println!("next: done");
    }
    Ok(())
}
