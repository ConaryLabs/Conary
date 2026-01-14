// src/commands/query.rs
//! Query and dependency inspection commands

use anyhow::Result;
use std::collections::HashSet;
use tracing::info;

/// Options for the query command
#[derive(Default)]
pub struct QueryOptions {
    /// Show detailed package information
    pub info: bool,
    /// Show files in ls -l style
    pub lsl: bool,
    /// Find package by file path
    pub path: Option<String>,
    /// List files in package
    pub files: bool,
}

/// Query installed packages
pub fn cmd_query(pattern: Option<&str>, db_path: &str, options: QueryOptions) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Path query mode: find package containing a file
    if let Some(file_path) = &options.path {
        return query_by_path(&conn, file_path, &options);
    }

    let troves = if let Some(pattern) = pattern {
        conary::db::models::Trove::find_by_name(&conn, pattern)?
    } else {
        conary::db::models::Trove::list_all(&conn)?
    };

    if troves.is_empty() {
        println!("No packages found.");
        return Ok(());
    }

    // Detailed info mode
    if options.info && troves.len() == 1 {
        return show_package_info(&conn, &troves[0], &options);
    }

    // List with files (ls -l style)
    if (options.lsl || options.files) && troves.len() == 1 {
        return list_package_files(&conn, &troves[0], options.lsl);
    }

    // Standard listing
    println!("Installed packages:");
    for trove in &troves {
        print!(
            "  {} {} ({:?})",
            trove.name, trove.version, trove.trove_type
        );
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} package(s)", troves.len());

    Ok(())
}

/// Query package by file path
fn query_by_path(
    conn: &rusqlite::Connection,
    file_path: &str,
    options: &QueryOptions,
) -> Result<()> {
    // Try exact match first
    let file = conary::db::models::FileEntry::find_by_path(conn, file_path)?;

    if let Some(file) = file
        && let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(conn, file.trove_id)
    {
        if options.info {
            return show_package_info(conn, &trove, options);
        }
        println!("{} {} provides:", trove.name, trove.version);
        println!("  {}", file_path);
        return Ok(());
    }

    // Try pattern match
    let pattern = if file_path.contains('%') || file_path.contains('*') {
        file_path.replace('*', "%")
    } else {
        format!("%{file_path}%")
    };

    let files = conary::db::models::FileEntry::find_by_path_pattern(conn, &pattern)?;
    if files.is_empty() {
        println!("No package owns file matching '{}'", file_path);
        return Ok(());
    }

    // Group by trove
    let mut trove_files: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    for file in &files {
        trove_files.entry(file.trove_id).or_default().push(file.path.clone());
    }

    println!("Packages owning files matching '{}':", file_path);
    for (trove_id, paths) in &trove_files {
        if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(conn, *trove_id) {
            println!("\n{} {}:", trove.name, trove.version);
            for path in paths {
                println!("  {}", path);
            }
        }
    }

    Ok(())
}

/// Show detailed package information
fn show_package_info(
    conn: &rusqlite::Connection,
    trove: &conary::db::models::Trove,
    _options: &QueryOptions,
) -> Result<()> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    println!("Name        : {}", trove.name);
    println!("Version     : {}", trove.version);
    println!("Type        : {:?}", trove.trove_type);

    if let Some(arch) = &trove.architecture {
        println!("Architecture: {}", arch);
    }

    if let Some(desc) = &trove.description {
        println!("Description : {}", desc);
    }

    if let Some(installed) = &trove.installed_at {
        println!("Installed   : {}", installed);
    }

    if let Some(reason) = &trove.selection_reason {
        println!("Reason      : {}", reason);
    }

    // Show install reason
    println!("Install Type: {:?}", trove.install_reason);
    println!("Pinned      : {}", if trove.pinned { "yes" } else { "no" });

    // Count files
    let files = conary::db::models::FileEntry::find_by_trove(conn, trove_id)?;
    println!("Files       : {}", files.len());

    // Calculate total size
    let total_size: i64 = files.iter().map(|f| f.size).sum();
    println!("Size        : {}", format_size(total_size));

    // Dependencies
    let deps = conary::db::models::DependencyEntry::find_by_trove(conn, trove_id)?;
    if !deps.is_empty() {
        println!("\nDependencies ({}):", deps.len());
        for dep in &deps {
            println!("  {}", dep.to_typed_string());
        }
    }

    // Provides
    let provides = conary::db::models::ProvideEntry::find_by_trove(conn, trove_id)?;
    if !provides.is_empty() {
        println!("\nProvides ({}):", provides.len());
        for p in &provides {
            println!("  {}", p.to_typed_string());
        }
    }

    // Components
    let components = conary::db::models::Component::find_by_trove(conn, trove_id)?;
    if !components.is_empty() {
        println!("\nComponents ({}):", components.len());
        for comp in &components {
            let installed = if comp.is_installed { "" } else { " [not installed]" };
            println!("  :{}{}", comp.name, installed);
        }
    }

    Ok(())
}

