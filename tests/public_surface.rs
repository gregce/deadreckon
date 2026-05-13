use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const LIB_CRATES: &[(&str, &str)] = &[
    ("deadreckon-core", "crates/deadreckon-core/src/lib.rs"),
    (
        "deadreckon-providers",
        "crates/deadreckon-providers/src/lib.rs",
    ),
    (
        "deadreckon-runtime",
        "crates/deadreckon-runtime/src/lib.rs",
    ),
    (
        "deadreckon-sandbox",
        "crates/deadreckon-sandbox/src/lib.rs",
    ),
];

#[test]
fn public_surface_set_matches_pre_rider_head() {
    let root = workspace_root();
    let expected = parse_baseline(&root.join("tests/.public-surface-baseline"));
    let actual = current_public_surface(&root);
    assert_eq!(
        expected, actual,
        "public re-export set drifted; this rider may only regroup lib.rs exports"
    );
}

#[test]
fn public_surface_baseline_lists_all_four_library_crates() {
    let root = workspace_root();
    let baseline = parse_baseline(&root.join("tests/.public-surface-baseline"));
    let crates = baseline.keys().cloned().collect::<BTreeSet<_>>();
    let expected = LIB_CRATES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, crates);
}

pub fn current_public_surface(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    LIB_CRATES
        .iter()
        .map(|(name, rel)| {
            (
                (*name).to_string(),
                collect_pub_uses(&root.join(rel))
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect()
}

fn parse_baseline(path: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let text = fs::read_to_string(path).expect("read public surface baseline");
    let mut out = BTreeMap::<String, BTreeSet<String>>::new();
    let mut current = None::<String>;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("crate: ") {
            let name = name.to_string();
            out.entry(name.clone()).or_default();
            current = Some(name);
        } else if let Some(symbol) = line.strip_prefix("  ") {
            let crate_name = current
                .as_ref()
                .expect("symbol line must follow a crate header");
            out.entry(crate_name.clone())
                .or_default()
                .insert(symbol.to_string());
        } else if !line.trim().is_empty() {
            panic!("unrecognized baseline line: {line}");
        }
    }
    out
}

fn collect_pub_uses(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).expect("read lib.rs");
    let mut statements = Vec::new();
    let mut current = Vec::new();
    let mut in_statement = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !in_statement && trimmed.starts_with("pub use ") {
            current.push(trimmed.to_string());
            in_statement = true;
        } else if in_statement {
            current.push(trimmed.to_string());
        }
        if in_statement && trimmed.ends_with(';') {
            statements.push(current.join(" "));
            current.clear();
            in_statement = false;
        }
    }
    statements
        .iter()
        .flat_map(|statement| expand_pub_use(statement))
        .collect()
}

fn expand_pub_use(statement: &str) -> Vec<String> {
    let body = statement
        .trim()
        .strip_prefix("pub use ")
        .expect("pub use statement")
        .trim_end_matches(';')
        .trim();
    let Some((prefix, rest)) = body.split_once('{') else {
        return vec![body.to_string()];
    };
    let inner = rest
        .rsplit_once('}')
        .map(|(items, _)| items)
        .expect("grouped pub use closes with brace");
    let prefix = prefix.trim_end_matches(':');
    split_group_items(inner)
        .into_iter()
        .map(|item| format!("{prefix}::{item}"))
        .collect()
}

fn split_group_items(items: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in items.chars() {
        match ch {
            '{' => {
                depth += 1;
                current.push(ch);
            }
            '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let item = current.trim();
                if !item.is_empty() {
                    out.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let item = current.trim();
    if !item.is_empty() {
        out.push(item.to_string());
    }
    out
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("crates").is_dir() && dir.join("Cargo.toml").is_file() {
            return dir;
        }
        assert!(dir.pop(), "could not locate workspace root");
    }
}
