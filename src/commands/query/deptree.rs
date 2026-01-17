// src/commands/query/deptree.rs

//! Dependency tree visualization
//!
//! Functions for displaying full dependency trees in a tree format,
//! supporting both forward and reverse dependency traversal.

use anyhow::Result;
use std::collections::HashSet;
use tracing::info;

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
