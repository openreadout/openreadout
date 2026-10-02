//! `--help` layout: a command's own flags first (under "Options"), then the shared batch,
//! batch-table and global flags under their own headings.
//!
//! clap's `next_help_heading` on a flattened struct stays in force for every argument declared
//! after it, so a command's own flags that follow `#[command(flatten)] batch` would be listed
//! under the batch heading. [`EndHeading`], flattened as the last field of such a struct,
//! returns the following arguments to the default section. Flag names and parsing are not
//! affected.

use clap::{ArgMatches, Args, Command, FromArgMatches};

/// Ends a flattened group of options in `--help` (holds no value).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndHeading;

impl FromArgMatches for EndHeading {
    fn from_arg_matches(_: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(EndHeading)
    }
    fn update_from_arg_matches(&mut self, _: &ArgMatches) -> Result<(), clap::Error> {
        Ok(())
    }
}

impl Args for EndHeading {
    fn augment_args(cmd: Command) -> Command {
        cmd.next_help_heading(None::<&str>)
    }
    fn augment_args_for_update(cmd: Command) -> Command {
        cmd.next_help_heading(None::<&str>)
    }
}