/// List package files
fn list_package_files(
    conn: &rusqlite::Connection,
    trove: &conary::db::models::Trove,
    lsl: bool,
) -> Result<()> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
    let files = conary::db::models::FileEntry::list_files_lsl(conn, trove_id)?;

    if files.is_empty() {
        println!("No files in package {} {}", trove.name, trove.version);
        return Ok(());
    }

    println!("Files in {} {} ({} files):", trove.name, trove.version, files.len());

    if lsl {
        // ls -l style output
        for file in &files {
            println!(
                "{} {:>8} {:>8} {:>8} {}",
                file.format_permissions(),
                file.owner.as_deref().unwrap_or("root"),
                file.group_name.as_deref().unwrap_or("root"),
                file.size_human(),
                file.path
            );
        }
    } else {
        // Simple list
        for file in &files {
            println!("{}", file.path);
        }
    }

    Ok(())
}

/// Format size as human-readable
fn format_size(size: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{} bytes", size)
    }
}

/// Query packages available in repositories (not installed)
///
/// This is similar to `dnf repoquery` or `apt-cache search`.
pub fn cmd_repquery(pattern: Option<&str>, db_path: &str, info: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let packages = if let Some(pattern) = pattern {
        conary::db::models::RepositoryPackage::search(&conn, pattern)?
    } else {
        conary::db::models::RepositoryPackage::list_all(&conn)?
    };

    if packages.is_empty() {
        if let Some(p) = pattern {
            println!("No packages matching '{}' found in repositories.", p);
        } else {
            println!("No packages in repositories. Run 'conary repo-sync' first.");
        }
        return Ok(());
    }

    // If info mode and single result, show detailed info
    if info && packages.len() == 1 {
        return show_repo_package_info(&conn, &packages[0]);
    }

    println!("Available packages{}:", pattern.map(|p| format!(" matching '{}'", p)).unwrap_or_default());
    for pkg in &packages {
        print!("  {} {}", pkg.name, pkg.version);
        if let Some(arch) = &pkg.architecture {
            print!(" [{}]", arch);
        }
        // Show which repo it's from
        if let Ok(repo_name) = pkg.get_repository_name(&conn) {
            print!(" @{}", repo_name);
        }
        println!();
    }
    println!("\nTotal: {} package(s) available", packages.len());

    Ok(())
}

/// Show detailed info for a repository package
fn show_repo_package_info(
    conn: &rusqlite::Connection,
    pkg: &conary::db::models::RepositoryPackage,
) -> Result<()> {
    println!("Name        : {}", pkg.name);
    println!("Version     : {}", pkg.version);

    if let Some(arch) = &pkg.architecture {
        println!("Architecture: {}", arch);
    }

    if let Some(desc) = &pkg.description {
        println!("Description : {}", desc);
    }

    println!("Size        : {}", pkg.size_human());

    if let Ok(repo_name) = pkg.get_repository_name(conn) {
        println!("Repository  : {}", repo_name);
    }

    println!("Checksum    : {}", pkg.checksum);
    println!("URL         : {}", pkg.download_url);

    // Check if installed
    let installed = conary::db::models::Trove::find_by_name(conn, &pkg.name)?;
    if let Some(installed_pkg) = installed.first() {
        println!("Status      : Installed ({})", installed_pkg.version);
    } else {
        println!("Status      : Not installed");
    }

    // Show dependencies
    if let Ok(deps) = pkg.parse_dependencies()
        && !deps.is_empty()
    {
        println!("\nDependencies ({}):", deps.len());
        for dep in &deps {
            println!("  {}", dep);
        }
    }

    Ok(())
}

