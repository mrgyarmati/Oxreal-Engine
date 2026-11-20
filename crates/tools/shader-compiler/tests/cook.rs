//! Cooking real shaders with the pinned `slangc` and `spirv-val` (ADR-0019 §4).
//!
//! These need Slang and `spirv-val`, so they're ignored by default. CI runs them with
//! `OXREAL_REQUIRE_SPIRV_VAL=1` on every platform, so a missing validator fails there instead of
//! skipping. Run them by hand with
//! `cargo xtask slang install && cargo test -p oxreal-shader-compiler --test cook -- --ignored`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use oxreal_shader_compiler::cook::{
    CookError, CookRequest, MANIFEST_FILE, Manifest, cook, read_recipe, source_key,
};
use oxreal_shader_compiler::{
    Optimization, ShaderError, SlangCompiler, SpirvValidator, pinned_version,
};

/// A copy of `tests/cook` in a fresh temporary directory, removed when dropped.
struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "oxreal-cook-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cook");
        for file in [
            "shaders/cook.json",
            "shaders/Tint.slang",
            "common/Palette.slang",
        ] {
            let to = root.join(file);
            std::fs::create_dir_all(to.parent().expect("a file has a parent"))
                .expect("creating the temporary workspace");
            std::fs::copy(fixture.join(file), to).expect("copying the fixture");
        }
        Self { root }
    }

    fn recipe(&self) -> PathBuf {
        self.root.join("shaders/cook.json")
    }

    fn out_dir(&self) -> PathBuf {
        self.root.join("target/cooked/shaders")
    }

    fn edit(&self, file: &str, from: &str, to: &str) {
        let path = self.root.join(file);
        let text = std::fs::read_to_string(&path).expect("reading a workspace file");
        assert!(text.contains(from), "{file} lacks `{from}`");
        std::fs::write(&path, text.replace(from, to)).expect("writing a workspace file");
    }

    fn cook(&self, optimization: Optimization) -> Result<Vec<Vec<String>>, CookError> {
        let recipes = [self.recipe()];
        let out_dir = self.out_dir();
        let report = cook(
            &compiler(),
            &validator().expect("checked by the test"),
            &CookRequest {
                workspace: &self.root,
                recipes: &recipes,
                out_dir: &out_dir,
                optimization,
            },
        )?;
        Ok(vec![report.cooked, report.unchanged, report.removed])
    }

    fn manifest(&self) -> Manifest {
        let text = std::fs::read_to_string(self.out_dir().join(MANIFEST_FILE))
            .expect("the cook wrote a manifest");
        Manifest::from_previous(&text).expect("a manifest this crate wrote")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn compiler() -> SlangCompiler {
    SlangCompiler::locate().expect("Slang is installed (cargo xtask slang install)")
}

/// The validator, or `None` (with a note) where it isn't installed and isn't required.
#[expect(
    clippy::print_stderr,
    reason = "a skipped test says why, like the other shader tests"
)]
fn validator() -> Option<SpirvValidator> {
    match SpirvValidator::locate() {
        Ok(validator) => Some(validator),
        Err(ShaderError::ValidatorNotFound) => {
            let required = std::env::var("OXREAL_REQUIRE_SPIRV_VAL").is_ok_and(|v| v == "1");
            assert!(
                !required,
                "OXREAL_REQUIRE_SPIRV_VAL=1 but spirv-val was not found (VULKAN_SDK or PATH)"
            );
            eprintln!("note: spirv-val not found; cooking needs it, so this test is skipped");
            None
        }
        Err(other) => panic!("spirv-val is unusable: {other}"),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn cooks_recooks_only_what_changed_and_records_it() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();

    // First cook: everything is compiled.
    let report = ws.cook(Optimization::Debug).unwrap();
    assert_eq!(report, [names(&["Tint"]), vec![], vec![]]);
    let manifest = ws.manifest();
    assert_eq!(manifest.slang_version, pinned_version());
    assert_eq!(manifest.spirv_val_version, validator().unwrap().version());
    let tint = &manifest.shaders[0];
    assert_eq!(tint.file, "Tint.spv");
    assert_eq!(tint.entry_points, ["tintVertex", "tintPixel"]);
    let entry = &read_recipe(&ws.recipe()).unwrap()[0];
    let expected_key = source_key(&ws.root, &pinned_version(), entry).unwrap();
    assert_eq!(tint.source_key, expected_key.to_string());
    let spirv = std::fs::read(ws.out_dir().join("Tint.spv")).unwrap();
    assert_eq!(&spirv[..4], &0x0723_0203_u32.to_le_bytes(), "SPIR-V magic");

    // Nothing changed: nothing is compiled, and the manifest is byte-identical.
    let before = std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap();
    assert_eq!(
        ws.cook(Optimization::Debug).unwrap(),
        [vec![], names(&["Tint"]), vec![]]
    );
    assert_eq!(
        std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap(),
        before
    );

    // Editing an imported module changes the key and recooks.
    ws.edit(
        "common/Palette.slang",
        "float3(1.0, 0.2, 0.2)",
        "float3(0.9, 0.2, 0.2)",
    );
    assert_eq!(
        ws.cook(Optimization::Debug).unwrap(),
        [names(&["Tint"]), vec![], vec![]]
    );
    assert_ne!(ws.manifest().shaders[0].source_key, tint.source_key);

    // So does another optimization level, which is recorded.
    assert_eq!(
        ws.cook(Optimization::Release).unwrap(),
        [names(&["Tint"]), vec![], vec![]]
    );
    assert_eq!(ws.manifest().shaders[0].optimization, Optimization::Release);

    // A damaged file is recooked, never trusted.
    std::fs::write(ws.out_dir().join("Tint.spv"), b"garbage").unwrap();
    assert_eq!(
        ws.cook(Optimization::Release).unwrap(),
        [names(&["Tint"]), vec![], vec![]]
    );
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn an_undeclared_import_fails_and_leaves_the_output_untouched() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    ws.cook(Optimization::Debug).unwrap();
    let before = std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap();
    let spirv_before = std::fs::read(ws.out_dir().join("Tint.spv")).unwrap();

    // Tint now imports a module its recipe doesn't declare.
    std::fs::write(
        ws.root.join("common/Extra.slang"),
        "module Extra;\npublic float extraValue() { return 0.5; }\n",
    )
    .unwrap();
    ws.edit(
        "shaders/Tint.slang",
        "import Palette;",
        "import Palette;\nimport Extra;",
    );
    ws.edit(
        "shaders/Tint.slang",
        "return float4(input.color, 1.0);",
        "return float4(input.color, extraValue());",
    );

    match ws.cook(Optimization::Debug).unwrap_err() {
        CookError::UndeclaredDependency {
            output, dependency, ..
        } => {
            assert_eq!(output, "Tint");
            assert_eq!(dependency, "common/Extra.slang");
        }
        other => panic!("expected UndeclaredDependency, got {other}"),
    }
    assert_eq!(
        std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(ws.out_dir().join("Tint.spv")).unwrap(),
        spirv_before
    );

    // Declaring it fixes the cook.
    ws.edit(
        "shaders/cook.json",
        "\"common/Palette.slang\"]",
        "\"common/Palette.slang\", \"common/Extra.slang\"]",
    );
    assert_eq!(
        ws.cook(Optimization::Debug).unwrap(),
        [names(&["Tint"]), vec![], vec![]]
    );
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn a_shader_no_recipe_lists_is_removed() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    ws.cook(Optimization::Debug).unwrap();
    // The same shader under a new name: the old file goes, the new one arrives.
    ws.edit(
        "shaders/cook.json",
        "\"output\": \"Tint\"",
        "\"output\": \"Tinted\"",
    );
    assert_eq!(
        ws.cook(Optimization::Debug).unwrap(),
        [names(&["Tinted"]), vec![], names(&["Tint"])]
    );
    assert!(!ws.out_dir().join("Tint.spv").exists());
    assert!(ws.out_dir().join("Tinted.spv").is_file());
    assert_eq!(ws.manifest().shaders.len(), 1);
}

/// A second recipe entry, compiled from `shaders/<source>`.
fn add_entry(ws: &Workspace, output: &str, source: &str) {
    ws.edit(
        "shaders/cook.json",
        "  \"shaders\": [\n",
        &format!(
            "  \"shaders\": [\n    {{ \"output\": \"{output}\", \"source\": \"shaders/{source}\", \
             \"entry_points\": [\"tintVertex\"], \"include_dirs\": [\"common\"], \
             \"inputs\": [\"shaders/{source}\", \"common/Palette.slang\"] }},\n"
        ),
    );
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn a_failing_shader_leaves_every_file_untouched_even_after_others_recompiled() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    ws.cook(Optimization::Debug).unwrap();
    let manifest = std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap();
    let spirv = std::fs::read(ws.out_dir().join("Tint.spv")).unwrap();

    // Tint must recompile (its import changed), and a second shader, sorted after it, is broken.
    ws.edit(
        "common/Palette.slang",
        "float3(1.0, 0.2, 0.2)",
        "float3(0.8, 0.2, 0.2)",
    );
    std::fs::write(
        ws.root.join("shaders/Zbroken.slang"),
        "import Palette;\n[shader(\"vertex\")] float4 tintVertex() : SV_Position { return undefinedSymbol; }\n",
    )
    .unwrap();
    add_entry(&ws, "Zbroken", "Zbroken.slang");

    match ws.cook(Optimization::Debug).unwrap_err() {
        CookError::Shader { output, .. } => assert_eq!(output, "Zbroken"),
        other => panic!("expected a shader error, got {other}"),
    }
    assert_eq!(
        std::fs::read(ws.out_dir().join(MANIFEST_FILE)).unwrap(),
        manifest
    );
    assert_eq!(std::fs::read(ws.out_dir().join("Tint.spv")).unwrap(), spirv);
    let leftovers: Vec<_> = std::fs::read_dir(ws.out_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp") || name.starts_with("Zbroken"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn outputs_differing_only_in_case_are_refused_on_every_platform() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    add_entry(&ws, "tint", "Tint.slang");
    match ws.cook(Optimization::Debug).unwrap_err() {
        CookError::DuplicateOutput { name, .. } => {
            assert!(name.eq_ignore_ascii_case("tint"), "{name}");
        }
        other => panic!("expected DuplicateOutput, got {other}"),
    }
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn renaming_a_shader_only_in_case_keeps_its_new_file() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    ws.cook(Optimization::Debug).unwrap();
    ws.edit(
        "shaders/cook.json",
        "\"output\": \"Tint\"",
        "\"output\": \"tint\"",
    );
    assert_eq!(
        ws.cook(Optimization::Debug).unwrap(),
        [names(&["tint"]), vec![], names(&["Tint"])]
    );
    // On Windows Tint.spv and tint.spv are one file: removing the old name first keeps it.
    let files: Vec<String> = std::fs::read_dir(ws.out_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".spv"))
        .collect();
    assert_eq!(files, ["tint.spv"]);
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn a_hostile_previous_manifest_never_removes_files_outside_the_directory() {
    if validator().is_none() {
        return;
    }
    let ws = Workspace::new();
    ws.cook(Optimization::Debug).unwrap();
    let victim = ws.root.join("target/cooked/victim.spv");
    std::fs::write(&victim, b"keep me").unwrap();
    let path = ws.out_dir().join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    // A hostile name, and a valid name with a hostile file: both guards are exercised.
    let hostile = text.replacen(
        "\"shaders\": [",
        "\"shaders\": [\n    { \"name\": \"../victim\", \"file\": \"../victim.spv\", \
         \"entry_points\": [], \"optimization\": \"debug\", \"spirv_sha256\": \"\", \
         \"source_key\": \"\" },\n    { \"name\": \"victim\", \"file\": \"../victim.spv\", \
         \"entry_points\": [], \"optimization\": \"debug\", \"spirv_sha256\": \"\", \
         \"source_key\": \"\" },",
        1,
    );
    assert_ne!(hostile, text);
    std::fs::write(&path, hostile).unwrap();

    ws.cook(Optimization::Debug).unwrap();
    assert!(victim.is_file(), "a path from the manifest was removed");
}
