//! Changelog fragments (ADR-0009).
//!
//! Every PR with a user-visible change adds `changelog.d/<slug>.<kind>.md` containing one or more
//! Markdown bullet points. `check` validates the fragments. `release` writes them into
//! `CHANGELOG.md` as a new version section and deletes them.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directory (relative to the workspace root) holding unreleased fragments.
pub(crate) const FRAGMENT_DIR: &str = "changelog.d";
const CHANGELOG: &str = "CHANGELOG.md";
const UNRELEASED_HEADING: &str = "## [Unreleased]";

/// Keep a Changelog categories, in the order they appear in a release section.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Kind {
    Added,
    Changed,
    Deprecated,
    Removed,
    Fixed,
    Security,
}

impl Kind {
    const ALL: [Kind; 6] = [
        Kind::Added,
        Kind::Changed,
        Kind::Deprecated,
        Kind::Removed,
        Kind::Fixed,
        Kind::Security,
    ];

    fn file_tag(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Changed => "changed",
            Kind::Deprecated => "deprecated",
            Kind::Removed => "removed",
            Kind::Fixed => "fixed",
            Kind::Security => "security",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Kind::Added => "Added",
            Kind::Changed => "Changed",
            Kind::Deprecated => "Deprecated",
            Kind::Removed => "Removed",
            Kind::Fixed => "Fixed",
            Kind::Security => "Security",
        }
    }

    fn parse(tag: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.file_tag() == tag)
    }
}

/// A validated changelog fragment.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub(crate) file_name: String,
    pub(crate) kind: Kind,
    /// Trimmed Markdown bullet list.
    pub(crate) body: String,
}

/// Parses `<slug>.<kind>.md` where the slug is lowercase ASCII letters, digits and hyphens.
pub(crate) fn parse_file_name(name: &str) -> Result<Kind, String> {
    let kinds = Kind::ALL.map(Kind::file_tag).join(", ");
    let stem = name
        .strip_suffix(".md")
        .ok_or_else(|| format!("`{name}`: fragment files must end in `.md`"))?;
    let (slug, tag) = stem
        .rsplit_once('.')
        .ok_or_else(|| format!("`{name}`: expected `<slug>.<kind>.md` (kinds: {kinds})"))?;
    let slug_ok = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !slug_ok {
        return Err(format!(
            "`{name}`: slug `{slug}` must be lowercase letters, digits and hyphens"
        ));
    }
    Kind::parse(tag).ok_or_else(|| format!("`{name}`: unknown kind `{tag}` (valid: {kinds})"))
}

/// A fragment body must be non-empty and consist of `- ` bullets, optionally with indented
/// continuation lines (nested bullets or wrapped text).
pub(crate) fn validate_body(name: &str, body: &str) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(format!("`{name}`: fragment is empty"));
    }
    for (i, line) in trimmed.lines().enumerate() {
        let ok = line.starts_with("- ") || line.starts_with("  ") || line.is_empty();
        if !ok {
            return Err(format!(
                "`{name}` line {}: expected a `- ` bullet or an indented continuation line",
                i + 1
            ));
        }
    }
    if !trimmed.starts_with("- ") {
        return Err(format!("`{name}`: must start with a `- ` bullet"));
    }
    Ok(trimmed.replace("\r\n", "\n"))
}

/// Reads and validates all fragments, sorted by file name. Collects every error.
pub(crate) fn load_fragments(dir: &Path) -> Result<Vec<Fragment>, Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(vec![format!("cannot read `{}`: {e}", dir.display())]),
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();

    let mut fragments = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            errors.push(format!("non-UTF-8 file name in `{}`", dir.display()));
            continue;
        };
        if name == "README.md" || name.starts_with('.') {
            continue;
        }
        let result = parse_file_name(&name).and_then(|kind| {
            let text = fs::read_to_string(&path).map_err(|e| format!("`{name}`: {e}"))?;
            let body = validate_body(&name, &text)?;
            Ok(Fragment {
                file_name: name,
                kind,
                body,
            })
        });
        match result {
            Ok(f) => fragments.push(f),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(fragments)
    } else {
        Err(errors)
    }
}

/// Accepts `MAJOR.MINOR.PATCH` (no `v` prefix, no pre-release: releases are final versions).
pub(crate) fn validate_version(v: &str) -> Result<(), String> {
    let parts: Vec<&str> = v.split('.').collect();
    let numeric = |p: &&str| {
        !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && (*p == "0" || !p.starts_with('0'))
    };
    if parts.len() == 3 && parts.iter().all(numeric) {
        Ok(())
    } else {
        Err(format!(
            "`{v}` is not a release version; expected MAJOR.MINOR.PATCH such as 0.1.0"
        ))
    }
}

