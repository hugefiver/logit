use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

const INPUT_NAMES: [&str; 16] = [
    "username",
    "token",
    "command",
    "days",
    "periods",
    "include-forks",
    "include-contributed",
    "include-private",
    "exclude-lang",
    "exclude",
    "short",
    "lang-rows",
    "title",
    "output",
    "retry-count",
    "retry-delay",
];

fn action_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("action.yml"))
        .expect("read action.yml")
}

fn generate_svg_step(source: &str) -> &str {
    let start = source
        .find("    - name: Generate SVG\n")
        .expect("Generate SVG step");
    let step = &source[start..];
    let end = step.find("\n    - ").unwrap_or(step.len());
    &step[..end]
}

fn generate_svg_run_block(step: &str) -> String {
    let marker = "      run: |\n";
    let start = step.find(marker).expect("Generate SVG run block") + marker.len();
    let mut run_block = String::new();

    for line in step[start..].lines() {
        if line.is_empty() {
            run_block.push('\n');
        } else if let Some(line) = line.strip_prefix("        ") {
            run_block.push_str(line);
            run_block.push('\n');
        } else {
            break;
        }
    }

    run_block
}

fn input_env_mappings(step: &str) -> HashMap<String, String> {
    let marker = "      env:\n";
    let start = step.find(marker).expect("Generate SVG env block") + marker.len();
    let mut mappings = HashMap::new();

    for line in step[start..].lines() {
        let Some(line) = line.strip_prefix("        ") else {
            break;
        };
        let (name, expression) = line.split_once(": ").expect("env mapping");
        let input = expression
            .strip_prefix("${{ inputs.")
            .and_then(|expression| expression.strip_suffix(" }}"))
            .expect("input expression in Generate SVG env block");
        mappings.insert(input.to_owned(), name.to_owned());
    }

    mappings
}

fn valid_inputs() -> HashMap<String, String> {
    HashMap::from([
        ("username".to_owned(), "octocat".to_owned()),
        ("token".to_owned(), "secret-token-value".to_owned()),
        ("command".to_owned(), "card".to_owned()),
        ("days".to_owned(), "365".to_owned()),
        ("periods".to_owned(), "week,month,year".to_owned()),
        ("include-forks".to_owned(), "false".to_owned()),
        ("include-contributed".to_owned(), "false".to_owned()),
        ("include-private".to_owned(), "false".to_owned()),
        ("exclude-lang".to_owned(), "".to_owned()),
        ("exclude".to_owned(), "".to_owned()),
        ("short".to_owned(), "false".to_owned()),
        ("lang-rows".to_owned(), "2".to_owned()),
        ("title".to_owned(), "".to_owned()),
        ("output".to_owned(), "profile-card.svg".to_owned()),
        ("retry-count".to_owned(), "0".to_owned()),
        ("retry-delay".to_owned(), "0".to_owned()),
    ])
}

