# logit — lines of git

CLI tool for analyzing Git repository statistics — locally or from GitHub.

## Install

```sh
cargo install --path . --features github
```

Without GitHub features:

```sh
cargo install --path .
```

## Usage

### Scan repositories

Find Git repositories under a directory:

```sh
logit scan /path/to/projects
```

### Local statistics

Analyze one or more local repos:

```sh
# Single repo (current directory)
logit stats

# Multiple repos
logit stats /path/to/repo1 /path/to/repo2

# Recursive scan + stats
logit stats /path/to/projects

# Filter by author, period, language
logit stats --author "Alice" --period week --lang Rust

# Multi-level grouping
logit stats --group repo,author,period
logit stats --group repo,language

# Compact / short output
logit stats --compact --short
```

#### Group options

`--group` accepts a comma-separated list: `repo`, `author`, `period`, `language`.

- Single group behaves like a flat table (backward-compatible).
- Multiple groups produce a nested tree. For example `--group repo,author` shows authors within each repo.
- If a grouping level has only one unique value across all data, it is automatically skipped.
- `language` can only appear as the last group.

### GitHub statistics

Requires a `GITHUB_TOKEN` environment variable (PAT with `read:user` scope).

```sh
# Fetch contribution stats
logit github fetch <username>
logit github fetch <username> --period week --include-contributed

# Include private repos (token must belong to <username>; bypasses fine-grained PAT
# limitation that hides private contributions in contributionsCollection)
logit github fetch <username> --include-private --include-contributed --include-forks

# Generate SVG profile card
logit github card <username>
logit github card <username> --short --days 90

# Multi-period comparison card
logit github multi <username> -p week,month,year
```

### Output formats

```sh
# Table (default), JSON, or TUI (if compiled with tui feature)
logit stats -f table
logit stats -f json
logit stats -f tui

# Write to file
logit stats -o stats.txt
logit github card <username> -o card.svg
```

## GitHub Action

Use logit as a GitHub Action to automatically generate and update profile cards:

```yaml
name: Update Profile Card
on:
  schedule:
    - cron: '0 0 * * 1'  # Weekly
  workflow_dispatch:

jobs:
  card:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - uses: hugefiver/logit@master
        with:
          username: ${{ github.actor }}
          # token: ${{ github.token }}  # default, or use a PAT for private repos
          command: card
          days: '365'
          include-contributed: 'true'
          output: profile-card.svg

      - uses: stefanzweifel/git-auto-commit-action@v7
        with:
          commit_message: 'chore: update profile card'
          file_pattern: 'profile-card.svg'
```

### Action inputs

| Input | Default | Description |
|-------|---------|-------------|
| `username` | *(required)* | GitHub username |
| `token` | `${{ github.token }}` | GitHub token |
| `command` | `card` | `card` or `multi` |
| `days` | `365` | Lookback days (card) |
| `periods` | `week,month,year` | Periods (multi) |
| `include-forks` | `false` | Include forks |
| `include-contributed` | `false` | Include contributed repos |
| `include-private` | `false` | Include token holder's private repos (requires PAT matching `username`; default `${{ github.token }}` is silently ignored) |
| `exclude-lang` | | Languages to exclude |
| `short` | `false` | Compact card layout |
| `lang-rows` | `2` | Language rows |
| `title` | | Custom title |
| `output` | `profile-card.svg` | Output path |

## License

[Anti American AI Public License](https://github.com/hugefiver/AAAPL) - See [LICENSE](LICENSE) for details.
