#![deny(unused_crate_dependencies)]
#![deny(unused_extern_crates)]
#![deny(unused_import_braces)]
#![deny(non_ascii_idents)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use log::LevelFilter;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use structopt::StructOpt;

mod branch;
mod command;
mod commit;
mod formatter;
mod git;
mod issue;
mod logger;
mod rule;
mod utils;

use branch::Branch;
use command::run_command;
use commit::Commit;
use formatter::{formatted_branch_issue, formatted_commit_issue};
use git::{fetch_and_parse_branch, fetch_and_parse_commits, parse_commit_hook_format};
use logger::Logger;
use termcolor::{ColorChoice, StandardStream, WriteColor};

#[derive(StructOpt, Debug)]
#[structopt(name = "lintje", verbatim_doc_comment)]
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
struct Lint {
    /// Prints debug information
    #[structopt(long)]
    debug: bool,

    /// Lint the contents the Git hook commit-msg commit message file.
    #[structopt(long, parse(from_os_str))]
    hook_message_file: Option<PathBuf>,

    /// Disable branch validation
    #[structopt(long = "no-branch")]
    no_branch_validation: bool,

    /// Enable color output
    #[structopt(long = "color")]
    color: bool,

    /// Disable color output
    #[structopt(long = "no-color")]
    no_color: bool,

    /// Lint commits by Git commit SHA or by a range of commits. When no <commit> is specified, it
    /// defaults to linting the latest commit.
    #[structopt(name = "commit (range)")]
    selection: Option<String>,
}

pub struct Options {
    debug: bool,
    color: bool,
}

fn main() {
    let args = Lint::from_args();
    init_logger(args.debug);
    let commit_result = match args.hook_message_file {
        Some(hook_message_file) => lint_commit_hook(&hook_message_file),
        None => lint_commit(args.selection),
    };
    let branch_result = if args.no_branch_validation {
        None
    } else {
        Some(lint_branch())
    };
    let options = Options {
        debug: args.debug,
        color: with_color(args.color, args.no_color),
    };
    handle_result(print_lint_result(commit_result, branch_result, options));
}

fn with_color(color: bool, no_color: bool) -> bool {
    if no_color {
        return false;
    }
    if color {
        return true;
    }
    false // By default color is turned off
}

fn lint_branch() -> Result<Branch, String> {
    fetch_and_parse_branch()
}

fn lint_commit(selection: Option<String>) -> Result<Vec<Commit>, String> {
    fetch_and_parse_commits(selection)
}

fn lint_commit_hook(filename: &Path) -> Result<Vec<Commit>, String> {
    let commits = match File::open(filename) {
        Ok(mut file) => {
            let mut contents = String::new();
            match file.read_to_string(&mut contents) {
                Ok(_) => {}
                Err(e) => {
                    return Err(format!(
                        "Unable to read commit message file contents: {}\n{}",
                        filename.to_str().unwrap(),
                        e
                    ));
                }
            };

            // Run the diff command to fetch the current staged changes and determine if the commit is
            // empty or not. The contents of the commit message file is too unreliable as it depends on
            // user config and how the user called the `git commit` command.
            let mut has_changes = true;
            match run_command("git", &["diff", "--cached", "--shortstat"]) {
                Ok(stdout) => {
                    if stdout.is_empty() {
                        has_changes = false;
                    }
                }
                Err(e) => error!("Unable to determine commit changes.\nError: {}", e.message),
            }
            let commit = parse_commit_hook_format(
                &contents,
                git::cleanup_mode(),
                git::comment_char(),
                has_changes,
            );
            vec![commit]
        }
        Err(e) => {
            return Err(format!(
                "Unable to open commit message file: {}\n{}",
                filename.to_str().unwrap(),
                e
            ));
        }
    };
    Ok(commits)
}

fn handle_result(result: io::Result<()>) {
    match result {
        Ok(()) => {}
        Err(error) => error!("Unexpected error encountered: {}", error),
    }
}

