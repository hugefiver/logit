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

# Ordered primary fallback
logit stats --group repo,author,language

# Nested subgroup levels under the selected primary
logit stats --group repo,author,language --groups author,period

# Compact / short output
logit stats --compact --short
```

#### Group options

`--group` is an ordered fallback list of primary dimensions: `repo`, `author`,
`period`, `language`. It selects the first dimension with more than one distinct
value. When exactly one non-language dimension is explicitly requested, it is
retained even when unique so the requested group remains visible; otherwise,
`language` is the final fallback.

`--groups` adds nested subgroup levels under that selected primary. For example,
`logit stats --group repo,author,language --groups author,period` selects
`author` when there is only one repository, then nests periods below each author.

- A subgroup with only one distinct value is skipped.
- One subgroup occurrence equal to the selected primary is removed; any other duplicate dimension is an error.
- `language` may only be the final grouping level.
- Local statistics support `repo`, `author`, `period`, and `language`.
- GitHub contribution statistics support `repo`, `period`, and `language`; `author` is rejected because contribution records have no author identity.

#### Excluding repos, languages, and paths

`--exclude` filters repos, languages, or file paths. Repeatable.

```sh
# Exclude entire repos
logit stats --exclude old-project --exclude archived-repo

# Exclude a language within a specific repo
logit stats --exclude my-repo:lang:markdown

# Exclude file paths matching a glob within a repo
logit stats --exclude my-repo:path:docs/**
logit stats --exclude frontend:path:**/*.test.ts

# Combine qualifiers for one repo
logit stats --exclude "my-repo:lang:md,p:*.md"

# Global language/path exclusion (no repo prefix)
logit stats --exclude :lang:json --exclude :path:**/*.lock
```

### GitHub statistics

Requires a `GITHUB_TOKEN` environment variable (PAT with `read:user` scope).

```sh
# Fetch contribution stats
logit github fetch <username>
logit github fetch <username> --period week --include-contributed
logit github fetch <username> --group repo,language --groups period,language

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
| `exclude` | | Multi-line exclusions (repo[:lang:LANG]) |
| `short` | `false` | Compact card layout |
| `lang-rows` | `2` | Language rows |
| `title` | | Custom title |
| `output` | `profile-card.svg` | Output path |
| `retry-count` | `3` | Retries after the initial attempt |
| `retry-delay` | `5` | Seconds between retries |

## License

[Anti American AI Public License](https://github.com/hugefiver/AAAPL) - See [LICENSE](LICENSE) for details.
