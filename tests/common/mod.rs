use std::path::Path;

use git2::{Oid, Repository, Signature, Time};

/// Creates a deterministic test repository with exactly 5 commits.
/// This fixture is the source of truth for all numeric assertions in tests.
pub fn create_test_repo(dir: &Path) -> Repository {
    let repo = Repository::init(dir).expect("failed to init repo");

    let commit_oid_1 = make_commit_1(&repo);
    let commit_oid_2 = make_commit_2(&repo, commit_oid_1);
    let commit_oid_3 = make_commit_3(&repo, commit_oid_2);
    let commit_oid_4 = make_commit_4(&repo, commit_oid_3);
    make_commit_5(&repo, commit_oid_4);

    repo
}

fn make_commit_1(repo: &Repository) -> Oid {
    let sig =
        Signature::new("Alice", "alice@test.com", &Time::new(1_705_312_800, 0)).expect("signature");

    let main_rs = concat!(
        "fn main() {\n",
        "    println!(\"Hello, world!\");\n",
        "}\n",
        "\n",
        "fn add(a: i32, b: i32) -> i32 {\n",
        "    a + b\n",
        "}\n",
        "\n",
        "fn sub(a: i32, b: i32) -> i32 {\n",
        "    a - b\n",
        "}\n",
    );

    let readme = concat!(
        "# Test Project\n",
        "\n",
        "This is a test.\n",
        "\n",
        "More info here.\n",
    );

    let main_rs_blob = repo.blob(main_rs.as_bytes()).expect("blob main.rs");
    let mut src_tb = repo.treebuilder(None).expect("treebuilder src");
    src_tb
        .insert("main.rs", main_rs_blob, 0o100644)
        .expect("insert main.rs");
    let src_oid = src_tb.write().expect("write src tree");

    let readme_blob = repo.blob(readme.as_bytes()).expect("blob README.md");
    let mut root_tb = repo.treebuilder(None).expect("treebuilder root");
    root_tb
        .insert("README.md", readme_blob, 0o100644)
        .expect("insert README.md");
    root_tb
        .insert("src", src_oid, 0o040000)
        .expect("insert src");
    let tree_oid = root_tb.write().expect("write root tree");

    let tree = repo.find_tree(tree_oid).expect("find tree");
    repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .expect("commit 1")
}

fn make_commit_2(repo: &Repository, parent_oid: Oid) -> Oid {
    let sig =
        Signature::new("Bob", "bob@test.com", &Time::new(1_705_413_600, 0)).expect("signature");

    let main_rs = concat!(
        "fn main() {\n",
        "    println!(\"Hello, logit!\");\n",
        "}\n",
        "\n",
        "fn add(a: i32, b: i32) -> i32 {\n",
        "    a + b\n",
        "}\n",
        "\n",
        "fn sub(a: i32, b: i32) -> i32 {\n",
        "    a - b\n",
        "}\n",
        "\n",
        "fn mul(a: i32, b: i32) -> i32 {\n",
        "    a * b\n",
        "}\n",
    );

    let lib_py = concat!(
        "def hello():\n",
        "    print(\"hello\")\n",
        "\n",
        "def greet(name):\n",
        "    print(f\"hello {name}\")\n",
        "\n",
        "def farewell():\n",
        "    print(\"goodbye\")\n",
    );

    let parent = repo.find_commit(parent_oid).expect("find parent");
    let parent_tree = parent.tree().expect("parent tree");
    let parent_src_oid = parent_tree.get_name("src").expect("src entry").id();
    let parent_src_tree = repo.find_tree(parent_src_oid).expect("parent src tree");

    let main_rs_blob = repo.blob(main_rs.as_bytes()).expect("blob main.rs");
    let lib_py_blob = repo.blob(lib_py.as_bytes()).expect("blob lib.py");

    let mut src_tb = repo
        .treebuilder(Some(&parent_src_tree))
        .expect("treebuilder src");
    src_tb
        .insert("main.rs", main_rs_blob, 0o100644)
        .expect("insert main.rs");
    src_tb
        .insert("lib.py", lib_py_blob, 0o100644)
        .expect("insert lib.py");
    let src_oid = src_tb.write().expect("write src tree");

    let mut root_tb = repo
        .treebuilder(Some(&parent_tree))
        .expect("treebuilder root");
    root_tb
        .insert("src", src_oid, 0o040000)
        .expect("insert src");
    let tree_oid = root_tb.write().expect("write root tree");

    let tree = repo.find_tree(tree_oid).expect("find tree");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Add Python library",
        &tree,
        &[&parent],
    )
    .expect("commit 2")
}