/// Show changeset history
pub fn cmd_history(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;
    let changesets = conary::db::models::Changeset::list_all(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
            let timestamp = changeset
                .applied_at
                .as_ref()
                .or(changeset.rolled_back_at.as_ref())
                .or(changeset.created_at.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("pending");
            let id = changeset
                .id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "?".to_string());
            println!(
                "  [{}] {} - {} ({:?})",
                id, timestamp, changeset.description, changeset.status
            );
        }
        println!("\nTotal: {} changeset(s)", changesets.len());
    }

    Ok(())
}

/// Show dependencies for a package
pub fn cmd_depends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing dependencies for package: {}", package_name);
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    let trove = troves
        .first()
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let deps = conary::db::models::DependencyEntry::find_by_trove(&conn, trove_id)?;

    if deps.is_empty() {
        println!("Package '{}' has no dependencies", package_name);
    } else {
        println!("Dependencies for package '{}':", package_name);
        for dep in deps {
            // Display typed dependency
            let typed_str = dep.to_typed_string();
            print!("  {} [{}]", typed_str, dep.dependency_type);
            if let Some(version) = dep.depends_on_version {
                print!(" - version: {}", version);
            }
            println!();
        }
    }

    Ok(())
}

/// Show reverse dependencies
pub fn cmd_rdepends(package_name: &str, db_path: &str) -> Result<()> {
    info!(
        "Showing reverse dependencies for package: {}",
        package_name
    );
    let conn = conary::db::open(db_path)?;

    let dependents = conary::db::models::DependencyEntry::find_dependents(&conn, package_name)?;

    if dependents.is_empty() {
        println!(
            "No packages depend on '{}' (or package not installed)",
            package_name
        );
    } else {
        println!("Packages that depend on '{}':", package_name);
        for dep in dependents {
            if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(&conn, dep.trove_id) {
                // Show the dependency kind if not a plain package
                let kind_str = if dep.kind != "package" && !dep.kind.is_empty() {
                    format!(" [{}]", dep.kind)
                } else {
                    String::new()
                };
                print!("  {} ({}){}",trove.name, dep.dependency_type, kind_str);
                if let Some(constraint) = dep.version_constraint {
                    print!(" - requires: {}", constraint);
                }
                println!();
            }
        }
    }

    Ok(())
}

/// Show what packages would break if a package is removed
pub fn cmd_whatbreaks(package_name: &str, db_path: &str) -> Result<()> {
    info!(
        "Checking what would break if '{}' is removed...",
        package_name
    );
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    troves
        .first()
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;

    let resolver = conary::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

    if breaking.is_empty() {
        println!(
            "Package '{}' can be safely removed (no dependencies)",
            package_name
        );
    } else {
        println!(
            "Removing '{}' would break the following packages:",
            package_name
        );
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nTotal: {} packages would be affected", breaking.len());
    }

    Ok(())
}

/// List components of an installed package
pub fn cmd_list_components(package_name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find the package
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        println!("Package: {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            println!("  Architecture: {}", arch);
        }

        // Get components
        let components = conary::db::models::Component::find_by_trove(&conn, trove_id)?;

        if components.is_empty() {
            println!("  Components: (none - legacy install)");
        } else {
            println!("  Components:");
            for comp in &components {
                let file_count = conary::db::models::FileEntry::find_by_component(&conn, comp.id.unwrap_or(0))?.len();
                let default_marker = if conary::components::ComponentType::parse(&comp.name)
                    .map(|ct| ct.is_default())
                    .unwrap_or(false)
                {
                    " (default)"
                } else {
                    ""
                };
                let installed_marker = if comp.is_installed { "" } else { " [not installed]" };
                println!(
                    "    :{} - {} files{}{}",
                    comp.name, file_count, default_marker, installed_marker
                );
            }
        }
        println!();
    }

    Ok(())
}