fn print_lint_result(
    commit_result: Result<Vec<Commit>, String>,
    branch_result: Option<Result<Branch, String>>,
    options: Options,
) -> io::Result<()> {
    let mut out = buffer_writer(options.color);
    let mut issue_count = 0;
    let mut commit_count = 0;
    let mut ignored_commit_count = 0;
    let mut branch_message = "";

    if let Ok(ref commits) = commit_result {
        debug!("Commits: {:?}", commits);
        for commit in commits {
            if commit.ignored {
                ignored_commit_count += 1;
                continue;
            }
            commit_count += 1;
            if !commit.is_valid() {
                for issue in &commit.issues {
                    issue_count += 1;
                    formatted_commit_issue(&mut out, commit, issue)?;
                }
            }
        }
    }
    let mut branch_error = None;
    if let Some(result) = branch_result {
        match result {
            Ok(ref branch) => {
                debug!("Branch: {:?}", branch);
                branch_message = " and branch";
                if !branch.is_valid() {
                    for issue in &branch.issues {
                        issue_count += 1;
                        formatted_branch_issue(&mut out, branch, issue)?;
                    }
                }
            }
            Err(error) => branch_error = Some(error),
        }
    }

    let commit_plural = if commit_count != 1 { "s" } else { "" };
    write!(
        out,
        "{} commit{}{} inspected, ",
        commit_count, commit_plural, branch_message
    )?;
    print_issue_count(&mut out, issue_count)?;
    if ignored_commit_count > 0 || options.debug {
        let ignored_plural = if ignored_commit_count != 1 { "s" } else { "" };
        write!(
            out,
            " ({} commit{} ignored)",
            ignored_commit_count, ignored_plural
        )?;
    }
    writeln!(out)?;
    let mut has_error = false;
    if let Err(error) = commit_result {
        has_error = true;
        error!("An error occurred validating commits: {}", error.trim());
    }
    if let Some(error) = branch_error {
        has_error = true;
        error!("An error occurred validating the branch: {}", error.trim());
    }
    if has_error {
        std::process::exit(2)
    }
    if issue_count > 0 {
        std::process::exit(1)
    }
    Ok(())
}

fn print_issue_count(out: &mut impl WriteColor, issue_count: usize) -> io::Result<()> {
    let issue_plural = if issue_count != 1 { "s" } else { "" };
    let color = if issue_count > 0 {
        formatter::red_color()
    } else {
        formatter::green_color()
    };
    out.set_color(&color)?;
    write!(out, "{} issue{} detected", issue_count, issue_plural)?;
    out.reset()?;
    Ok(())
}

fn init_logger(debug: bool) {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let result = log::set_boxed_logger(Box::new(Logger::new())).map(|()| log::set_max_level(level));
    match result {
        Ok(_) => (),
        Err(error) => {
            eprintln!(
                "An error occurred while initialzing the logger. \
                Cannot continue.\n{:?}",
                error
            );
            std::process::exit(2)
        }
    }
}

/// Returns a StandardStream configured to write with color or not based on the config flag set by
/// the user.
fn buffer_writer(color: bool) -> StandardStream {
    let color_choice = if color {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };
    StandardStream::stdout(color_choice)
}

#[cfg(test)]
mod tests {
    use super::with_color;
    use predicates::prelude::*;
    use regex::Regex;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    const TEST_DIR: &str = "tmp/tests/test_repo";

    fn test_dir(name: &str) -> PathBuf {
        Path::new(TEST_DIR).join(name)
    }

