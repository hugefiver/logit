use std::{fs, path::PathBuf, process::Command};

use assert_cmd::cargo::CommandCargoExt;
use tempfile::TempDir;

const RAW_PAYLOAD: &str = r#"<>&"'"#;

fn generate_cli_svg_with_metacharacter_json() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let input_path = temp.path().join("card.json");
    let svg_path = temp.path().join("card.svg");
    let json = serde_json::json!({
        "metadata": {
            "username": RAW_PAYLOAD,
            "days": 30,
            "active_repos": 1,
        },
        "user": {
            "login": RAW_PAYLOAD,
            "name": RAW_PAYLOAD,
            "bio": RAW_PAYLOAD,
            "public_repos": 1,
            "followers": 2,
            "following": 3,
            "avatar_url": RAW_PAYLOAD,
            "html_url": RAW_PAYLOAD,
            "created_at": RAW_PAYLOAD,
        },
        "summary": {
            "total_prs": 4,
            "total_reviews": 5,
            "total_issues": 6,
        },
        "totals": {
            "total_commits": 7,
            "total_additions": 8,
            "total_deletions": 9,
            "total_net_modifications": 10,
            "total_net_additions": 11,
            "by_language": {
                RAW_PAYLOAD: {
                    "additions": 12,
                    "deletions": 13,
                    "files_changed": 14,
                    "net_modifications": 15,
                    "net_additions": 16,
                },
            },
        },
    });
    fs::write(
        &input_path,
        serde_json::to_vec_pretty(&json).expect("serialize card JSON"),
    )
    .expect("write card JSON");

    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .args(["github", "card", "--input"])
        .arg(&input_path)
        .args(["--output"])
        .arg(&svg_path)
        .output()
        .expect("run logit github card");
    assert!(
        output.status.success(),
        "logit github card failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    (temp, svg_path)
}

fn find_pwsh() -> Option<PathBuf> {
    let executable = PathBuf::from("pwsh");
    Command::new(&executable)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| executable)
}

#[test]
fn cli_json_input_escapes_dynamic_xml_metacharacters() {
    let (_temp, svg_path) = generate_cli_svg_with_metacharacter_json();
    let svg = fs::read_to_string(svg_path).expect("read generated SVG");

    assert!(svg.contains("&lt;&gt;&amp;&quot;"));
    assert!(
        svg.contains("&lt;&gt;&amp;&quot;&#39;") || svg.contains("&lt;&gt;&amp;&quot;&#x27;"),
        "expected apostrophes to use a numeric XML entity: {svg}"
    );
    assert!(!svg.contains(RAW_PAYLOAD));
    assert!(!svg.contains("<>&"));
    assert!(!svg.contains("\"'"));
}

#[test]
fn cli_generated_svg_is_well_formed_xml_when_pwsh_available() {
    let Some(pwsh) = find_pwsh() else {
        return;
    };
    let (_temp, svg_path) = generate_cli_svg_with_metacharacter_json();
    let svg_path_literal = svg_path.display().to_string().replace('\'', "''");
    let parser_script = format!(
        "& {{ $ErrorActionPreference='Stop'; $doc=[xml](Get-Content -Raw -LiteralPath $args[0]); if ($doc.DocumentElement.LocalName -ne 'svg') {{ throw 'root is not svg' }}; 'DOTNET_XML_PARSED' }} '{svg_path_literal}'"
    );
    let output = Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-Command", &parser_script])
        .output()
        .expect("run PowerShell XML parser");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "PowerShell XML parse failed\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(stdout.contains("DOTNET_XML_PARSED"));
    println!("DOTNET_XML_PARSED");
}
