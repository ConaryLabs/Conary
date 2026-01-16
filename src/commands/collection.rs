// src/commands/collection.rs
//! Collection management commands

use anyhow::Result;
use conary::scriptlet::SandboxMode;
use tracing::info;

/// Create a new collection
pub fn cmd_collection_create(
    name: &str,
    description: Option<&str>,
    members: &[String],
    db_path: &str,
) -> Result<()> {
    info!("Creating collection: {}", name);
    let mut conn = conary::db::open(db_path)?;

    // Check if collection already exists
    let existing = conary::db::models::Trove::find_by_name(&conn, name)?;
    if !existing.is_empty() {
        return Err(anyhow::anyhow!(
            "A package or collection named '{}' already exists",
            name
        ));
    }

    conary::db::transaction(&mut conn, |tx| {
        // Create the collection as a trove
        let mut trove = conary::db::models::Trove::new(
            name.to_string(),
            "1.0".to_string(),
            conary::db::models::TroveType::Collection,
        );
        trove.description = description.map(|s| s.to_string());
        let collection_id = trove.insert(tx)?;

        // Add members
        for member_name in members {
            let mut member =
                conary::db::models::CollectionMember::new(collection_id, member_name.clone());
            member.insert(tx)?;
        }

        Ok(())
    })?;

    println!("Created collection: {}", name);
    if let Some(desc) = description {
        println!("  Description: {}", desc);
    }
    if !members.is_empty() {
        println!("  Members: {}", members.join(", "));
    }

    Ok(())
}

/// List all collections
pub fn cmd_collection_list(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find all troves with type 'collection'
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description FROM troves WHERE type = 'collection' ORDER BY name",
    )?;

    let collections: Vec<(i64, String, String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if collections.is_empty() {
        println!("No collections found.");
        println!("\nCreate a collection with: conary collection-create <name> --members pkg1,pkg2,...");
        return Ok(());
    }

    println!("Collections:");
    for (id, name, version, description) in &collections {
        let members = conary::db::models::CollectionMember::find_by_collection(&conn, *id)?;
        print!("  {} v{}", name, version);
        if let Some(desc) = description {
            print!(" - {}", desc);
        }
        println!(" ({} members)", members.len());
    }

    println!("\nTotal: {} collection(s)", collections.len());
    Ok(())
}

/// Show details of a collection
pub fn cmd_collection_show(name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, name)?;
    let trove = troves
        .iter()
        .find(|t| t.trove_type == conary::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = trove.id.ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;
    let members = conary::db::models::CollectionMember::find_by_collection(&conn, collection_id)?;

    println!("Collection: {} v{}", trove.name, trove.version);
    if let Some(desc) = &trove.description {
        println!("Description: {}", desc);
    }
    println!("\nMembers ({}):", members.len());

    for member in &members {
        print!("  {}", member.member_name);
        if let Some(ver) = &member.member_version {
            print!(" ({})", ver);
        }
        if member.is_optional {
            print!(" [optional]");
        }

        // Check if member is installed
        let installed = conary::db::models::Trove::find_by_name(&conn, &member.member_name)?;
        if installed.is_empty() {
            print!(" [not installed]");
        } else {
            print!(" [installed: {}]", installed[0].version);
        }
        println!();
    }

    Ok(())
}

/// Add members to a collection
pub fn cmd_collection_add(name: &str, members: &[String], db_path: &str) -> Result<()> {
    info!("Adding members to collection: {}", name);
    let mut conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, name)?;
    let trove = troves
        .iter()
        .find(|t| t.trove_type == conary::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = trove.id.ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;

    conary::db::transaction(&mut conn, |tx| {
        for member_name in members {
            // Check if already a member
            if conary::db::models::CollectionMember::is_member(tx, collection_id, member_name)? {
                println!("  {} is already a member, skipping", member_name);
                continue;
            }
            let mut member =
                conary::db::models::CollectionMember::new(collection_id, member_name.clone());
            member.insert(tx)?;
            println!("  Added: {}", member_name);
        }
        Ok(())
    })?;

    println!("\nUpdated collection '{}'", name);
    Ok(())
}