    fn create_test_repo(dir: &Path) {
        if Path::new(&dir).exists() {
            fs::remove_dir_all(&dir).expect("Could not remove test repo dir");
        }
        fs::create_dir_all(&dir).expect("Could not create test repo dir");
        let output = Command::new("git")
            .args(&["init"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .output()
            .expect("Could not init test repo!");
        if !output.status.success() {
            panic!(
                "Failed to initialize repo!\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git init"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
        create_commit(dir, "Initial commit", "");
    }

    fn checkout_branch(dir: &Path, name: &str) {
        let output = Command::new("git")
            .args(&["checkout", "-b", name])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|_| panic!("Could not checkout branch: {}", name));
        if !output.status.success() {
            panic!(
                "Failed to checkout branch: {}\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                name,
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git checkout"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
    }

    fn create_commit(dir: &Path, subject: &str, message: &str) {
        let mut args = vec![
            "commit".to_string(),
            "--no-gpg-sign".to_string(),
            "--allow-empty".to_string(),
            format!("-m{}", subject),
        ];
        if !message.is_empty() {
            let message_arg = format!("-m {}", message);
            args.push(message_arg)
        }
        let output = Command::new("git")
            .args(args.as_slice())
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|_| panic!("Failed to make commit: {}, {}", subject, message));
        if !output.status.success() {
            panic!(
                "Failed to make commit!\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git commit"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
    }

    fn create_commit_with_file(dir: &Path, subject: &str, message: &str, filename: &str) {
        create_file(&dir.join(&filename));
        stage_files(dir);
        create_commit(dir, subject, message)
    }

    fn create_file(file_path: &Path) {
        let mut file = match File::create(&file_path) {
            Ok(file) => file,
            Err(e) => panic!("Could not create file: {:?}: {}", file_path, e),
        };
        // Write a slice of bytes to the file
        match file.write_all(b"I am a test file!") {
            Ok(_) => (),
            Err(e) => panic!("Could not write to file: {:?}: {}", file_path, e),
        }
    }

    fn stage_files(dir: &Path) {
        let output = Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|e| panic!("Failed to add files to commit: {:?}", e));
        if !output.status.success() {
            panic!(
                "Failed to add files to commit!\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git add"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
    }

    fn configure_git_cleanup_mode(dir: &Path, mode: &str) {
        let output = Command::new("git")
            .args(&["config", "commit.cleanup", mode])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|_| panic!("Failed to configure Git commit.cleanup: {}", mode));
        if !output.status.success() {
            panic!(
                "Failed to configure Git commit.cleanup!\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git config"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
    }

    fn configure_git_comment_char(dir: &Path, character: &str) {
        let output = Command::new("git")
            .args(&["config", "core.commentChar", character])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|_| panic!("Failed to configure Git core.commentChar: {}", character));
        if !output.status.success() {
            panic!(
                "Failed to configure Git core.commentChar!\nExit code: {}\nSDTOUT: {}\nSTDERR: {}",
                output
                    .status
                    .code()
                    .expect("Could not fetch status code of git config"),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            )
        }
    }

    fn normalize_output(output: &[u8]) -> String {
        // Replace dynamic commit short SHA with 0000000 dummy placeholder
        let regexp = Regex::new("([a-z0-9]{7})(:\\d:\\d)").unwrap();
        let raw_output = String::from_utf8_lossy(output);
        regexp.replace_all(&raw_output, "0000000$2").to_string()
    }

    fn compile_bin() {
        Command::new("cargo")
            .args(&["build"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("Could not compile debug target!");
    }

    #[test]
    fn test_color_flags() {
        assert!(!with_color(true, true)); // Both color flags set, but --no-color is leading
        assert!(with_color(true, false)); // --color is set
        assert!(!with_color(false, true)); // --no-color is set
        assert!(!with_color(false, false)); // No flags are set
    }

    #[test]
    fn test_commit_by_sha() {
        compile_bin();
        let dir = test_dir("commit_by_sha");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Test commit", "", "file");
        let output = Command::new("git")
            .args(&["log", "--pretty=%H", "-n 1"])
            .current_dir(&dir)
            .output()
            .expect("Failed to fetch commit SHA.");
        let sha = String::from_utf8_lossy(&output.stdout);
        let short_sha = sha.get(0..7).expect("Unable to build short commit SHA");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &sha])
            .current_dir(dir)
            .assert()
            .failure();
        assert
            .stdout(
                predicate::str::is_match(format!("{}:\\d+:\\d+: Test commit", short_sha)).unwrap(),
            )
            .stdout(predicate::str::contains("1 commit and branch inspected"));
    }

    #[test]
    fn test_single_commit_valid() {
        compile_bin();
        let dir = test_dir("single_commit_valid");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Test commit", "I am a test commit", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd.arg("--no-color").current_dir(dir).assert().success();
        assert.stdout("1 commit and branch inspected, 0 issues detected\n");
    }

    #[test]
    fn test_single_commit_valid_with_color() {
        compile_bin();
        let dir = test_dir("single_commit_valid_with_color");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Test commit", "I am a test commit", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd.arg("--color").current_dir(dir).assert().success();
        assert.stdout(
            "1 commit and branch inspected, \u{1b}[0m\u{1b}[32m0 issues detected\u{1b}[0m\n",
        );
    }

    #[test]
    fn test_single_commit_invalid() {
        compile_bin();
        let dir = test_dir("single_commit_invalid");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Fixing tests", "", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .arg("--no-color")
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);

        let output = normalize_output(&assert.get_output().stdout);
        assert_eq!(
            output,
            "SubjectCliche: The subject does not explain the change in much detail\n\
            \x20\x200000000:1:1: Fixing tests\n\
            \x20\x20  |\n\
            \x20\x201 | Fixing tests\n\
            \x20\x20  | ^^^^^^^^^^^^ Describe the change in more detail\n\
            \n\
            SubjectMood: The subject does not use the imperative grammatical mood\n\
            \x20\x200000000:1:1: Fixing tests\n\
            \x20\x20  |\n\
            \x20\x201 | Fixing tests\n\
            \x20\x20  | ^^^^^^ Use the imperative mood for the subject\n\
            \n\
            MessagePresence: No message body was found\n\
            \x20\x200000000:3:1: Fixing tests\n\
            \x20\x20  |\n\
            \x20\x201 | Fixing tests\n\
            \x20\x202 | \n\
            \x20\x203 | \n\
            \x20\x20  | ^ Add a message body with context about the change and why it was made\n\
            \n\
            1 commit and branch inspected, 3 issues detected\n"
        );
    }

    #[test]
    fn test_single_commit_invalid_with_color() {
        compile_bin();
        let dir = test_dir("with_color");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Fixing tests", "", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .arg("--color")
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);

        let output = normalize_output(&assert.get_output().stdout);
        assert_eq!(
            output,
            "\u{1b}[0m\u{1b}[31mSubjectCliche\u{1b}[0m: The subject does not explain the change in much detail\n\
            \x20\x20\u{1b}[0m\u{1b}[38;5;12m0000000:1:1:\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\n\
            \u{1b}[0m\u{1b}[38;5;12m  1 |\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\u{1b}[38;5;9m ^^^^^^^^^^^^ Describe the change in more detail\u{1b}[0m\n\
            \n\
            \u{1b}[0m\u{1b}[31mSubjectMood\u{1b}[0m: The subject does not use the imperative grammatical mood\n\
            \x20\x20\u{1b}[0m\u{1b}[38;5;12m0000000:1:1:\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\n\
            \u{1b}[0m\u{1b}[38;5;12m  1 |\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\u{1b}[38;5;9m ^^^^^^ Use the imperative mood for the subject\u{1b}[0m\n\
            \n\
            \u{1b}[0m\u{1b}[31mMessagePresence\u{1b}[0m: No message body was found\n\
            \x20\x20\u{1b}[0m\u{1b}[38;5;12m0000000:3:1:\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\n\
            \u{1b}[0m\u{1b}[38;5;12m  1 |\u{1b}[0m Fixing tests\n\
            \u{1b}[0m\u{1b}[38;5;12m  2 |\u{1b}[0m \n\
            \u{1b}[0m\u{1b}[38;5;12m  3 |\u{1b}[0m \n\
            \u{1b}[0m\u{1b}[38;5;12m    |\u{1b}[0m\u{1b}[38;5;9m ^ Add a message body with context about the change and why it was made\u{1b}[0m\n\
            \n\
            1 commit and branch inspected, \u{1b}[0m\u{1b}[31m3 issues detected\u{1b}[0m\n"
        );
    }

    #[test]
    fn test_single_commit_invalid_one_issue() {
        compile_bin();
        let dir = test_dir("single_commit_invalid_one_issue");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Valid commit subject", "", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .arg("--no-color")
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert
            .stdout(predicate::str::contains(
                "MessagePresence: No message body was found",
            ))
            .stdout(predicate::str::contains(
                "1 commit and branch inspected, 1 issue detected",
            ));
    }

    #[test]
    fn test_single_commit_invalid_without_file_changes() {
        compile_bin();
        let dir = test_dir("single_commit_invalid_without_file_changes");
        create_test_repo(&dir);
        create_commit(&dir, "Valid commit subject", "");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .arg("--no-color")
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert
            .stdout(predicate::str::contains(
                "MessagePresence: No message body was found",
            ))
            .stdout(predicate::str::contains(
                "DiffPresence: No file changes found",
            ))
            .stdout(predicate::str::contains(
                "1 commit and branch inspected, 2 issues detected",
            ));
    }

    #[test]
    fn test_single_commit_ignored() {
        compile_bin();
        let dir = test_dir("single_commit_ignored");
        create_test_repo(&dir);
        create_commit_with_file(
            &dir,
            "Merge pull request #123 from tombruijn/repo",
            "",
            "file",
        );

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd.arg("--no-color").current_dir(dir).assert().success();
        assert.stdout("0 commits and branch inspected, 0 issues detected (1 commit ignored)\n");
    }

    #[test]
    fn test_single_commit_ignored_with_color() {
        compile_bin();
        let dir = test_dir("single_commit_ignored_with_color");
        create_test_repo(&dir);
        create_commit_with_file(
            &dir,
            "Merge pull request #123 from tombruijn/repo",
            "",
            "file",
        );

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd.arg("--color").current_dir(dir).assert().success();
        assert.stdout("0 commits and branch inspected, \u{1b}[0m\u{1b}[32m0 issues detected\u{1b}[0m (1 commit ignored)\n");
    }

    #[test]
    fn test_single_commit_with_debug() {
        compile_bin();
        let dir = test_dir("single_commit_with_debug");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Valid commit subject", "Valid message body", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", "--debug"])
            .current_dir(dir)
            .assert()
            .success();
        assert.stdout(predicate::str::contains(
            "1 commit and branch inspected, 0 issues detected (0 commits ignored)",
        ));
    }

    #[test]
    fn test_multiple_commit_invalid() {
        compile_bin();
        let dir = test_dir("multiple_commits_invalid");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "added some code", "This is a message.", "file1");
        create_commit_with_file(&dir, "Fixing tests", "", "file2");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", "HEAD~2..HEAD"])
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        let output = normalize_output(&assert.get_output().stdout);

        assert!(predicate::str::contains(
            "SubjectMood: The subject does not use the imperative grammatical mood\n\
            \x20\x200000000:1:1: Fixing tests\n"
        )
        .eval(&output));
        assert!(predicate::str::contains(
            "SubjectCapitalization: The subject does not start with a capital letter\n\
            \x20\x200000000:1:1: added some code\n"
        )
        .eval(&output));
        assert.stdout(predicate::str::contains(
            "2 commits and branch inspected, 5 issues detected",
        ));
    }

    #[test]
    fn test_lint_hook() {
        compile_bin();
        let dir = test_dir("commit_file_option");
        create_test_repo(&dir);
        let filename = "commit_message_file";
        let commit_file = dir.join(filename);
        let mut file = File::create(&commit_file).unwrap();
        file.write_all(b"added some code\n\nThis is a message.")
            .unwrap();

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &format!("--hook-message-file={}", filename)])
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert
            .stdout(predicate::str::contains(
                "SubjectMood: The subject does not use the imperative grammatical mood",
            ))
            .stdout(predicate::str::contains(
                "SubjectCapitalization: The subject does not start with a capital letter",
            ))
            .stdout(predicate::str::contains(
                "DiffPresence: No file changes found",
            ))
            .stdout(predicate::str::contains(
                "1 commit and branch inspected, 3 issues detected",
            ));
    }

