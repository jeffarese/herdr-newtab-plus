//! The places the version lives must agree (the release tag is checked
//! against them, and install.sh downloads the release named by the manifest),
//! and the manifest has the keys Herdr needs. Ported from CI's "Manifest
//! parses" step of the Python release; the popup-size checks of that step are
//! unit tests, in src/tests/manifest.rs, as they need the layout code.

use std::path::Path;

fn read(path: &str) -> String {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("read {path}: {e}"));
    text.replace('\r', "")
}

/// The quoted value of the first line that starts with `prefix` and then
/// optional spaces and `=`.
fn quoted_value(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(prefix)?.trim_start().strip_prefix('=')?.trim();
        let inner = rest.strip_prefix('"')?;
        Some(inner[..inner.find('"')?].to_string())
    })
}

/// The lines of the first `[[name]]` table, up to the next table header.
fn first_table(text: &str, name: &str) -> String {
    let header = format!("[[{name}]]");
    let mut lines = text.lines().skip_while(|l| l.trim() != header);
    assert!(lines.next().is_some(), "no {header} in herdr-plugin.toml");
    lines.take_while(|l| !l.trim_start().starts_with('[')).collect::<Vec<_>>().join("\n")
}

#[test]
fn every_source_agrees() {
    let lock = read("Cargo.lock");
    let package = lock.split("[[package]]").find(|p| p.contains("name = \"herdr-newtab-plus\"")).unwrap_or("");
    let found = [
        ("herdr-plugin.toml", quoted_value(&read("herdr-plugin.toml"), "version")),
        ("Cargo.toml", quoted_value(&read("Cargo.toml"), "version")),
        ("Cargo.lock", quoted_value(package.trim(), "version")),
    ];
    assert!(found.iter().all(|(_, v)| v.is_some()), "{found:?}");
    assert!(found.iter().all(|(_, v)| *v == found[0].1), "{found:?}");
    // --version, and the version install.sh checks a download against, come
    // from Cargo.toml rather than from a constant of their own.
    assert!(read("src/cli.rs").contains("pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");"));
    assert_eq!(found[1].1.as_deref(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn manifest_parses() {
    let manifest = read("herdr-plugin.toml");
    // Top-level keys: the part before the first table header.
    let top: String = manifest.lines().take_while(|l| !l.trim_start().starts_with('[')).collect::<Vec<_>>().join("\n");
    for key in ["id", "name", "version", "min_herdr_version"] {
        let value = quoted_value(&top, key);
        assert!(value.is_some_and(|v| !v.is_empty()), "manifest is missing {key}");
    }
    assert_eq!(quoted_value(&top, "id").as_deref(), Some("herdr-newtab-plus"));
    let pane = first_table(&manifest, "panes");
    assert_eq!(quoted_value(&pane, "placement").as_deref(), Some("popup"));
    assert!(pane.contains("command = [\"bin/herdr-newtab-plus\"]"), "{pane}");
    assert_eq!(quoted_value(&first_table(&manifest, "actions"), "id").as_deref(), Some("open"));
    let build = first_table(&manifest, "build");
    assert!(build.contains("command = [\"sh\", \"scripts/install.sh\"]"), "{build}");
}

#[test]
fn install_script_is_where_the_manifest_says() {
    let script = read("scripts/install.sh");
    assert!(script.starts_with("#!/bin/sh\n"));
    assert!(script.contains("TARGET=\"$BIN_DIR/herdr-newtab-plus\""));
    assert!(script.contains("REPO_URL=\"https://github.com/jeffarese/herdr-newtab-plus\""));
}
