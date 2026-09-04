// crates/conary-xtask/src/line_cap.rs

use proc_macro2::{Span, TokenStream, TokenTree};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, ForeignItem, ImplItem, Item, Meta, TraitItem};

const PRODUCTION_LINE_LIMIT: usize = 1_000;
const INLINE_TEST_LINE_LIMIT: usize = 300;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct LineSpan {
    start: usize,
    end: usize,
}

impl LineSpan {
    fn line_count(self) -> usize {
        self.end - self.start + 1
    }
}

#[derive(Debug, Eq, PartialEq)]
struct FileMetrics {
    total_lines: usize,
    production_lines: usize,
    inline_test_lines: usize,
}

#[derive(Debug)]
struct Options {
    root: PathBuf,
    allowlist: PathBuf,
    report: bool,
}

pub(crate) fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = args.collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| argument == "-h" || argument == "--help")
    {
        println!(
            "Usage: cargo run -q -p conary-xtask -- line-cap --allowlist <path> [--root <path>] [--report]"
        );
        return Ok(());
    }
    let options = Options::parse(args.into_iter())?;
    let root = options.root.canonicalize().map_err(|error| {
        format!(
            "cannot resolve scan root {}: {error}",
            options.root.display()
        )
    })?;
    let allowlist = read_allowlist(&options.allowlist)?;
    let files = rust_source_files(&root)?;
    let mut used_allowlist_entries = BTreeSet::new();
    let mut errors = Vec::new();

    for path in files {
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| format!("cannot relativize {}: {error}", path.display()))?;
        let relative = path_text(relative);
        let metrics = match analyze_file(&path) {
            Ok(metrics) => metrics,
            Err(error) => {
                errors.push(format!("failed to parse {relative}: {error}"));
                continue;
            }
        };

        if options.report {
            println!(
                "{relative}\ttotal={}\tproduction={}\tinline_test={}",
                metrics.total_lines, metrics.production_lines, metrics.inline_test_lines
            );
        }

        let production_over = metrics.production_lines > PRODUCTION_LINE_LIMIT;
        let inline_test_over = metrics.inline_test_lines > INLINE_TEST_LINE_LIMIT;
        if !production_over && !inline_test_over {
            continue;
        }

        if allowlist.contains_key(&relative) {
            used_allowlist_entries.insert(relative);
            continue;
        }

        if production_over {
            errors.push(format!(
                "{relative} has {} non-test lines (limit: {PRODUCTION_LINE_LIMIT})",
                metrics.production_lines
            ));
        }
        if inline_test_over {
            errors.push(format!(
                "{relative} has {} inline test lines (limit: {INLINE_TEST_LINE_LIMIT})",
                metrics.inline_test_lines
            ));
        }
    }

    for (path, issue) in allowlist {
        if !used_allowlist_entries.contains(&path) {
            errors.push(format!("stale line-cap allowlist entry: {path} {issue}"));
        }
    }

    if errors.is_empty() {
        println!("Rust source line caps passed.");
        return Ok(());
    }

    for error in errors {
        eprintln!("ERROR: {error}");
    }
    Err("Rust source line caps failed".to_string())
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut root = env::current_dir().map_err(|error| format!("cannot read cwd: {error}"))?;
        let mut allowlist = None;
        let mut report = false;

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--root" => {
                    root = PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--root requires a path".to_string())?,
                    );
                }
                "--allowlist" => {
                    allowlist = Some(PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--allowlist requires a path".to_string())?,
                    ));
                }
                "--report" => report = true,
                _ => return Err(format!("unknown line-cap argument: {argument}")),
            }
        }

        let allowlist = allowlist.ok_or_else(|| "--allowlist requires a path".to_string())?;
        Ok(Self {
            root,
            allowlist,
            report,
        })
    }
}

fn read_allowlist(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("line-cap allowlist not found: {}: {error}", path.display()))?;
    let mut entries = BTreeMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2 || !valid_issue(fields[1]) {
            return Err(format!(
                "invalid allowlist entry (expected '<path> #<issue>'): {line}"
            ));
        }
        if entries
            .insert(fields[0].to_string(), fields[1].to_string())
            .is_some()
        {
            return Err(format!("duplicate allowlist entry: {}", fields[0]));
        }
    }

    Ok(entries)
}

fn valid_issue(value: &str) -> bool {
    value.strip_prefix('#').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn rust_source_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut found_source_root = false;
    for source_root in [root.join("apps"), root.join("crates")] {
        if !source_root.is_dir() {
            continue;
        }
        found_source_root = true;
        collect_rust_files(&source_root, root, &mut files)?;
    }
    if !found_source_root {
        return Err(format!(
            "no apps/ or crates/ source roots below {}",
            root.display()
        ));
    }
    files.sort();
    Ok(files)
}

fn collect_rust_files(
    directory: &Path,
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            if entry.file_name() != OsStr::new("target") {
                collect_rust_files(&path, root, files)?;
            }
        } else if file_type.is_file()
            && path.extension() == Some(OsStr::new("rs"))
            && !excluded_test_file(&path, root)?
        {
            files.push(path);
        }
    }
    Ok(())
}

fn excluded_test_file(path: &Path, root: &Path) -> Result<bool, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| format!("cannot relativize {}: {error}", path.display()))?;
    Ok(relative.file_name() == Some(OsStr::new("tests.rs"))
        || relative
            .components()
            .any(|component| component == Component::Normal(OsStr::new("tests"))))
}