    #[test]
    fn test_file_option_with_file_changes() {
        compile_bin();
        let dir = test_dir("commit_file_option_with_file_changes");
        create_test_repo(&dir);
        create_file(&dir.join("file name"));
        stage_files(&dir);
        let filename = "commit_message_file";
        let commit_file = dir.join(filename);
        let mut file = File::create(&commit_file).unwrap();
        file.write_all(b"Valid subject\n\nValid message body.")
            .unwrap();

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &format!("--hook-message-file={}", filename)])
            .current_dir(dir)
            .assert()
            .success();
        assert.stdout(predicate::str::contains(
            "1 commit and branch inspected, 0 issues detected",
        ));
    }

    #[test]
    fn test_file_option_with_scissors_cleanup() {
        compile_bin();
        let dir = test_dir("commit_file_option_with_scissors_cleanup_default_comment_char");
        create_test_repo(&dir);
        configure_git_cleanup_mode(&dir, "scissors");
        let filename = "commit_message_file";
        let commit_file = dir.join(filename);
        let mut file = File::create(&commit_file).unwrap();
        file.write_all(
            b"This is a subject\n\n\
            # ------------------------ >8 ------------------------
            # This is part of the comment that will be ignored
            ",
        )
        .unwrap();

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &format!("--hook-message-file={}", filename)])
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert.stdout(predicate::str::contains("MessagePresence: "));
    }

    #[test]
    fn test_file_option_with_scissors_cleanup_custom_comment_char() {
        compile_bin();
        let dir = test_dir("commit_file_option_with_scissors_cleanup_custom_comment_char");
        create_test_repo(&dir);
        configure_git_cleanup_mode(&dir, "scissors");
        configure_git_comment_char(&dir, "-");
        let filename = "commit_message_file";
        let commit_file = dir.join(filename);
        let mut file = File::create(&commit_file).unwrap();
        file.write_all(
            b"This is a subject\n\n\
            - ------------------------ >8 ------------------------
            - This is part of the comment that will be ignored
            ",
        )
        .unwrap();

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &format!("--hook-message-file={}", filename)])
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert.stdout(predicate::str::contains("MessagePresence: "));
    }

    #[test]
    fn test_file_option_without_file() {
        compile_bin();
        let dir = test_dir("commit_file_option_without_file");
        create_test_repo(&dir);
        let filename = "commit_message_file";

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", &format!("--hook-message-file={}", filename)])
            .current_dir(dir)
            .assert()
            .failure()
            .code(2);
        assert.stdout(predicate::str::contains(
            "Unable to open commit message file: commit_message_file",
        ));
    }

    #[test]
    fn test_branch_valid() {
        compile_bin();
        let dir = test_dir("branch_valid");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Test commit", "I am a test commit.", "file");
        checkout_branch(&dir, "my-branch");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd.arg("--no-color").current_dir(dir).assert().success();
        assert.stdout(predicate::str::contains(
            "1 commit and branch inspected, 0 issues detected",
        ));
    }

    #[test]
    fn test_branch_invalid() {
        compile_bin();
        let dir = test_dir("branch_invalid");
        create_test_repo(&dir);
        checkout_branch(&dir, "fix-123");
        create_commit_with_file(&dir, "Test commit", "I am a test commit.", "file");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .arg("--no-color")
            .current_dir(dir)
            .assert()
            .failure()
            .code(1);
        assert
            .stdout(predicate::str::contains(
                "BranchNameTicketNumber: A ticket number was detected in the branch name\n\
                \x20\x20Branch:1: fix-123\n\
                \x20\x20|\n\
                \x20\x20| fix-123\n\
                \x20\x20| ^^^^^^^ Remove the ticket number from the branch name or expand the branch name with more details\n"
            ))
            .stdout(predicate::str::contains(
                "BranchNameCliche: The branch name does not explain the change in much detail\n\
                \x20\x20Branch:1: fix-123\n\
                \x20\x20|\n\
                \x20\x20| fix-123\n\
                \x20\x20| ^^^^^^^ Describe the change in more detail\n"
            ))
            .stdout(predicate::str::contains(
                    "1 commit and branch inspected, 2 issues detected",
            ));
    }

    #[test]
    fn test_no_branch_validation() {
        compile_bin();
        let dir = test_dir("branch_invalid_disabled");
        create_test_repo(&dir);
        create_commit_with_file(&dir, "Test commit", "I am a test commit.", "file");
        checkout_branch(&dir, "fix-123");

        let mut cmd = assert_cmd::Command::cargo_bin("lintje").unwrap();
        let assert = cmd
            .args(["--no-color", "--no-branch"])
            .current_dir(dir)
            .assert()
            .success();
        assert.stdout(predicate::str::contains(
            "1 commit inspected, 0 issues detected",
        ));
    }
}