fn make_commit_3(repo: &Repository, parent_oid: Oid) -> Oid {
    let sig =
        Signature::new("Alice", "alice@test.com", &Time::new(1_705_910_400, 0)).expect("signature");

    let app_js = concat!(
        "const express = require('express');\n",
        "const app = express();\n",
        "\n",
        "app.get('/', (req, res) => {\n",
        "    res.send('Hello World');\n",
        "});\n",
        "\n",
        "app.get('/api', (req, res) => {\n",
        "    res.json({ status: 'ok' });\n",
        "});\n",
        "\n",
        "app.listen(3000);\n",
    );

    let parent = repo.find_commit(parent_oid).expect("find parent");
    let parent_tree = parent.tree().expect("parent tree");

    let app_js_blob = repo.blob(app_js.as_bytes()).expect("blob app.js");
    let mut root_tb = repo
        .treebuilder(Some(&parent_tree))
        .expect("treebuilder root");
    root_tb
        .insert("app.js", app_js_blob, 0o100644)
        .expect("insert app.js");
    let tree_oid = root_tb.write().expect("write root tree");

    let tree = repo.find_tree(tree_oid).expect("find tree");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Add JavaScript app\n\nCo-authored-by: Charlie <charlie@test.com>",
        &tree,
        &[&parent],
    )
    .expect("commit 3")
}

fn make_commit_4(repo: &Repository, parent_oid: Oid) -> Oid {
    let sig =
        Signature::new("Bob", "bob@test.com", &Time::new(1_706_785_200, 0)).expect("signature");

    let main_rs = concat!(
        "fn main() {\n",
        "    println!(\"Hello, logit!\");\n",
        "}\n",
        "\n",
        "fn add(a: i32, b: i32) -> i32 {\n",
        "    a + b\n",
        "}\n",
        "\n",
        "fn mul(a: i32, b: i32) -> i32 {\n",
        "    a * b\n",
        "}\n",
        "\n",
        "fn div(a: i32, b: i32) -> i32 {\n",
        "    a / b\n",
        "}\n",
        "\n",
        "fn modulo(a: i32, b: i32) -> i32 {\n",
        "    a % b\n",
        "}\n",
    );

    let lib_py = concat!(
        "def hello():\n",
        "    print(\"hello\")\n",
        "\n",
        "def greet(name):\n",
        "    print(f\"hello {name}\")\n",
        "\n",
        "def farewell():\n",
        "    print(\"goodbye\")\n",
        "\n",
        "def greet_all(names):\n",
        "    for name in names:\n",
        "        greet(name)\n",
    );

    let style_css = concat!(
        "body {\n",
        "    margin: 0;\n",
        "    padding: 0;\n",
        "    font-family: sans-serif;\n",
        "}\n",
        "\n",
        "h1 {\n",
        "    color: #333;\n",
        "    font-size: 2em;\n",
        "}\n",
        "\n",
        "p {\n",
        "    color: #666;\n",
        "    line-height: 1.6;\n",
        "}\n",
        "\n",
        ".container {\n",
        "    max-width: 960px;\n",
        "    margin: 0 auto;\n",
        "}\n",
    );

    let parent = repo.find_commit(parent_oid).expect("find parent");
    let parent_tree = parent.tree().expect("parent tree");
    let parent_src_oid = parent_tree.get_name("src").expect("src entry").id();
    let parent_src_tree = repo.find_tree(parent_src_oid).expect("parent src tree");

    let main_rs_blob = repo.blob(main_rs.as_bytes()).expect("blob main.rs");
    let lib_py_blob = repo.blob(lib_py.as_bytes()).expect("blob lib.py");
    let style_css_blob = repo.blob(style_css.as_bytes()).expect("blob style.css");

    let mut src_tb = repo
        .treebuilder(Some(&parent_src_tree))
        .expect("treebuilder src");
    src_tb
        .insert("main.rs", main_rs_blob, 0o100644)
        .expect("insert main.rs");
    src_tb
        .insert("lib.py", lib_py_blob, 0o100644)
        .expect("insert lib.py");
    let src_oid = src_tb.write().expect("write src tree");

    let mut root_tb = repo
        .treebuilder(Some(&parent_tree))
        .expect("treebuilder root");
    root_tb
        .insert("src", src_oid, 0o040000)
        .expect("insert src");
    root_tb
        .insert("style.css", style_css_blob, 0o100644)
        .expect("insert style.css");
    let tree_oid = root_tb.write().expect("write root tree");

    let tree = repo.find_tree(tree_oid).expect("find tree");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Update multiple languages",
        &tree,
        &[&parent],
    )
    .expect("commit 4")
}

fn make_commit_5(repo: &Repository, parent_oid: Oid) -> Oid {
    let sig =
        Signature::new("Alice", "alice@test.com", &Time::new(1_708_012_800, 0)).expect("signature");

    let readme = concat!(
        "# Test Project\n",
        "\n",
        "A project for testing logit.\n",
        "\n",
    );

    let parent = repo.find_commit(parent_oid).expect("find parent");
    let parent_tree = parent.tree().expect("parent tree");

    let readme_blob = repo.blob(readme.as_bytes()).expect("blob README.md");
    let mut root_tb = repo
        .treebuilder(Some(&parent_tree))
        .expect("treebuilder root");
    root_tb.remove("app.js").expect("remove app.js");
    root_tb
        .insert("README.md", readme_blob, 0o100644)
        .expect("insert README.md");
    let tree_oid = root_tb.write().expect("write root tree");

    let tree = repo.find_tree(tree_oid).expect("find tree");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Remove app.js, update README",
        &tree,
        &[&parent],
    )
    .expect("commit 5")
}