/// Remove members from a collection
pub fn cmd_collection_remove_member(name: &str, members: &[String], db_path: &str) -> Result<()> {
    info!("Removing members from collection: {}", name);
    let mut conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, name)?;
    let trove = troves
        .iter()
        .find(|t| t.trove_type == conary::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = trove.id.ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;

    conary::db::transaction(&mut conn, |tx| {
        for member_name in members {
            if let Some(member) =
                conary::db::models::CollectionMember::find_member(tx, collection_id, member_name)?
            {
                if let Some(id) = member.id {
                    conary::db::models::CollectionMember::delete(tx, id)?;
                    println!("  Removed: {}", member_name);
                }
            } else {
                println!("  {} is not a member, skipping", member_name);
            }
        }
        Ok(())
    })?;

    println!("\nUpdated collection '{}'", name);
    Ok(())
}

/// Delete a collection
pub fn cmd_collection_delete(name: &str, db_path: &str) -> Result<()> {
    info!("Deleting collection: {}", name);
    let mut conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, name)?;
    let trove = troves
        .iter()
        .find(|t| t.trove_type == conary::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;

    conary::db::transaction(&mut conn, |tx| {
        // Members will be cascade deleted due to FK constraint
        conary::db::models::Trove::delete(tx, trove_id)?;
        Ok(())
    })?;

    println!("Deleted collection: {}", name);
    Ok(())
}

/// Install all packages in a collection
pub fn cmd_collection_install(
    name: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    skip_optional: bool,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    info!("Installing collection: {}", name);
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, name)?;
    let trove = troves
        .iter()
        .find(|t| t.trove_type == conary::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = trove.id.ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;
    let members = conary::db::models::CollectionMember::find_by_collection(&conn, collection_id)?;

    if members.is_empty() {
        println!("Collection '{}' has no members.", name);
        return Ok(());
    }

    // Filter out optional members if requested
    let members_to_install: Vec<_> = members
        .iter()
        .filter(|m| !skip_optional || !m.is_optional)
        .collect();

    println!(
        "Collection '{}' contains {} package(s) to install:",
        name,
        members_to_install.len()
    );
    for member in &members_to_install {
        print!("  {}", member.member_name);
        if member.is_optional {
            print!(" [optional]");
        }
        // Check if already installed
        let installed = conary::db::models::Trove::find_by_name(&conn, &member.member_name)?;
        if !installed.is_empty() {
            print!(" (already installed: {})", installed[0].version);
        }
        println!();
    }

    if dry_run {
        println!("\nDry run - no packages will be installed.");
        return Ok(());
    }

    // Drop the connection before calling cmd_install
    drop(conn);

    // Install each member that isn't already installed
    let mut installed_count = 0;
    let mut skipped_count = 0;
    let mut failed_count = 0;

    for member in &members_to_install {
        // Re-check if installed (connection was dropped)
        let conn = conary::db::open(db_path)?;
        let installed = conary::db::models::Trove::find_by_name(&conn, &member.member_name)?;
        drop(conn);

        if !installed.is_empty() {
            skipped_count += 1;
            continue;
        }

        println!("\nInstalling {}...", member.member_name);
        let reason = format!("Installed via @{}", name);
        match super::cmd_install(
            &member.member_name,
            db_path,
            root,
            member.member_version.clone(),
            None,
            false,
            false,
            false,
            Some(&reason),
            sandbox_mode,
            false,  // allow_downgrade
            false,  // convert_to_ccs
            None,   // refinery
            None,   // distro
        ) {
            Ok(()) => {
                installed_count += 1;
            }
            Err(e) => {
                eprintln!("  Failed to install {}: {}", member.member_name, e);
                failed_count += 1;
            }
        }
    }

    println!("\nCollection install complete:");
    println!("  Installed: {} package(s)", installed_count);
    println!("  Already installed: {} package(s)", skipped_count);
    if failed_count > 0 {
        println!("  Failed: {} package(s)", failed_count);
    }

    Ok(())
}
