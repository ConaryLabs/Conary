// src/commands/redirect.rs

//! Redirect command implementations for package aliasing and supersession

use anyhow::{Context, Result};
use conary::db::models::{Redirect, RedirectType};

/// List all redirects
pub fn cmd_redirect_list(
    db_path: &str,
    type_filter: Option<&str>,
    verbose: bool,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let redirects = if let Some(type_str) = type_filter {
        let redirect_type = type_str.parse::<RedirectType>()
            .map_err(|_| anyhow::anyhow!("Invalid redirect type: {}. Use: rename, obsolete, merge, split", type_str))?;
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
        let source = if let Some(ref ver) = redirect.source_version {
            format!("{}={}", redirect.source_name, ver)
        } else {
            redirect.source_name.clone()
        };

        let target = if let Some(ref ver) = redirect.target_version {
            format!("{}={}", redirect.target_name, ver)
        } else {
            redirect.target_name.clone()
        };

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
pub fn cmd_redirect_add(
    source: &str,
    target: &str,
    db_path: &str,
    redirect_type: &str,
    source_version: Option<&str>,
    target_version: Option<&str>,
    message: Option<&str>,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let rtype = redirect_type.parse::<RedirectType>()
        .map_err(|_| anyhow::anyhow!("Invalid redirect type: {}. Use: rename, obsolete, merge, split", redirect_type))?;

    // Check if redirect already exists
    if Redirect::find_by_source(&conn, source, source_version)?.is_some() {
        let source_desc = if let Some(ver) = source_version {
            format!("{}={}", source, ver)
        } else {
            source.to_string()
        };
        return Err(anyhow::anyhow!("Redirect for '{}' already exists", source_desc).into());
    }

    // Check for circular redirects before adding
    // If target already redirects somewhere, check the chain
    let resolve_result = Redirect::resolve(&conn, target, target_version)?;
    if resolve_result.chain.contains(&source.to_string()) {
        return Err(anyhow::anyhow!(
            "Adding this redirect would create a circular chain: {} -> {} -> {}",
            source, target, source
        ).into());
    }

    let mut redirect = Redirect::new(source.to_string(), target.to_string(), rtype);

    if let Some(ver) = source_version {
        redirect.source_version = Some(ver.to_string());
    }

    if let Some(ver) = target_version {
        redirect.target_version = Some(ver.to_string());
    }

    if let Some(msg) = message {
        redirect.message = Some(msg.to_string());
    }

    redirect.insert(&conn).context("Failed to insert redirect")?;

    let source_desc = if let Some(ver) = source_version {
        format!("{}={}", source, ver)
    } else {
        source.to_string()
    };

    let target_desc = if let Some(ver) = target_version {
        format!("{}={}", target, ver)
    } else {
        target.to_string()
    };

    println!("Created redirect: {} -> {} ({})", source_desc, target_desc, redirect_type);

    if let Some(msg) = message {
        println!("Message: {}", msg);
    }

    Ok(())
}

/// Show details of a redirect
pub fn cmd_redirect_show(
    source: &str,
    db_path: &str,
    version: Option<&str>,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

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
            let source_desc = if let Some(ver) = version {
                format!("{}={}", source, ver)
            } else {
                source.to_string()
            };
            println!("No redirect found for '{}'", source_desc);
        }
    }

    Ok(())
}

/// Remove a redirect
pub fn cmd_redirect_remove(source: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let count = Redirect::delete_by_source(&conn, source)?;

    if count > 0 {
        println!("Removed {} redirect(s) for '{}'", count, source);
    } else {
        println!("No redirect found for '{}'", source);
    }

    Ok(())
}

/// Resolve a package name through redirect chain
pub fn cmd_redirect_resolve(
    package: &str,
    db_path: &str,
    version: Option<&str>,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

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
