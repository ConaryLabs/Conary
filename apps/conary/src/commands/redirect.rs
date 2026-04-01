// src/commands/redirect.rs

//! Redirect command implementations for package aliasing and supersession

use super::open_db;
use anyhow::{Context, Result};
use conary_core::db::models::{Redirect, RedirectType};

/// Format a package name with an optional version constraint (e.g. "foo=1.0" or "foo")
fn format_name_version(name: &str, version: Option<&str>) -> String {
    if let Some(ver) = version {
        format!("{}={}", name, ver)
    } else {
        name.to_string()
    }
}

/// List all redirects
pub async fn cmd_redirect_list(
    db_path: &str,
    type_filter: Option<&str>,
    verbose: bool,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let redirects = if let Some(type_str) = type_filter {
        let redirect_type = type_str.parse::<RedirectType>().map_err(|_| {
            anyhow::anyhow!(
                "Invalid redirect type: {}. Use: rename, obsolete, merge, split",
                type_str
            )
        })?;
        Redirect::list_by_type(&conn, redirect_type)?
    } else {
        Redirect::list_all(&conn)?
    };

    if redirects.is_empty() {
        println!("No redirects configured.");
        return Ok(());
    }

    println!("Package Redirects:");
    println!("{}", "-".repeat(70));

    for redirect in &redirects {
        let source = format_name_version(&redirect.source_name, redirect.source_version.as_deref());
        let target = format_name_version(&redirect.target_name, redirect.target_version.as_deref());

        println!("{} -> {} ({})", source, target, redirect.redirect_type);

        if verbose {
            if let Some(ref msg) = redirect.message {
                println!("  Message: {}", msg);
            }
            if let Some(ref created) = redirect.created_at {
                println!("  Created: {}", created);
            }
            println!();
        }
    }

    if !verbose {
        println!("{}", "-".repeat(70));
        println!("{} redirect(s) total", redirects.len());
    }

    Ok(())
}

/// Add a new redirect
pub async fn cmd_redirect_add(
    source: &str,
    target: &str,
    db_path: &str,
    redirect_type: &str,
    source_version: Option<&str>,
    target_version: Option<&str>,
    message: Option<&str>,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let rtype = redirect_type.parse::<RedirectType>().map_err(|_| {
        anyhow::anyhow!(
            "Invalid redirect type: {}. Use: rename, obsolete, merge, split",
            redirect_type
        )
    })?;

    // Check if redirect already exists
    if Redirect::find_by_source(&conn, source, source_version)?.is_some() {
        return Err(anyhow::anyhow!(
            "Redirect for '{}' already exists",
            format_name_version(source, source_version)
        ));
    }

    // Check for circular redirects before adding
    let resolve_result = Redirect::resolve(&conn, target, target_version)?;
    if resolve_result.chain.contains(&source.to_string()) {
        return Err(anyhow::anyhow!(
            "Adding this redirect would create a circular chain: {} -> {} -> {}",
            source,
            target,
            source
        ));
    }

    let mut redirect = Redirect::new(source.to_string(), target.to_string(), rtype);
    redirect.source_version = source_version.map(String::from);
    redirect.target_version = target_version.map(String::from);
    redirect.message = message.map(String::from);

    redirect
        .insert(&conn)
        .context("Failed to insert redirect")?;

    println!(
        "Created redirect: {} -> {} ({})",
        format_name_version(source, source_version),
        format_name_version(target, target_version),
        redirect_type
    );

    if let Some(msg) = message {
        println!("Message: {}", msg);
    }

    Ok(())
}

/// Show details of a redirect
pub async fn cmd_redirect_show(source: &str, db_path: &str, version: Option<&str>) -> Result<()> {
    let conn = open_db(db_path)?;

    let redirect = Redirect::find_by_source(&conn, source, version)?;

    match redirect {
        Some(r) => {
            println!("Redirect Details:");
            println!("{}", "-".repeat(40));
            println!("Source: {}", r.source_name);
            if let Some(ref ver) = r.source_version {
                println!("Source Version: {}", ver);
            }
            println!("Target: {}", r.target_name);
            if let Some(ref ver) = r.target_version {
                println!("Target Version: {}", ver);
            }
            println!("Type: {}", r.redirect_type);
            if let Some(ref msg) = r.message {
                println!("Message: {}", msg);
            }
            if let Some(ref created) = r.created_at {
                println!("Created: {}", created);
            }

            // Show full resolution chain
            let resolve_result = Redirect::resolve(&conn, source, version)?;
            if resolve_result.chain.len() > 2 {
                println!();
                println!("Full Resolution Chain:");
                println!("  {}", resolve_result.chain.join(" -> "));
            }
        }
        None => {
            println!(
                "No redirect found for '{}'",
                format_name_version(source, version)
            );
        }
    }

    Ok(())
}

/// Remove a redirect
pub async fn cmd_redirect_remove(source: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let count = Redirect::delete_by_source(&conn, source)?;

    if count > 0 {
        println!("Removed {} redirect(s) for '{}'", count, source);
    } else {
        println!("No redirect found for '{}'", source);
    }

    Ok(())
}

/// Resolve a package name through redirect chain
pub async fn cmd_redirect_resolve(
    package: &str,
    db_path: &str,
    version: Option<&str>,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let result = Redirect::resolve(&conn, package, version)?;

    if result.was_redirected {
        println!("Resolution for '{}':", package);
        println!();
        println!("  Chain: {}", result.chain.join(" -> "));
        println!("  Resolved: {}", result.resolved);
        if let Some(ref ver) = result.version {
            println!("  Version: {}", ver);
        }

        if !result.messages.is_empty() {
            println!();
            println!("Messages:");
            for msg in &result.messages {
                println!("  - {}", msg);
            }
        }
    } else {
        println!("'{}' has no redirects, resolves to itself.", package);
    }

    Ok(())
}
