use std::fs;

#[test]
fn fixture_shape_is_documented() {
    // This test intentionally avoids reaching into private bootstrap internals.
    // It protects the repository fixture snippets from accidental deletion.
    let cargo = fs::read_to_string("examples/workspace-Cargo.toml").unwrap();
    let config = fs::read_to_string("examples/workspace-config.toml").unwrap();
    assert!(cargo.contains("workspace.metadata.orphan-gc"));
    // Shadow is the install default (R748-F6); a fixture that showed the tool
    // deleting on first install would be teaching the wrong thing.
    assert!(cargo.contains("dry-run = true"), "{cargo}");
    assert!(config.contains("rustc-wrapper = \"cargo-orphan-gc\""));
    // The inner slot is the arrangement that silently collapses a chained
    // cache; the fixture must never regress to it.
    assert!(!config.contains("rustc-workspace-wrapper = "));
}