/// Find what package provides a capability
///
/// Searches for packages that provide a given capability, which can be:
/// - A package name
/// - A virtual provide (e.g., perl(DBI))
/// - A file path (e.g., /usr/bin/python3)
/// - A shared library (e.g., libssl.so.3)
pub fn cmd_whatprovides(capability: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // First try exact match
    let mut providers = conary::db::models::ProvideEntry::find_all_by_capability(&conn, capability)?;

    // If no exact match, try pattern search
    if providers.is_empty() {
        // Try with wildcards for partial matching
        let pattern = format!("%{}%", capability);
        providers = conary::db::models::ProvideEntry::search_capability(&conn, &pattern)?;
    }

    if providers.is_empty() {
        println!("No package provides '{}'", capability);
        return Ok(());
    }

    println!("Capability '{}' is provided by:", capability);
    for provide in &providers {
        if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(&conn, provide.trove_id) {
            print!("  {} {}", trove.name, trove.version);
            if let Some(ref ver) = provide.version {
                print!(" (provides version: {})", ver);
            }
            if let Some(ref arch) = trove.architecture {
                print!(" [{}]", arch);
            }
            println!();
        }
    }

    println!("\nTotal: {} provider(s)", providers.len());
    Ok(())
}

/// Query files in a specific component
pub fn cmd_query_component(component_spec: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Parse the component spec (e.g., "nginx:lib")
    let (package_name, component_name) = conary::components::parse_component_spec(component_spec)
        .ok_or_else(|| anyhow::anyhow!(
            "Invalid component spec '{}'. Expected format: package:component (e.g., nginx:lib)",
            component_spec
        ))?;

    // Find the package
    let troves = conary::db::models::Trove::find_by_name(&conn, &package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        // Find the component
        let component = conary::db::models::Component::find_by_trove_and_name(&conn, trove_id, &component_name)?;

        match component {
            Some(comp) => {
                let comp_id = comp.id.ok_or_else(|| anyhow::anyhow!("Component has no ID"))?;
                let files = conary::db::models::FileEntry::find_by_component(&conn, comp_id)?;

                println!("{}:{} ({} {})", package_name, component_name, trove.name, trove.version);
                if let Some(desc) = &comp.description {
                    println!("  Description: {}", desc);
                }
                println!("  Files: {}", files.len());
                println!();

                for file in &files {
                    println!("  {}", file.path);
                }
            }
            None => {
                // Check if any components exist
                let components = conary::db::models::Component::find_by_trove(&conn, trove_id)?;
                if components.is_empty() {
                    println!(
                        "Package '{}' was installed without component tracking (legacy install)",
                        package_name
                    );
                    println!("All files belong to the implicit :runtime component.");
                } else {
                    let available: Vec<String> = components.iter().map(|c| format!(":{}", c.name)).collect();
                    return Err(anyhow::anyhow!(
                        "Component '{}' not found in package '{}'. Available: {}",
                        component_name,
                        package_name,
                        available.join(", ")
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Show full dependency tree for a package
///
/// Displays a tree visualization of all transitive dependencies (forward)
/// or all transitive reverse dependencies (what depends on this package).
pub fn cmd_deptree(package_name: &str, db_path: &str, reverse: bool, max_depth: Option<usize>) -> Result<()> {
    info!(
        "Building {} dependency tree for package: {}",
        if reverse { "reverse" } else { "forward" },
        package_name
    );
    let conn = conary::db::open(db_path)?;

    // Verify package exists
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    let trove = &troves[0];
    println!(
        "{} {} ({})",
        trove.name,
        trove.version,
        if reverse { "reverse deps" } else { "dependencies" }
    );

    // Create tree context
    let mut ctx = TreeContext::new(&conn, max_depth);
    ctx.visited.insert(package_name.to_string());

    if reverse {
        // Reverse dependency tree: what depends on this package, transitively
        print_reverse_tree(&mut ctx, package_name, "", 0)?;
    } else {
        // Forward dependency tree: what this package depends on, transitively
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
        print_dependency_tree(&mut ctx, trove_id, "", 0)?;
    }

    // Print summary
    println!();
    println!(
        "{} unique dependencies, {} total nodes{}",
        ctx.stats.unique_packages,
        ctx.stats.total_nodes,
        if ctx.stats.cycles_detected > 0 {
            format!(", {} cycle(s) detected", ctx.stats.cycles_detected)
        } else {
            String::new()
        }
    );

    Ok(())
}

/// Context for tree traversal, reducing parameter count
struct TreeContext<'a> {
    conn: &'a rusqlite::Connection,
    visited: HashSet<String>,
    max_depth: Option<usize>,
    stats: TreeStats,
}

/// Statistics for tree traversal
#[derive(Default)]
struct TreeStats {
    unique_packages: usize,
    total_nodes: usize,
    cycles_detected: usize,
}

impl<'a> TreeContext<'a> {
    fn new(conn: &'a rusqlite::Connection, max_depth: Option<usize>) -> Self {
        Self {
            conn,
            visited: HashSet::new(),
            max_depth,
            stats: TreeStats::default(),
        }
    }
}

/// Recursively print the forward dependency tree
fn print_dependency_tree(
    ctx: &mut TreeContext<'_>,
    trove_id: i64,
    prefix: &str,
    depth: usize,
) -> Result<()> {
    // Check depth limit
    if ctx.max_depth.is_some_and(|max| depth >= max) {
        return Ok(());
    }

    // Get dependencies for this package
    let deps = conary::db::models::DependencyEntry::find_by_trove(ctx.conn, trove_id)?;

    // Filter to runtime dependencies only, and only those that are installed
    let mut installed_deps = Vec::new();
    for dep in &deps {
        if dep.dependency_type != "runtime" {
            continue;
        }
        // Check if this dependency is installed
        if let Ok(dep_troves) = conary::db::models::Trove::find_by_name(ctx.conn, &dep.depends_on_name)
            && let Some(dep_trove) = dep_troves.first()
        {
            installed_deps.push((dep.depends_on_name.clone(), dep_trove.clone()));
        }
    }

    for (i, (dep_name, dep_trove)) in installed_deps.iter().enumerate() {
        let is_last_dep = i == installed_deps.len() - 1;
        let connector = if is_last_dep { "\\-- " } else { "|-- " };
        let next_prefix = if is_last_dep { "    " } else { "|   " };

        ctx.stats.total_nodes += 1;

        // Check for cycles
        if ctx.visited.contains(dep_name) {
            println!("{}{}{} {} [circular]", prefix, connector, dep_name, dep_trove.version);
            ctx.stats.cycles_detected += 1;
            continue;
        }

        println!("{}{}{} {}", prefix, connector, dep_name, dep_trove.version);
        ctx.stats.unique_packages += 1;

        // Mark as visited and recurse
        ctx.visited.insert(dep_name.clone());
        if let Some(dep_id) = dep_trove.id {
            print_dependency_tree(
                ctx,
                dep_id,
                &format!("{}{}", prefix, next_prefix),
                depth + 1,
            )?;
        }
    }

    Ok(())
}

/// Recursively print the reverse dependency tree
fn print_reverse_tree(
    ctx: &mut TreeContext<'_>,
    package_name: &str,
    prefix: &str,
    depth: usize,
) -> Result<()> {
    // Check depth limit
    if ctx.max_depth.is_some_and(|max| depth >= max) {
        return Ok(());
    }

    // Find packages that depend on this one
    let dependents = conary::db::models::DependencyEntry::find_dependents(ctx.conn, package_name)?;

    // Get unique package names
    let mut unique_dependents = Vec::new();
    let mut seen_names = HashSet::new();
    for dep in &dependents {
        if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(ctx.conn, dep.trove_id)
            && !seen_names.contains(&trove.name)
        {
            seen_names.insert(trove.name.clone());
            unique_dependents.push(trove);
        }
    }

    for (i, dep_trove) in unique_dependents.iter().enumerate() {
        let is_last_dep = i == unique_dependents.len() - 1;
        let connector = if is_last_dep { "\\-- " } else { "|-- " };
        let next_prefix = if is_last_dep { "    " } else { "|   " };

        ctx.stats.total_nodes += 1;

        // Check for cycles
        if ctx.visited.contains(&dep_trove.name) {
            println!("{}{}{} {} [circular]", prefix, connector, dep_trove.name, dep_trove.version);
            ctx.stats.cycles_detected += 1;
            continue;
        }

        println!("{}{}{} {}", prefix, connector, dep_trove.name, dep_trove.version);
        ctx.stats.unique_packages += 1;

        // Mark as visited and recurse
        ctx.visited.insert(dep_trove.name.clone());
        print_reverse_tree(
            ctx,
            &dep_trove.name,
            &format!("{}{}", prefix, next_prefix),
            depth + 1,
        )?;
    }

    Ok(())
}

/// Query packages by installation reason
///
/// Filters packages by their selection_reason field. Supports patterns:
/// - "explicit" or "explicitly" - packages installed directly by user
/// - "dependency" or "required" - packages installed as dependencies
/// - "collection" or "@*" - packages installed via collections
/// - Custom pattern with * wildcard - e.g., "Required by nginx"
pub fn cmd_query_reason(pattern: Option<&str>, db_path: &str) -> Result<()> {
    info!("Querying packages by reason: {:?}", pattern);
    let conn = conary::db::open(db_path)?;

    let (troves, filter_desc) = match pattern {
        Some("explicit") | Some("explicitly") => {
            (conary::db::models::Trove::find_explicitly_installed(&conn)?, "explicitly installed")
        }
        Some("dependency") | Some("required") | Some("dep") => {
            (conary::db::models::Trove::find_dependencies_installed(&conn)?, "installed as dependencies")
        }
        Some("collection") | Some("@") => {
            (conary::db::models::Trove::find_collection_installed(&conn)?, "installed via collections")
        }
        Some(custom) if custom.starts_with("@") => {
            // Pattern like "@server" - find packages from specific collection
            let pattern = format!("Installed via {}", custom);
            (conary::db::models::Trove::find_by_reason(&conn, &pattern)?, &*format!("installed via {}", custom))
        }
        Some(custom) => {
            // Custom pattern
            (conary::db::models::Trove::find_by_reason(&conn, custom)?, custom)
        }
        None => {
            // Show all with their reasons grouped
            return print_all_with_reasons(&conn);
        }
    };

    if troves.is_empty() {
        println!("No packages found matching reason: {}", filter_desc);
    } else {
        println!("Packages {} ({}):", filter_desc, troves.len());
        for trove in &troves {
            print!("  {} {}", trove.name, trove.version);
            if let Some(reason) = &trove.selection_reason {
                print!(" - {}", reason);
            }
            println!();
        }
    }

    Ok(())
}

/// Print all packages grouped by their installation reason
fn print_all_with_reasons(conn: &rusqlite::Connection) -> Result<()> {
    let all_troves = conary::db::models::Trove::list_all(conn)?;

    // Group by reason
    let mut explicit = Vec::new();
    let mut dependencies = Vec::new();
    let mut collections = Vec::new();
    let mut other = Vec::new();

    for trove in all_troves {
        match &trove.selection_reason {
            Some(r) if r == "Explicitly installed" => explicit.push(trove),
            Some(r) if r.starts_with("Required by") => dependencies.push(trove),
            Some(r) if r.starts_with("Installed via @") => collections.push(trove),
            _ => other.push(trove),
        }
    }

    if !explicit.is_empty() {
        println!("Explicitly installed ({}):", explicit.len());
        for t in &explicit {
            println!("  {} {}", t.name, t.version);
        }
        println!();
    }

    if !dependencies.is_empty() {
        println!("Installed as dependencies ({}):", dependencies.len());
        for t in &dependencies {
            let reason = t.selection_reason.as_deref().unwrap_or("");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    if !collections.is_empty() {
        println!("Installed via collections ({}):", collections.len());
        for t in &collections {
            let reason = t.selection_reason.as_deref().unwrap_or("");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    if !other.is_empty() {
        println!("Other ({}):", other.len());
        for t in &other {
            let reason = t.selection_reason.as_deref().unwrap_or("(no reason recorded)");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    let total = explicit.len() + dependencies.len() + collections.len() + other.len();
    println!("Total: {} package(s)", total);

    Ok(())
}