/// Renders a release section, grouping fragments by kind in Keep a Changelog order.
pub(crate) fn render_section(version: &str, date: &str, fragments: &[Fragment]) -> String {
    let mut out = format!("## [{version}] - {date}\n");
    for kind in Kind::ALL {
        let mut bodies = fragments.iter().filter(|f| f.kind == kind).peekable();
        if bodies.peek().is_none() {
            continue;
        }
        let _ = write!(out, "\n### {}\n", kind.heading());
        for f in bodies {
            let _ = writeln!(out, "{}", f.body);
        }
    }
    out
}

/// Inserts `section` directly after the `[Unreleased]` section (before the previous release).
pub(crate) fn insert_section(
    changelog: &str,
    version: &str,
    section: &str,
) -> Result<String, String> {
    if changelog.contains(&format!("## [{version}]")) {
        return Err(format!("CHANGELOG.md already has a section for {version}"));
    }
    let start = changelog
        .find(UNRELEASED_HEADING)
        .ok_or_else(|| format!("CHANGELOG.md has no `{UNRELEASED_HEADING}` heading"))?;
    let after_heading = start + UNRELEASED_HEADING.len();
    let insert_at = changelog[after_heading..]
        .find("\n## [")
        .map_or(changelog.len(), |i| after_heading + i + 1);
    let (head, tail) = changelog.split_at(insert_at);
    let head = head.trim_end_matches('\n');
    let mut out = format!("{head}\n\n{}", section.trim_end());
    if tail.is_empty() {
        out.push('\n');
    } else {
        out.push_str("\n\n");
        out.push_str(tail);
    }
    Ok(out)
}

/// Converts days since 1970-01-01 to a (year, month, day) civil date (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Algorithm from Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Month is 1..=12 and day is 1..=31 by construction.
    (year, month as u32, day as u32)
}

fn today_utc() -> Result<String, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock is before 1970: {e}"))?
        .as_secs();
    let days = i64::try_from(secs / 86_400).map_err(|e| format!("date out of range: {e}"))?;
    let (y, m, d) = civil_from_days(days);
    Ok(format!("{y:04}-{m:02}-{d:02}"))
}

const USAGE: &str =
    "usage: cargo xtask changelog <check | release <MAJOR.MINOR.PATCH> [--dry-run]>";

