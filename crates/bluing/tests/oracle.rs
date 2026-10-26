//! Runs every fixture in `tests/fixtures` through Bluing and through rustc.
//!
//! Each fixture states what both compilers must do. On top of that, the
//! subset rule is checked directly: a fixture Bluing accepts must also be
//! accepted by rustc. rustc must be on `PATH`; the test fails without it.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use support::{Fixture, check_files, codes, render};

#[test]
fn fixtures_agree_with_bluing_and_rustc() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read tests/fixtures")
        .map(|entry| entry.expect("read a fixture entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "blu"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures found in {}", dir.display());

    let mut failures = Vec::new();
    for path in &paths {
        let name = path
            .file_stem()
            .expect("fixture has a name")
            .to_string_lossy();
        let text = fs::read_to_string(path).expect("read fixture");
        let fixture = match Fixture::parse(&text) {
            Ok(fixture) => fixture,
            Err(err) => {
                failures.push(format!("{name}: bad fixture: {err}"));
                continue;
            }
        };
        if fixture.bluing.is_empty() && !fixture.rustc_ok {
            failures.push(format!(
                "{name}: Bluing accepts a program rustc rejects, which breaks the subset rule"
            ));
        }
        let checked = check_files(&fixture.file_refs());
        let actual = codes(&checked);
        if actual != fixture.bluing {
            failures.push(format!(
                "{name}: Bluing reported {actual:?}, expected {:?}\n{}",
                fixture.bluing,
                render(&checked)
            ));
        }
        match run_rustc(&name, &fixture) {
            Ok(rustc) if rustc.ok == fixture.rustc_ok => {}
            Ok(rustc) => failures.push(format!(
                "{name}: rustc {}, but the fixture says it should {}{}",
                if rustc.ok {
                    "accepted it"
                } else {
                    "rejected it"
                },
                if fixture.rustc_ok { "accept" } else { "reject" },
                rustc
                    .first_error
                    .map_or_else(String::new, |error| format!("\n  rustc: {error}")),
            )),
            Err(err) => failures.push(format!("{name}: {err}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

struct RustcOutcome {
    ok: bool,
    /// The first `error` line rustc printed, if it rejected the crate.
    first_error: Option<String>,
}

/// Compiles the fixture as a Rust library: the same files with `.rs` in
/// place of `.blu`.
fn run_rustc(name: &str, fixture: &Fixture) -> Result<RustcOutcome, String> {
    let dir = std::env::temp_dir().join(format!("bluing-oracle-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for (path, text) in &fixture.files {
        let path = dir
            .join(path.strip_suffix(".blu").unwrap_or(path))
            .with_extension("rs");
        fs::create_dir_all(path.parent().expect("fixture files are in a directory"))
            .map_err(|err| format!("create {}: {err}", dir.display()))?;
        fs::write(&path, text).map_err(|err| format!("write {}: {err}", path.display()))?;
    }
    let root = Path::new(&fixture.files[0].0).with_extension("rs");
    let output = Command::new("rustc")
        .args([
            "--edition",
            "2024",
            "--crate-type",
            "lib",
            "--emit=metadata",
            // Bluing targets wasm32, where `usize` is 32 bits.
            "--target",
            "wasm32-unknown-unknown",
            "-A",
            "warnings",
        ])
        .arg("--out-dir")
        .arg(&dir)
        .arg(dir.join(root))
        .output()
        .map_err(|err| format!("could not run rustc (is it on PATH?): {err}"))?;
    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(RustcOutcome {
        ok: output.status.success(),
        first_error: stderr
            .lines()
            .find(|line| line.starts_with("error"))
            .map(str::to_owned),
    })
}