fn analyze_file(path: &Path) -> Result<FileMetrics, String> {
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    analyze_source(&source).map_err(|error| error.to_string())
}

fn analyze_source(source: &str) -> syn::Result<FileMetrics> {
    let syntax = syn::parse_file(source)?;
    let mut visitor = TestSpanVisitor::default();
    visitor.visit_file(&syntax);
    let spans = union_spans(visitor.spans);
    let inline_test_lines = spans.iter().copied().map(LineSpan::line_count).sum();
    let total_lines = source.lines().count();
    Ok(FileMetrics {
        total_lines,
        production_lines: total_lines.saturating_sub(inline_test_lines),
        inline_test_lines,
    })
}

#[derive(Default)]
struct TestSpanVisitor {
    spans: Vec<LineSpan>,
}

impl TestSpanVisitor {
    fn record(&mut self, attributes: &[Attribute], span: Span) -> bool {
        if !attributes.iter().any(cfg_mentions_test) {
            return false;
        }
        let start = attributes
            .iter()
            .map(|attribute| attribute.span().start().line)
            .min()
            .unwrap_or_else(|| span.start().line);
        self.spans.push(LineSpan {
            start,
            end: span.end().line,
        });
        true
    }
}

impl<'ast> Visit<'ast> for TestSpanVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        if self.record(item_attributes(item), item.span()) {
            return;
        }
        visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if self.record(impl_item_attributes(item), item.span()) {
            return;
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        if self.record(trait_item_attributes(item), item.span()) {
            return;
        }
        visit::visit_trait_item(self, item);
    }

    fn visit_foreign_item(&mut self, item: &'ast ForeignItem) {
        if self.record(foreign_item_attributes(item), item.span()) {
            return;
        }
        visit::visit_foreign_item(self, item);
    }
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => &[],
        _ => &[],
    }
}

fn impl_item_attributes(item: &ImplItem) -> &[Attribute] {
    match item {
        ImplItem::Const(item) => &item.attrs,
        ImplItem::Fn(item) => &item.attrs,
        ImplItem::Type(item) => &item.attrs,
        ImplItem::Macro(item) => &item.attrs,
        ImplItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn trait_item_attributes(item: &TraitItem) -> &[Attribute] {
    match item {
        TraitItem::Const(item) => &item.attrs,
        TraitItem::Fn(item) => &item.attrs,
        TraitItem::Type(item) => &item.attrs,
        TraitItem::Macro(item) => &item.attrs,
        TraitItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn foreign_item_attributes(item: &ForeignItem) -> &[Attribute] {
    match item {
        ForeignItem::Fn(item) => &item.attrs,
        ForeignItem::Static(item) => &item.attrs,
        ForeignItem::Type(item) => &item.attrs,
        ForeignItem::Macro(item) => &item.attrs,
        ForeignItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn cfg_mentions_test(attribute: &Attribute) -> bool {
    attribute.path().is_ident("cfg")
        && match &attribute.meta {
            Meta::List(meta) => token_stream_mentions_test(&meta.tokens),
            _ => false,
        }
}

fn token_stream_mentions_test(tokens: &TokenStream) -> bool {
    tokens.clone().into_iter().any(|token| match token {
        TokenTree::Ident(identifier) => identifier == "test",
        TokenTree::Group(group) => token_stream_mentions_test(&group.stream()),
        _ => false,
    })
}

fn union_spans(mut spans: Vec<LineSpan>) -> Vec<LineSpan> {
    spans.sort_by_key(|span| (span.start, span.end));
    let mut union: Vec<LineSpan> = Vec::new();
    for span in spans {
        if let Some(previous) = union.last_mut()
            && span.start <= previous.end.saturating_add(1)
        {
            previous.end = previous.end.max(span.end);
        } else {
            union.push(span);
        }
    }
    union
}

fn path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_union_of_typed_test_item_spans() {
        let source = r#"fn production() {}
#[cfg(test)]
/* retained inside the test span */
/// test helper
fn helper() {
    assert!(true);
}
fn middle() {}
#[cfg(all(test, feature = "fixture"))]
const FIXTURE: &str = "value";
"#;

        assert_eq!(
            analyze_source(source).unwrap(),
            FileMetrics {
                total_lines: 10,
                production_lines: 2,
                inline_test_lines: 8,
            }
        );
    }

    #[test]
    fn rejects_malformed_rust() {
        assert!(analyze_source("fn broken( {").is_err());
    }

    #[test]
    fn cfg_feature_named_test_is_not_the_test_predicate() {
        let source = "#[cfg(feature = \"test\")]\nfn production() {}\n";
        assert_eq!(analyze_source(source).unwrap().production_lines, 2);
    }

    #[test]
    fn counts_associated_items_without_double_counting_a_test_impl() {
        let source = r#"struct Example;
impl Example {
    #[cfg(test)]
    const FIXTURE: usize = 1;
    fn production() {}
}
#[cfg(test)]
impl Example {
    #[cfg(test)]
    fn nested_test_helper() {}
}
"#;

        assert_eq!(
            analyze_source(source).unwrap(),
            FileMetrics {
                total_lines: 11,
                production_lines: 4,
                inline_test_lines: 7,
            }
        );
    }
}