/// `cargo xtask changelog <check | release <version> [--dry-run]>`.
///
/// Always operates on the workspace at `root`, regardless of the current directory.
pub(crate) fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let dir = root.join(FRAGMENT_DIR);
    match args.first().map(String::as_str) {
        Some("check") => {
            let fragments = load_fragments(&dir).map_err(|errs| report(&errs))?;
            println!(
                "changelog: {} fragment(s) in {FRAGMENT_DIR}/ OK",
                fragments.len()
            );
            Ok(())
        }
        Some("release") => {
            let (version, dry_run) = match &args[1..] {
                [v] => (v, false),
                [v, flag] if flag == "--dry-run" => (v, true),
                _ => return Err(USAGE.to_owned()),
            };
            validate_version(version)?;
            let fragments = load_fragments(&dir).map_err(|errs| report(&errs))?;
            if fragments.is_empty() {
                return Err(format!(
                    "no fragments in {FRAGMENT_DIR}/; nothing to release"
                ));
            }
            let path = root.join(CHANGELOG);
            let changelog = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let section = render_section(version, &today_utc()?, &fragments);
            let updated = insert_section(&changelog, version, &section)?;
            if dry_run {
                println!("{section}");
                println!(
                    "changelog: dry run. {} would get this section and {} fragment(s) would be \
                     removed. Nothing was changed.",
                    path.display(),
                    fragments.len()
                );
                return Ok(());
            }
            fs::write(&path, updated)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            for f in &fragments {
                let p = dir.join(&f.file_name);
                fs::remove_file(&p).map_err(|e| format!("cannot remove {}: {e}", p.display()))?;
            }
            println!(
                "changelog: wrote [{version}] from {} fragment(s); fragments removed",
                fragments.len()
            );
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn report(errors: &[String]) -> String {
    format!(
        "{} invalid changelog fragment(s) (see {FRAGMENT_DIR}/README.md):\n  - {}",
        errors.len(),
        errors.join("\n  - ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(name: &str, kind: Kind, body: &str) -> Fragment {
        Fragment {
            file_name: name.to_owned(),
            kind,
            body: body.to_owned(),
        }
    }

    #[test]
    fn file_names() {
        assert_eq!(parse_file_name("vulkan-init.added.md"), Ok(Kind::Added));
        assert_eq!(parse_file_name("fix-42.fixed.md"), Ok(Kind::Fixed));
        assert!(parse_file_name("thing.md").is_err());
        assert!(parse_file_name("thing.improved.md").is_err());
        assert!(parse_file_name("Bad_Slug.added.md").is_err());
        assert!(parse_file_name(".added.md").is_err());
        assert!(parse_file_name("thing.added.txt").is_err());
    }

    #[test]
    fn bodies() {
        assert_eq!(validate_body("f", "- One\n"), Ok("- One".to_owned()));
        assert!(validate_body("f", "- One\n  - nested\n- Two\r\n").is_ok());
        assert!(validate_body("f", "  \n").is_err());
        assert!(validate_body("f", "Not a bullet").is_err());
        assert!(validate_body("f", "- One\nstray text").is_err());
    }

    #[test]
    fn versions() {
        assert!(validate_version("0.1.0").is_ok());
        assert!(validate_version("1.20.3").is_ok());
        assert!(validate_version("v0.1.0").is_err());
        assert!(validate_version("0.1").is_err());
        assert!(validate_version("0.1.0-dev").is_err());
        assert!(validate_version("01.1.0").is_err());
    }

    #[test]
    fn section_groups_in_keep_a_changelog_order() {
        let fragments = [
            frag("b.fixed.md", Kind::Fixed, "- Fix B"),
            frag("a.added.md", Kind::Added, "- Add A"),
            frag("c.added.md", Kind::Added, "- Add C"),
        ];
        let s = render_section("0.1.0", "2026-11-19", &fragments);
        assert_eq!(
            s,
            "## [0.1.0] - 2026-11-19\n\n### Added\n- Add A\n- Add C\n\n### Fixed\n- Fix B\n"
        );
    }

    #[test]
    fn insert_before_previous_release() {
        let log = "# Changelog\n\n## [Unreleased]\n\nSee changelog.d/.\n\n## [0.1.0] - 2026-01-01\n\n### Added\n- Old\n";
        let out = insert_section(
            log,
            "0.2.0",
            "## [0.2.0] - 2026-02-01\n\n### Added\n- New\n",
        )
        .unwrap();
        assert_eq!(
            out,
            "# Changelog\n\n## [Unreleased]\n\nSee changelog.d/.\n\n## [0.2.0] - 2026-02-01\n\n### Added\n- New\n\n## [0.1.0] - 2026-01-01\n\n### Added\n- Old\n"
        );
    }

    #[test]
    fn insert_first_release_and_reject_duplicates() {
        let log = "# Changelog\n\n## [Unreleased]\n\nSee changelog.d/.\n";
        let out = insert_section(
            log,
            "0.1.0",
            "## [0.1.0] - 2026-02-01\n\n### Added\n- New\n",
        )
        .unwrap();
        assert!(
            out.ends_with("See changelog.d/.\n\n## [0.1.0] - 2026-02-01\n\n### Added\n- New\n")
        );
        assert!(insert_section(&out, "0.1.0", "## [0.1.0] - x\n").is_err());
        assert!(insert_section("# Changelog\n", "0.1.0", "x").is_err());
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_776), (2026, 11, 19));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn load_reports_all_errors_and_skips_readme() {
        let dir = std::env::temp_dir().join(format!("oxreal-xtask-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README.md"), "docs").unwrap();
        fs::write(dir.join("good.added.md"), "- Good\n").unwrap();
        fs::write(dir.join("bad.nope.md"), "- Bad\n").unwrap();
        fs::write(dir.join("empty.fixed.md"), "\n").unwrap();
        let errs = load_fragments(&dir).unwrap_err();
        assert_eq!(errs.len(), 2);
        fs::remove_file(dir.join("bad.nope.md")).unwrap();
        fs::remove_file(dir.join("empty.fixed.md")).unwrap();
        let ok = load_fragments(&dir).unwrap();
        assert_eq!(ok, vec![frag("good.added.md", Kind::Added, "- Good")]);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load_fragments(&dir).unwrap(), Vec::new());
    }
}
