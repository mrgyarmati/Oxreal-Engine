//! Stable type IDs (ADR-0021 §2): `cargo xtask new-type-id` and `cargo xtask type-ids`.
//!
//! The scan reads every Rust source file of the workspace and collects the stable type IDs
//! written in it:
//!
//! - `#[reflect(id = "...")]` on a struct or enum;
//! - `StableTypeId::from_uuid("...")` in hand-written `Reflect` impls.
//!
//! It fails if two places declare the same ID (a copy-pasted attribute, caught even if the two
//! types are never registered together), or if a retired ID from
//! `docs/development/retired-type-ids.txt` appears again.
//!
//! It also keeps each crate's `stable-type-ids.txt` up to date: the IDs its non-generic derived
//! types declare, which a test in that crate checks its `register_types` registers.
//! `--write` regenerates the files; without it, a stale or missing file is an error.
//!
//! **Test code is skipped:** `tests/`, `benches/` and `examples/` directories, items marked
//! exactly `#[cfg(test)]` or `#[test]`, and out-of-line modules declared `#[cfg(test)] mod x;`.
//! Test fixtures may therefore reuse IDs. Items that macros generate are invisible to the scan.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Attribute, Expr, ExprCall, ExprLit, Item, ItemEnum, ItemStruct, Lit, Meta};

/// The checked-in list of retired IDs, relative to the workspace root.
const RETIRED: &str = "docs/development/retired-type-ids.txt";
/// The per-crate list of derived IDs, relative to the crate root.
const LISTING: &str = "stable-type-ids.txt";
/// Directories under the workspace root that hold crates.
const SOURCE_ROOTS: [&str; 3] = ["crates", "applications", "tools"];
/// Directories whose files are test, benchmark or example code.
const TEST_DIRS: [&str; 3] = ["tests", "benches", "examples"];

/// `cargo xtask new-type-id`: prints a random (version 4) UUID for `#[reflect(id = "...")]`.
pub(crate) fn new_type_id() -> Result<(), String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot read OS randomness: {e}"))?;
    println!("{}", uuid_v4(bytes));
    Ok(())
}

/// `cargo xtask type-ids [--write]`.
pub(crate) fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let write = match args {
        [] => false,
        [flag] if flag == "--write" => true,
        _ => return Err("usage: cargo xtask type-ids [--write]".to_owned()),
    };
    let scan = scan_workspace(root)?;
    let retired = read_retired(&root.join(RETIRED))?;
    check(&scan, &retired)?;
    sync_listings(root, &scan, write)?;
    println!(
        "type-ids: {} stable type IDs in {} files; no duplicates or retired IDs",
        scan.declarations.len(),
        scan.files
    );
    Ok(())
}

/// One place that declares a stable type ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Declaration {
    /// As written, lowercased.
    id: String,
    /// The type's name, or the constant a hand-written ID is assigned to.
    name: String,
    file: PathBuf,
    /// The crate root (the directory of the nearest `Cargo.toml`).
    krate: PathBuf,
    /// Declared by the derive on a non-generic type, so `register_types` must register it.
    registrable: bool,
}

#[derive(Debug, Default)]
struct Scan {
    declarations: Vec<Declaration>,
    files: usize,
}

