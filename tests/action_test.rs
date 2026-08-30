use std::{collections::HashMap, fs, path::Path};

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

fn normalized_source(path: impl AsRef<Path>, error_message: &str) -> String {
    fs::read_to_string(path)
        .expect(error_message)
        .replace("\r\n", "\n")
}

fn action_source() -> String {
    normalized_source(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("action.yml"),
        "read action.yml",
    )
}

fn ci_source() -> String {
    normalized_source(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".github")
            .join("workflows")
            .join("ci.yml"),
        "read CI workflow",
    )
}

fn declared_input_names(source: &str) -> Vec<String> {
    let inputs = source
        .strip_prefix("name: 'Logit GitHub Card'\n")
        .and_then(|source| source.split_once("inputs:\n"))
        .expect("inputs block")
        .1
        .split_once("outputs:\n")
        .expect("outputs block")
        .0;

    inputs
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  ")
                .and_then(|line| line.strip_suffix(':'))
                .filter(|name| !name.contains(' '))
                .map(str::to_owned)
        })
        .collect()
}

fn declared_output_names(source: &str) -> Vec<String> {
    let outputs = source
        .split_once("outputs:\n")
        .expect("outputs block")
        .1
        .split_once("runs:\n")
        .expect("runs block")
        .0;

    outputs
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  ")
                .and_then(|line| line.strip_suffix(':'))
                .filter(|name| !name.contains(' '))
                .map(str::to_owned)
        })
        .collect()
}

fn data_cache_key_line(source: &str) -> &str {
    source
        .lines()
        .find(|line| line.trim_start().starts_with("key: logit-data-"))
        .expect("GitHub data cache key")
}

fn named_step<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("    - name: {name}\n"))
        .expect("named action step");
    let step = &source[start..];
    let end = step.find("\n    - ").unwrap_or(step.len());
    &step[..end]
}

fn generate_svg_step(source: &str) -> &str {
    named_step(source, "Generate SVG")
}

fn step_run_block(step: &str) -> String {
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
        if let Some(input) = expression
            .strip_prefix("${{ inputs.")
            .and_then(|expression| expression.strip_suffix(" }}"))
        {
            mappings.insert(input.to_owned(), name.to_owned());
        }
    }

    mappings
}

#[test]
fn ci_has_exact_locked_ubuntu_quality_and_windows_test_contract() {
    let source = ci_source();

    for command in [
        "cargo fmt --all -- --check",
        "cargo clippy --locked --all-targets --all-features -- -D warnings",
        "cargo check --locked --no-default-features",
        "cargo build --locked --release --all-features",
    ] {
        assert_eq!(source.matches(command).count(), 1, "command: {command}");
    }
    assert_eq!(
        source.matches("cargo test --locked --all-features").count(),
        2
    );
    assert!(source.contains("runs-on: ubuntu-latest"));
    assert!(source.contains("runs-on: windows-latest"));
    assert!(source.contains("components: rustfmt, clippy"));
    assert!(!source.to_ascii_lowercase().contains("macos"));
    assert!(!source.to_ascii_lowercase().contains("msrv"));
}

#[test]
fn action_preserves_inputs_output_and_uses_one_cross_platform_cache_directory() {
    let source = action_source();
    assert_eq!(
        declared_input_names(&source),
        INPUT_NAMES
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
    assert_eq!(declared_output_names(&source), ["svg-path"]);

    let cache_dir = "${{ runner.temp }}/logit-github-cache";
    assert_eq!(source.matches(&format!("path: {cache_dir}")).count(), 1);
    assert_eq!(
        source
            .matches(&format!("LOGIT_GITHUB_CACHE_DIR: {cache_dir}"))
            .count(),
        1
    );
    assert!(source.contains("logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-${{ github.run_id }}-${{ github.run_attempt }}"));
    assert!(source.contains("logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-"));
    assert!(!data_cache_key_line(&source).contains("inputs.username"));
    assert!(source.contains("cargo install --locked --path"));

    let data_key_step = named_step(&source, "Compute GitHub data cache key");
    let data_inputs = [
        "username",
        "command",
        "days",
        "periods",
        "include-forks",
        "include-contributed",
        "include-private",
    ];
    let mappings = input_env_mappings(data_key_step);
    assert_eq!(mappings.len(), data_inputs.len());
    for input in data_inputs {
        assert!(
            mappings.contains_key(input),
            "missing data input mapping for {input}"
        );
    }
    let run_block = step_run_block(data_key_step);
    assert!(!run_block.contains("${{ inputs."));
    assert!(run_block.contains("printf '%s\\0'"));
    assert!(run_block.contains("sha256sum"));
    assert_eq!(
        step_run_block(generate_svg_step(&source))
            .matches("--refresh-cache")
            .count(),
        1
    );
}

#[test]
fn action_run_block_has_no_expression_interpolation_eval_or_xargs() {
    let source = action_source();
    let step = generate_svg_step(&source);
    let run_block = step_run_block(step);

    assert!(!run_block.contains("${{ inputs."));
    assert!(!run_block.contains("eval"));
    assert!(!run_block.contains("xargs"));
    assert!(!run_block.contains("GITHUB_TOKEN"));
    assert!(!run_block.contains("INPUT_TOKEN"));
    assert!(run_block.contains("\"${cmd[@]}\""));

    let mappings = input_env_mappings(step);
    for input in INPUT_NAMES {
        assert!(
            mappings.contains_key(input),
            "missing env mapping for {input}"
        );
    }
}
