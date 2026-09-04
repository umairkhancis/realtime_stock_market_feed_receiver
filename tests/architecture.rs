//! The dependency rule, enforced.
//!
//! Rust modules do **not** enforce acyclicity: `crate::domain` can name
//! `crate::presentation` and the compiler will happily agree. Directory layout
//! alone is therefore a convention, not an architecture — it documents intent
//! and does nothing to preserve it. The only two ways to make the rule real are
//! to split the layers into separate crates in a workspace (Cargo *does* refuse
//! a dependency cycle between crates) or to assert it in a test. This is the
//! second, and it is here because it is far cheaper than the first at this size;
//! `docs/clean_arch.md` records when the workspace becomes worth it.
//!
//! Comment lines are stripped before checking, so a doc link that points outward
//! — `domain::loss::detect` referring the reader to
//! `presentation::report::blind_spot_note`, for instance — is prose, not a
//! dependency, and is allowed.

use std::fs;
use std::path::{Path, PathBuf};

/// A ring, and what it may not name. Order is inner to outer.
struct Layer {
    /// Path prefix under `src/`, without extension.
    name: &'static str,
    /// Substrings that must not appear in this layer's code.
    forbidden: &'static [(&'static str, &'static str)],
}

/// `presentation` is the outermost ring and constrains nothing: everything it
/// could reach is already inside it. `infrastructure` and `presentation` are
/// siblings, so neither may name the other.
const LAYERS: &[Layer] = &[
    Layer {
        name: "domain",
        forbidden: &[
            (
                "crate::application",
                "the domain must not know a use case exists",
            ),
            (
                "crate::infrastructure",
                "the domain must not know an adapter exists",
            ),
            (
                "crate::presentation",
                "the domain must not know a terminal exists",
            ),
            (
                "std::io",
                "a domain rule that does I/O is not a domain rule",
            ),
            (
                "std::fs",
                "a domain rule that touches a filesystem is not a domain rule",
            ),
            (
                "std::net",
                "a domain rule that opens a socket is not a domain rule",
            ),
            ("std::thread", "the domain has no threads to block"),
            (
                "std::time",
                "loss is inferred from message timestamps, never from a wall clock",
            ),
            ("println!", "the domain does not have an audience"),
            ("colored", "third-party crates belong in the outermost ring"),
            (
                "figlet_rs",
                "third-party crates belong in the outermost ring",
            ),
        ],
    },
    Layer {
        name: "application",
        forbidden: &[
            (
                "crate::infrastructure",
                "use cases depend on ports, never on adapters",
            ),
            (
                "crate::presentation",
                "use cases must not know how they are rendered",
            ),
            (
                "std::fs",
                "a use case that opens a file has swallowed its adapter",
            ),
            (
                "std::net",
                "a use case that opens a socket has swallowed its adapter",
            ),
            ("println!", "output belongs behind the observer port"),
            ("colored", "third-party crates belong in the outermost ring"),
            (
                "figlet_rs",
                "third-party crates belong in the outermost ring",
            ),
        ],
    },
    Layer {
        name: "infrastructure",
        forbidden: &[
            (
                "crate::presentation",
                "an adapter must not reach across to the UI",
            ),
            // The receiver's own version of the rule, and the one with teeth:
            // a `println!` inside the capture loop makes the terminal emulator
            // the bottleneck and the resulting socket-buffer overflow looks
            // exactly like network loss.
            (
                "println!",
                "an adapter that prints is an adapter that can be the bottleneck",
            ),
            ("colored", "third-party crates belong in the outermost ring"),
            (
                "figlet_rs",
                "third-party crates belong in the outermost ring",
            ),
        ],
    },
];

/// The single exemption, and it is the point of the module: `banner` is where
/// the crate's only third-party dependencies are quarantined.
const THIRD_PARTY_HOME: &str = "presentation/banner.rs";

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file at or under `src/<name>`, plus `src/<name>.rs` itself.
fn files_in_layer(name: &str) -> Vec<PathBuf> {
    let mut out = vec![src_dir().join(format!("{name}.rs"))];
    collect(&src_dir().join(name), &mut out);
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Drops comment lines: the rule constrains code, not prose.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn dependencies_point_inward() {
    let root = src_dir();
    let mut violations = Vec::new();

    for layer in LAYERS {
        for path in files_in_layer(layer.name) {
            let relative = path.strip_prefix(&root).unwrap().display().to_string();
            let code = code_only(&fs::read_to_string(&path).expect("layer file is readable"));
            for (needle, why) in layer.forbidden {
                if code.contains(needle) {
                    violations.push(format!("  {relative}: names `{needle}` — {why}"));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the dependency rule is broken in {} place(s):\n{}",
        violations.len(),
        violations.join("\n"),
    );
}

#[test]
fn third_party_crates_stay_in_one_file() {
    let root = src_dir();
    let mut homes = Vec::new();
    let mut all = vec![root.join("lib.rs"), root.join("main.rs")];
    collect(&root, &mut all);

    for path in all {
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let code = code_only(&source);
        if code.contains("colored") || code.contains("figlet_rs") {
            homes.push(path.strip_prefix(&root).unwrap().display().to_string());
        }
    }
    homes.sort();
    homes.dedup();

    assert_eq!(
        homes,
        [THIRD_PARTY_HOME],
        "`colored` and `figlet_rs` must be reachable from {THIRD_PARTY_HOME} alone, \
         so that deleting that one file deletes both dependencies"
    );
}

/// The rings are the only top-level modules. A new file dropped straight into
/// `src/` belongs to no layer and is therefore governed by nothing.
#[test]
fn every_module_belongs_to_a_ring() {
    let expected = [
        "application",
        "domain",
        "infrastructure",
        "lib",
        "main",
        "presentation",
    ];
    let mut found: Vec<String> = fs::read_dir(src_dir())
        .expect("src is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    found.sort();

    assert_eq!(found, expected, "an unlayered module appeared in src/");
}

/// The wire contract is shared with the transmitter, so the two codecs must
/// stay identical. The receiver's copy may not drift into naming anything the
/// transmitter's does not have.
#[test]
fn the_codec_and_the_message_records_name_nothing_outside_the_domain() {
    for file in ["domain/codec.rs", "domain/message.rs"] {
        let code = code_only(&fs::read_to_string(src_dir().join(file)).unwrap());
        for needle in ["crate::application", "crate::infrastructure", "crate::presentation"] {
            assert!(
                !code.contains(needle),
                "{file} names `{needle}`; it is byte-shared with the transmitter and \
                 must depend on nothing but `crate::domain`"
            );
        }
    }
}