fn scan_workspace(root: &Path) -> Result<Scan, String> {
    let mut files = Vec::new();
    for dir in SOURCE_ROOTS {
        collect_sources(&root.join(dir), &mut files)?;
    }
    files.sort();

    let mut parsed = Vec::with_capacity(files.len());
    for path in files {
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let file = syn::parse_file(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        parsed.push((path, file));
    }

    // Out-of-line `#[cfg(test)] mod x;` files, and their own submodules, are test code.
    let mut excluded = HashSet::new();
    let mut pending: Vec<PathBuf> = parsed
        .iter()
        .flat_map(|(path, file)| test_module_files(path, &file.items, false))
        .collect();
    while let Some(path) = pending.pop() {
        if excluded.insert(path.clone())
            && let Some((_, file)) = parsed.iter().find(|(p, _)| *p == path)
        {
            pending.extend(test_module_files(&path, &file.items, true));
        }
    }

    let mut scan = Scan::default();
    for (path, file) in &parsed {
        if excluded.contains(path) {
            continue;
        }
        let mut collector = Collector {
            file: path,
            krate: crate_root(path)?,
            out: &mut scan.declarations,
            errors: Vec::new(),
            constant: None,
        };
        collector.visit_file(file);
        if let Some(error) = collector.errors.into_iter().next() {
            return Err(error);
        }
        scan.files += 1;
    }
    Ok(scan)
}

/// Every `.rs` file under `dir`, skipping build output and test directories.
fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("failed to read {}: {e}", dir.display()))?
            .path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') && !TEST_DIRS.contains(&name) {
                collect_sources(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The files of out-of-line modules declared in `items` that are test code: marked
/// `#[cfg(test)]`, or all of them when the file itself is test code (`all`).
fn test_module_files(file: &Path, items: &[Item], all: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for item in items {
        let Item::Mod(module) = item else { continue };
        let test = all || is_test_code(&module.attrs);
        match &module.content {
            None if test => out.extend(module_file(file, &module.ident.to_string())),
            // Inline modules are skipped by the collector; their out-of-line children live in
            // a directory named after them, which this simple resolution doesn't follow.
            _ => {}
        }
    }
    out
}

/// The file of `mod name;` declared in `file`: `name.rs` or `name/mod.rs`, next to `file` if
/// it's a crate root or `mod.rs`, else in the directory named after `file`.
fn module_file(file: &Path, name: &str) -> Option<PathBuf> {
    let parent = file.parent()?;
    let stem = file.file_stem()?.to_str()?;
    let dir = if matches!(stem, "lib" | "main" | "mod") {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    };
    [
        dir.join(format!("{name}.rs")),
        dir.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// The directory of the nearest `Cargo.toml` above `file`.
fn crate_root(file: &Path) -> Result<PathBuf, String> {
    file.ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} isn't inside a crate", file.display()))
}

/// Exactly `#[cfg(test)]` or `#[test]`.
fn is_test_code(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| match &attr.meta {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => list.path.is_ident("cfg") && list.tokens.to_string() == "test",
        Meta::NameValue(_) => false,
    })
}

struct Collector<'a> {
    file: &'a Path,
    krate: PathBuf,
    out: &'a mut Vec<Declaration>,
    errors: Vec<String>,
    /// The constant being visited, naming hand-written IDs.
    constant: Option<String>,
}

impl Collector<'_> {
    fn declare(&mut self, id: &str, name: String, registrable: bool) {
        let normalized = id.to_ascii_lowercase();
        if !is_uuid(&normalized) {
            self.errors.push(format!(
                "{}: '{id}' (for {name}) is not a hyphenated UUID",
                self.file.display()
            ));
            return;
        }
        self.out.push(Declaration {
            id: normalized,
            name,
            file: self.file.to_path_buf(),
            krate: self.krate.clone(),
            registrable,
        });
    }

    fn derived(&mut self, attrs: &[Attribute], name: String, generic: bool) {
        for attr in attrs.iter().filter(|a| a.path().is_ident("reflect")) {
            let mut id = None;
            // Only `id = "..."` matters here; the derive validates everything else.
            let parsed = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    id = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                    Ok(())
                } else {
                    skip_meta(&meta)
                }
            });
            if let Err(error) = parsed {
                self.errors.push(format!(
                    "{}: #[reflect] on {name}: {error}",
                    self.file.display()
                ));
            }
            if let Some(id) = id {
                self.declare(&id, name.clone(), !generic);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if !is_test_code(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        self.derived(
            &item.attrs,
            item.ident.to_string(),
            !item.generics.params.is_empty(),
        );
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        self.derived(
            &item.attrs,
            item.ident.to_string(),
            !item.generics.params.is_empty(),
        );
        visit::visit_item_enum(self, item);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        let outer = self.constant.replace(item.ident.to_string());
        visit::visit_item_const(self, item);
        self.constant = outer;
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let Expr::Path(func) = &*call.func {
            let segments: Vec<String> = func
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if segments.ends_with(&["StableTypeId".to_owned(), "from_uuid".to_owned()])
                && let [
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(text),
                        ..
                    }),
                ] = call.args.iter().collect::<Vec<_>>()[..]
            {
                let name = self
                    .constant
                    .clone()
                    .unwrap_or_else(|| "a StableTypeId::from_uuid call".to_owned());
                self.declare(&text.value(), name, false);
            }
        }
        visit::visit_expr_call(self, call);
    }
}

