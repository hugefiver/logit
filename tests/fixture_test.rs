mod common;
use tempfile::TempDir;

#[test]
fn test_fixture_creates_five_commits() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    let count = revwalk.count();
    assert_eq!(count, 5, "Expected 5 commits, got {count}");
}

#[test]
fn test_fixture_authors() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .unwrap();

    let commits: Vec<_> = revwalk
        .map(|oid| repo.find_commit(oid.unwrap()).unwrap())
        .collect();

    assert_eq!(commits[0].author().name().unwrap(), "Alice");
    assert_eq!(commits[0].author().email().unwrap(), "alice@test.com");
    assert_eq!(commits[1].author().name().unwrap(), "Bob");
    assert_eq!(commits[1].author().email().unwrap(), "bob@test.com");
    assert_eq!(commits[2].author().name().unwrap(), "Alice");
}

#[test]
fn test_fixture_co_author_trailer() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .unwrap();

    let commits: Vec<_> = revwalk
        .map(|oid| repo.find_commit(oid.unwrap()).unwrap())
        .collect();

    let msg = commits[2].message().unwrap();
    assert!(msg.contains("Co-authored-by: Charlie"));
}