fn find_bash() -> bool {
    Command::new("bash")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn write_stub(bin_dir: &Path, statuses: &[i32]) -> (PathBuf, PathBuf) {
    fs::create_dir_all(bin_dir).expect("create stub directory");
    let argv_log = bin_dir.join("argv.log");
    let count_file = bin_dir.join("count");
    let status_file = bin_dir.join("statuses");
    let stub = bin_dir.join("logit");

    fs::write(
        &stub,
        "#!/usr/bin/env bash\nprintf '%s\\0' \"$@\" >> \"$LOGIT_ARGV_LOG\"\ncount=0\nif [ -f \"$LOGIT_COUNT_FILE\" ]; then count=$(<\"$LOGIT_COUNT_FILE\"); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$LOGIT_COUNT_FILE\"\nstatus=\nindex=0\nwhile IFS= read -r next_status || [ -n \"$next_status\" ]; do\n  index=$((index + 1))\n  if [ \"$index\" -eq \"$count\" ]; then status=$next_status; break; fi\ndone < \"$LOGIT_STATUS_FILE\"\nexit \"$status\"\n",
    )
    .expect("write logit stub");
    fs::write(bin_dir.join("sleep"), "#!/bin/bash\nexit 0\n").expect("write sleep stub");
    fs::write(
        &status_file,
        statuses
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("write statuses");

    (argv_log, count_file)
}

fn bash_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn run_action(
    inputs: &HashMap<String, String>,
    temp: &TempDir,
    statuses: &[i32],
) -> (Output, PathBuf, PathBuf) {
    let source = action_source();
    let step = generate_svg_step(&source);
    let input_env = input_env_mappings(step);
    let bin_dir = temp.path().join("bin");
    let (argv_log, count_file) = write_stub(&bin_dir, statuses);
    let mut environment = Vec::new();

    for (input, env_name) in input_env {
        environment.push(format!(
            "export {env_name}={}",
            bash_literal(&inputs[&input])
        ));
    }
    environment.push("export PATH=\"$PWD/bin:$PATH\"".to_owned());
    environment.push("export LOGIT_ARGV_LOG='bin/argv.log'".to_owned());
    environment.push("export LOGIT_COUNT_FILE='bin/count'".to_owned());
    environment.push("export LOGIT_STATUS_FILE='bin/statuses'".to_owned());
    let script_path = temp.path().join("run-action.sh");
    fs::write(
        &script_path,
        format!(
            "{}\n{}",
            environment.join("\n"),
            generate_svg_run_block(step)
        ),
    )
    .expect("write action test script");

    let mut command = Command::new("bash");
    command.arg("run-action.sh").current_dir(temp.path());

    (
        command.output().expect("run Generate SVG block"),
        argv_log,
        count_file,
    )
}

fn invocation_count(count_file: &Path) -> usize {
    fs::read_to_string(count_file)
        .expect("read invocation count")
        .parse()
        .expect("numeric invocation count")
}

fn logged_argv(argv_log: &Path) -> Vec<String> {
    fs::read(argv_log)
        .expect("read argv log")
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8(argument.to_vec()).expect("UTF-8 argv"))
        .collect()
}

#[test]
fn action_run_block_has_no_expression_interpolation_eval_or_xargs() {
    let source = action_source();
    let step = generate_svg_step(&source);
    let run_block = generate_svg_run_block(step);

    assert!(!run_block.contains("${{ inputs."));
    assert!(!run_block.contains("eval"));
    assert!(!run_block.contains("xargs"));
    assert!(run_block.contains("\"${cmd[@]}\""));

    let mappings = input_env_mappings(step);
    for input in INPUT_NAMES {
        assert!(
            mappings.contains_key(input),
            "missing env mapping for {input}"
        );
    }
}

#[test]
fn action_stub_preserves_literal_argv_and_final_status_when_bash_exists() {
    if !find_bash() {
        return;
    }

    let temp = TempDir::new().expect("temporary directory");
    let unsafe_title = "x'; echo PWNED; $(touch never)";
    let mut inputs = valid_inputs();
    inputs.insert("token".to_owned(), "do-not-log-this-token".to_owned());
    inputs.insert("include-forks".to_owned(), "true".to_owned());
    inputs.insert("include-contributed".to_owned(), "true".to_owned());
    inputs.insert("include-private".to_owned(), "true".to_owned());
    inputs.insert("exclude-lang".to_owned(), "Rust,TypeScript".to_owned());
    inputs.insert("exclude".to_owned(), "repo one\nrepo;two".to_owned());
    inputs.insert("short".to_owned(), "true".to_owned());
    inputs.insert("title".to_owned(), unsafe_title.to_owned());
    inputs.insert("output".to_owned(), "out.svg".to_owned());
    inputs.insert("retry-count".to_owned(), "2".to_owned());
    let (output, argv_log, count_file) = run_action(&inputs, &temp, &[23, 23, 23]);

    assert_eq!(output.status.code(), Some(23));
    assert_eq!(invocation_count(&count_file), 3);
    let expected_argv = [
        "github",
        "card",
        "octocat",
        "-d",
        "365",
        "--short",
        "--lang-rows",
        "2",
        "--title",
        unsafe_title,
        "--include-forks",
        "--include-contributed",
        "--include-private",
        "--exclude-lang",
        "Rust,TypeScript",
        "--exclude",
        "repo one",
        "--exclude",
        "repo;two",
        "-o",
        "out.svg",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let mut expected_invocations = Vec::new();
    for _ in 0..3 {
        expected_invocations.extend(expected_argv.iter().cloned());
    }
    assert_eq!(logged_argv(&argv_log), expected_invocations);
    assert!(!temp.path().join("never").exists());
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_text.contains("do-not-log-this-token"));
}

#[test]
fn action_invalid_inputs_fail_before_stub() {
    if !find_bash() {
        return;
    }

    let invalid_inputs = [
        ("command", "invalid"),
        ("include-forks", "yes"),
        ("include-contributed", "1"),
        ("include-private", "TRUE"),
        ("short", "no"),
        ("retry-count", "-1"),
        ("retry-delay", "NaN"),
        ("days", "0"),
        ("lang-rows", "-1"),
        ("periods", "week,,year"),
    ];

    for (input, invalid_value) in invalid_inputs {
        let temp = TempDir::new().expect("temporary directory");
        let mut inputs = valid_inputs();
        if input == "periods" {
            inputs.insert("command".to_owned(), "multi".to_owned());
        }
        inputs.insert(input.to_owned(), invalid_value.to_owned());
        let (output, _argv_log, count_file) = run_action(&inputs, &temp, &[0]);

        assert_eq!(output.status.code(), Some(2), "{input}={invalid_value}");
        assert!(
            !count_file.exists(),
            "stub ran for invalid {input}={invalid_value}"
        );
    }
}

#[test]
fn action_retry_count_zero_runs_once() {
    if !find_bash() {
        return;
    }

    let temp = TempDir::new().expect("temporary directory");
    let inputs = valid_inputs();
    let (output, _argv_log, count_file) = run_action(&inputs, &temp, &[0]);

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(invocation_count(&count_file), 1);
}

#[test]
fn action_retry_count_is_retries_after_initial_and_preserves_last_status() {
    if !find_bash() {
        return;
    }

    let temp = TempDir::new().expect("temporary directory");
    let mut inputs = valid_inputs();
    inputs.insert("retry-count".to_owned(), "2".to_owned());
    let (output, _argv_log, count_file) = run_action(&inputs, &temp, &[17, 17, 29]);

    assert_eq!(output.status.code(), Some(29));
    assert_eq!(invocation_count(&count_file), 3);
}

#[test]
fn action_token_and_unsafe_command_are_not_logged() {
    if !find_bash() {
        return;
    }

    let temp = TempDir::new().expect("temporary directory");
    let unsafe_title = "x'; echo PWNED; $(touch never)";
    let mut inputs = valid_inputs();
    inputs.insert("token".to_owned(), "sensitive-token-value".to_owned());
    inputs.insert("title".to_owned(), unsafe_title.to_owned());
    let (output, _argv_log, _count_file) = run_action(&inputs, &temp, &[0]);

    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    assert!(!output_text.contains("sensitive-token-value"));
    assert!(!output_text.contains(unsafe_title));
    assert!(!output_text.contains("Running:"));
}