/// Skips one nested meta item: `flag`, `key = value` or `key(...)`.
fn skip_meta(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _: Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_meta(&inner))?;
    }
    Ok(())
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Fails on duplicate and retired IDs, naming every place involved.
fn check(scan: &Scan, retired: &HashSet<String>) -> Result<(), String> {
    let mut by_id: BTreeMap<&str, Vec<&Declaration>> = BTreeMap::new();
    for declaration in &scan.declarations {
        by_id.entry(&declaration.id).or_default().push(declaration);
    }
    let mut problems = Vec::new();
    for (id, declarations) in &by_id {
        let places = || {
            declarations
                .iter()
                .map(|d| format!("{} ({})", d.name, d.file.display()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if declarations.len() > 1 {
            problems.push(format!(
                "stable type ID {id} is declared {} times: {}; give each type its own ID \
                 (`cargo xtask new-type-id`)",
                declarations.len(),
                places()
            ));
        }
        if retired.contains(*id) {
            problems.push(format!(
                "stable type ID {id} is retired ({RETIRED}) but used by {}; retired IDs are \
                 never reused, generate a new one with `cargo xtask new-type-id`",
                places()
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Reads the retired IDs: one UUID per line, then optional notes. `#` starts a comment line.
fn read_retired(path: &Path) -> Result<HashSet<String>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    parse_retired(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_retired(text: &str) -> Result<HashSet<String>, String> {
    let mut out = HashSet::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let id = line.split_whitespace().next().unwrap_or_default();
        let id = id.to_ascii_lowercase();
        if !is_uuid(&id) {
            return Err(format!(
                "line {}: '{id}' is not a hyphenated UUID",
                number + 1
            ));
        }
        out.insert(id);
    }
    Ok(out)
}

/// The expected `stable-type-ids.txt` of every crate that has derived IDs.
fn listings(scan: &Scan) -> BTreeMap<PathBuf, String> {
    let mut by_crate: BTreeMap<PathBuf, Vec<&Declaration>> = BTreeMap::new();
    for declaration in scan.declarations.iter().filter(|d| d.registrable) {
        by_crate
            .entry(declaration.krate.clone())
            .or_default()
            .push(declaration);
    }
    by_crate
        .into_iter()
        .map(|(krate, mut declarations)| {
            declarations.sort_by(|a, b| a.id.cmp(&b.id));
            let mut text = String::from(
                "# Stable type IDs declared by this crate's non-generic #[derive(Reflect)] \
                 types.\n# Generated by `cargo xtask type-ids --write`; a test checks that \
                 register_types registers each.\n",
            );
            for d in declarations {
                let _ = writeln!(text, "{} {}", d.id, d.name);
            }
            (krate, text)
        })
        .collect()
}

fn sync_listings(root: &Path, scan: &Scan, write: bool) -> Result<(), String> {
    let expected = listings(scan);
    let mut stale = Vec::new();
    let mut crate_roots = Vec::new();
    for dir in SOURCE_ROOTS {
        find_crate_roots(&root.join(dir), &mut crate_roots)?;
    }
    for krate in crate_roots {
        let path = krate.join(LISTING);
        let current = fs::read_to_string(&path).ok();
        let wanted = expected.get(&krate);
        if current.as_deref() == wanted.map(String::as_str) {
            continue;
        }
        if !write {
            stale.push(path.display().to_string());
            continue;
        }
        match wanted {
            Some(text) => fs::write(&path, text),
            None => fs::remove_file(&path),
        }
        .map_err(|e| format!("failed to update {}: {e}", path.display()))?;
        println!("type-ids: updated {}", path.display());
    }
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "out-of-date {LISTING} (run `cargo xtask type-ids --write` and commit the result): {}",
            stale.join(", ")
        ))
    }
}

/// Every directory with a `Cargo.toml` under `dir`.
fn find_crate_roots(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if dir.join("Cargo.toml").is_file() {
        out.push(dir.to_path_buf());
    }
    let entries =
        fs::read_dir(dir).map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("failed to read {}: {e}", dir.display()))?
            .path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() && name != "target" && name != "src" && !name.starts_with('.') {
            find_crate_roots(&path, out)?;
        }
    }
    Ok(())
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Formats random bytes as a version 4 UUID (RFC 9562): the version and variant bits are set.
fn uuid_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(&bytes)
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(36);
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04";
    const B: &str = "0b9e8d4c-7a61-4f3e-9c2d-5e8f1a7b6c30";

    fn collect(source: &str) -> Vec<Declaration> {
        let file = syn::parse_file(source).expect("valid Rust");
        let mut out = Vec::new();
        let mut collector = Collector {
            file: Path::new("lib.rs"),
            krate: PathBuf::from("crate"),
            out: &mut out,
            errors: Vec::new(),
            constant: None,
        };
        collector.visit_file(&file);
        assert!(collector.errors.is_empty(), "{:?}", collector.errors);
        out
    }

    #[test]
    fn derive_ids_and_hand_written_ids_are_found() {
        let found = collect(&format!(
            r#"
            #[derive(Reflect)]
            #[reflect(id = "{A}", clone, range(min = 0, max = 1), units = Metres)]
            struct Speed {{ value: f32 }}

            #[derive(Reflect)]
            #[reflect(id = "{}")]
            enum Mode<T> {{ A }}

            const ID: StableTypeId = StableTypeId::from_uuid("{B}");
            fn nested() {{
                #[derive(Reflect)]
                #[reflect(transient)]
                struct Local;
            }}
            "#,
            A.to_uppercase().replace("6F1C", "1111")
        ));
        let summary: Vec<_> = found
            .iter()
            .map(|d| (d.name.as_str(), d.registrable))
            .collect();
        assert_eq!(summary, [("Speed", true), ("Mode", false), ("ID", false)]);
        assert_eq!(found[0].id, A);
        assert_eq!(
            found[1].id, "11112e4a-93b0-4c1e-8d3f-2b7a9e5c1d04",
            "lowercased"
        );
    }

    #[test]
    fn test_code_is_skipped() {
        let found = collect(&format!(
            r#"
            #[cfg(test)]
            mod tests {{
                #[derive(Reflect)]
                #[reflect(id = "{A}")]
                struct Fixture;
            }}
            #[test]
            fn a_test() {{
                let _ = StableTypeId::from_uuid("{B}");
            }}
            #[cfg(not(test))]
            const KEPT: StableTypeId = StableTypeId::from_uuid("{B}");
            "#
        ));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "KEPT");
    }

    #[test]
    fn duplicates_and_retired_ids_are_errors_naming_every_place() {
        let mut scan = Scan {
            declarations: collect(&format!(
                r#"
                #[derive(Reflect)] #[reflect(id = "{A}")] struct First;
                #[derive(Reflect)] #[reflect(id = "{A}")] struct Second;
                "#
            )),
            files: 1,
        };
        let error = check(&scan, &HashSet::new()).expect_err("duplicate");
        assert!(
            error.contains("First") && error.contains("Second"),
            "{error}"
        );

        scan.declarations.pop();
        assert_eq!(check(&scan, &HashSet::new()), Ok(()));
        let retired = parse_retired(&format!("# comment\n\n{A} Old name, renamed 2026\n"))
            .expect("valid list");
        let error = check(&scan, &retired).expect_err("retired");
        assert!(
            error.contains("retired") && error.contains("First"),
            "{error}"
        );
        assert!(parse_retired("not-a-uuid").is_err());
    }

    #[test]
    fn listings_hold_only_registrable_ids_per_crate_in_id_order() {
        let scan = Scan {
            declarations: collect(&format!(
                r#"
                #[derive(Reflect)] #[reflect(id = "{A}")] struct Later;
                #[derive(Reflect)] #[reflect(id = "{B}")] struct Earlier;
                #[derive(Reflect)] #[reflect(id = "{}")] struct Generic<T>(T);
                const HAND: StableTypeId = StableTypeId::from_uuid("{}");
                "#,
                A.replace('6', "7"),
                B.replace('0', "1")
            )),
            files: 1,
        };
        let listings = listings(&scan);
        let text = &listings[&PathBuf::from("crate")];
        let lines: Vec<_> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines, [format!("{B} Earlier"), format!("{A} Later")]);
    }

    #[test]
    fn test_modules_resolve_to_their_files() {
        let dir = std::env::temp_dir().join(format!("xtask-type-ids-{}", std::process::id()));
        fs::create_dir_all(dir.join("src/storage")).expect("temp dir");
        fs::write(dir.join("src/tests.rs"), "").expect("write");
        fs::write(dir.join("src/storage/tests.rs"), "").expect("write");
        let lib = dir.join("src/lib.rs");
        let storage = dir.join("src/storage.rs");
        assert_eq!(module_file(&lib, "tests"), Some(dir.join("src/tests.rs")));
        assert_eq!(
            module_file(&storage, "tests"),
            Some(dir.join("src/storage/tests.rs"))
        );
        assert_eq!(module_file(&lib, "missing"), None);
        let items = syn::parse_file("#[cfg(test)] mod tests; mod storage;")
            .expect("valid")
            .items;
        assert_eq!(
            test_module_files(&lib, &items, false),
            [dir.join("src/tests.rs")]
        );
        fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn generated_ids_are_version_4_uuids() {
        assert_eq!(uuid_v4([0xff; 16]), "ffffffff-ffff-4fff-bfff-ffffffffffff");
        assert_eq!(uuid_v4([0x00; 16]), "00000000-0000-4000-8000-000000000000");
        assert!(is_uuid(&uuid_v4([0xab; 16])));
        assert!(!is_uuid("abababab-abab-abab-abab-ababababab"));
    }
}
