use clap::{AppSettings, Parser};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(
    name = "lintje",
    version,
    verbatim_doc_comment,
    setting(AppSettings::DeriveDisplayOrder)
)]
/**
Lint Git commits and branch name.

## Usage examples

    lintje
      Validate the latest commit.

    lintje HEAD
      Validate the latest commit.

    lintje 3a561ef766c2acfe5da478697d91758110b8b24c
      Validate a single specific commit.

    lintje HEAD~5..HEAD
      Validate the last 5 commits.

    lintje main..develop
      Validate the difference between the main and develop branch.

    lintje --hook-message-file=.git/COMMIT_EDITMSG
      Lints the given commit message file from the commit-msg hook.

    lintje --no-branch
      Disable branch name validation.

    lintje --color
      Enable color output.
*/
pub struct Lint {
    /// Disable branch validation
    #[clap(long = "no-branch", parse(from_flag = std::ops::Not::not))]
    pub branch_validation: bool,

    /// Disable hints
    #[clap(long = "no-hints", parse(from_flag = std::ops::Not::not))]
    pub hints: bool,

    /// Enable color output
    #[clap(long = "color")]
    pub color: bool,

    /// Disable color output
    #[clap(long = "no-color")]
    pub no_color: bool,

    /// Lint the contents the Git hook commit-msg commit message file.
    #[clap(long, parse(from_os_str))]
    pub hook_message_file: Option<PathBuf>,

    /// Prints debug information
    #[clap(long)]
    pub debug: bool,

    /// Lint commits by Git commit SHA or by a range of commits. When no <commit> is specified, it
    /// defaults to linting the latest commit.
    #[clap(name = "commit (range)")]
    pub selection: Option<String>,
}

impl Lint {
    pub fn color(&self) -> bool {
        if self.no_color {
            return false;
        }
        if self.color {
            return true;
        }
        false // By default color is turned off
    }
}

#[derive(Debug)]
pub struct Options {
    pub debug: bool,
    pub color: bool,
    pub hints: bool,
}

#[cfg(test)]
mod tests {
    use super::Lint;
    use clap::Parser;

    #[test]
    fn test_color_flags() {
        // Both color flags set, but --no-color is leading
        assert!(!Lint::parse_from(["lintje", "--color", "--no-color"]).color());

        // Only --color is set
        assert!(Lint::parse_from(["lintje", "--color"]).color());

        // Only --no-color is set
        assert!(!Lint::parse_from(["lintje", "--no-color"]).color());

        // No flags are set
        assert!(!Lint::parse_from(["lintje"]).color());
    }
}
