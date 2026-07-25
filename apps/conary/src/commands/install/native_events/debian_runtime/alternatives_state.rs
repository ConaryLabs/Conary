// apps/conary/src/commands/install/native_events/debian_runtime/alternatives_state.rs

//! Typed persistence for dpkg's update-alternatives administrative grammar.

use super::admin_projection::{absolute_package_path, ensure_plain_directories};
use crate::commands::live_root;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

const ALTERNATIVES_DIRECTORY: &str = "/etc/alternatives";
const SAVEPOINT: &str = "conary_debian_alternatives_capture";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlternativeMode {
    Auto,
    Manual,
}

impl AlternativeMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            _ => bail!("update-alternatives state has invalid mode '{value}'"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AlternativeChoice {
    priority: i32,
    slave_targets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AlternativeGroup {
    name: String,
    mode: AlternativeMode,
    master_link: String,
    selected_path: String,
    slaves: BTreeMap<String, String>,
    choices: BTreeMap<String, AlternativeChoice>,
}

pub(super) fn materialize(conn: &Connection, admin: &Path, root: &Path) -> Result<()> {
    let groups = load_groups(conn)?;
    let state_directory = admin.join("alternatives");
    fs::create_dir(&state_directory).with_context(|| {
        format!(
            "create update-alternatives state directory {}",
            state_directory.display()
        )
    })?;
    for group in groups.values() {
        fs::write(state_directory.join(&group.name), serialize_group(group)?)
            .with_context(|| format!("write update-alternatives group '{}'", group.name))?;
        fs::set_permissions(
            state_directory.join(&group.name),
            fs::Permissions::from_mode(0o644),
        )?;
        materialize_group_links(root, group)?;
    }
    Ok(())
}

pub(super) fn capture(conn: &Connection, admin: &Path, root: &Path) -> Result<()> {
    let state_directory = admin.join("alternatives");
    let metadata = fs::symlink_metadata(&state_directory).with_context(|| {
        format!(
            "inspect update-alternatives state directory {}",
            state_directory.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        bail!(
            "update-alternatives state path {} is not a plain directory",
            state_directory.display()
        );
    }

    let mut groups = BTreeMap::new();
    for entry in fs::read_dir(&state_directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("update-alternatives group name is not UTF-8"))?;
        validate_name(&name, "group")?;
        if !entry.file_type()?.is_file() {
            bail!("update-alternatives group '{}' is not a plain file", name);
        }
        let input = fs::read_to_string(entry.path())
            .with_context(|| format!("read update-alternatives group '{name}'"))?;
        let mut group = parse_group(&name, &input)?;
        group.selected_path = selected_path(root, &group)?;
        if groups.insert(name.clone(), group).is_some() {
            bail!("duplicate update-alternatives group '{name}'");
        }
    }
    persist_groups(conn, &groups)
}

fn load_groups(conn: &Connection) -> Result<BTreeMap<String, AlternativeGroup>> {
    let mut groups = BTreeMap::new();
    let mut statement = conn.prepare(
        "SELECT name, mode, master_link, selected_path
         FROM debian_alternative_groups
         ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (name, mode, master_link, selected_path) = row?;
        validate_name(&name, "group")?;
        validate_absolute_path(&master_link, "master link")?;
        validate_absolute_path(&selected_path, "selected alternative")?;
        groups.insert(
            name.clone(),
            AlternativeGroup {
                name,
                mode: AlternativeMode::parse(&mode)?,
                master_link,
                selected_path,
                slaves: BTreeMap::new(),
                choices: BTreeMap::new(),
            },
        );
    }

    load_slaves(conn, &mut groups)?;
    load_choices(conn, &mut groups)?;
    load_choice_slaves(conn, &mut groups)?;
    for group in groups.values() {
        validate_group(group)?;
    }
    Ok(groups)
}

fn load_slaves(conn: &Connection, groups: &mut BTreeMap<String, AlternativeGroup>) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT group_name, name, link
         FROM debian_alternative_slaves
         ORDER BY group_name, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (group_name, name, link) = row?;
        validate_name(&name, "slave")?;
        validate_absolute_path(&link, "slave link")?;
        let group = groups.get_mut(&group_name).with_context(|| {
            format!("update-alternatives slave references missing group '{group_name}'")
        })?;
        if group.slaves.insert(name.clone(), link).is_some() {
            bail!("duplicate slave '{name}' in alternatives group '{group_name}'");
        }
    }
    Ok(())
}

fn load_choices(conn: &Connection, groups: &mut BTreeMap<String, AlternativeGroup>) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT group_name, path, priority
         FROM debian_alternative_choices
         ORDER BY group_name, path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (group_name, path, priority) = row?;
        validate_absolute_path(&path, "alternative path")?;
        let priority = i32::try_from(priority).with_context(|| {
            format!("update-alternatives priority {priority} is out of range for '{group_name}'")
        })?;
        let group = groups.get_mut(&group_name).with_context(|| {
            format!("update-alternatives choice references missing group '{group_name}'")
        })?;
        if group
            .choices
            .insert(
                path.clone(),
                AlternativeChoice {
                    priority,
                    slave_targets: BTreeMap::new(),
                },
            )
            .is_some()
        {
            bail!("duplicate choice '{path}' in alternatives group '{group_name}'");
        }
    }
    Ok(())
}

fn load_choice_slaves(
    conn: &Connection,
    groups: &mut BTreeMap<String, AlternativeGroup>,
) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT group_name, choice_path, slave_name, target
         FROM debian_alternative_choice_slaves
         ORDER BY group_name, choice_path, slave_name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (group_name, choice_path, slave_name, target) = row?;
        if !target.is_empty() {
            validate_absolute_path(&target, "slave target")?;
        }
        let group = groups.get_mut(&group_name).with_context(|| {
            format!("update-alternatives choice slave references missing group '{group_name}'")
        })?;
        if !group.slaves.contains_key(&slave_name) {
            bail!(
                "choice '{choice_path}' references missing slave '{slave_name}' in group '{group_name}'"
            );
        }
        let choice = group.choices.get_mut(&choice_path).with_context(|| {
            format!("update-alternatives slave target references missing choice '{choice_path}'")
        })?;
        if choice
            .slave_targets
            .insert(slave_name.clone(), target)
            .is_some()
        {
            bail!("duplicate target for slave '{slave_name}' and choice '{choice_path}'");
        }
    }
    Ok(())
}

fn parse_group(name: &str, input: &str) -> Result<AlternativeGroup> {
    if input.is_empty() || !input.ends_with('\n') {
        bail!("update-alternatives group '{name}' is empty or lacks a final newline");
    }
    let lines = input.split_terminator('\n').collect::<Vec<_>>();
    let mode = AlternativeMode::parse(
        lines
            .first()
            .copied()
            .context("update-alternatives group has no mode")?,
    )?;
    let master_link = lines
        .get(1)
        .copied()
        .context("update-alternatives group has no master link")?;
    validate_absolute_path(master_link, "master link")?;

    let mut index = 2;
    let mut slaves = BTreeMap::new();
    let mut slave_links = BTreeSet::new();
    while lines.get(index).is_some_and(|line| !line.is_empty()) {
        let slave_name = lines[index];
        let slave_link = lines
            .get(index + 1)
            .copied()
            .context("update-alternatives slave has no link")?;
        validate_name(slave_name, "slave")?;
        validate_absolute_path(slave_link, "slave link")?;
        if slaves
            .insert(slave_name.to_string(), slave_link.to_string())
            .is_some()
        {
            bail!("duplicate slave '{slave_name}' in alternatives group '{name}'");
        }
        if !slave_links.insert(slave_link) {
            bail!("duplicate slave link '{slave_link}' in alternatives group '{name}'");
        }
        index += 2;
    }
    require_blank(&lines, &mut index, name, "slave list")?;

    let mut choices = BTreeMap::new();
    while lines.get(index).is_some_and(|line| !line.is_empty()) {
        let path = lines[index];
        validate_absolute_path(path, "alternative path")?;
        let priority = lines
            .get(index + 1)
            .copied()
            .context("update-alternatives choice has no priority")?
            .parse::<i32>()
            .with_context(|| format!("update-alternatives choice '{path}' has invalid priority"))?;
        index += 2;
        let mut slave_targets = BTreeMap::new();
        for slave_name in slaves.keys() {
            let target = lines
                .get(index)
                .copied()
                .context("update-alternatives choice has incomplete slave targets")?;
            if !target.is_empty() {
                validate_absolute_path(target, "slave target")?;
            }
            slave_targets.insert(slave_name.clone(), target.to_string());
            index += 1;
        }
        if choices
            .insert(
                path.to_string(),
                AlternativeChoice {
                    priority,
                    slave_targets,
                },
            )
            .is_some()
        {
            bail!("duplicate choice '{path}' in alternatives group '{name}'");
        }
    }
    require_blank(&lines, &mut index, name, "choice list")?;
    if index != lines.len() {
        bail!("update-alternatives group '{name}' has trailing records");
    }
    let mut group = AlternativeGroup {
        name: name.to_string(),
        mode,
        master_link: master_link.to_string(),
        selected_path: String::new(),
        slaves,
        choices,
    };
    group.selected_path = group.choices.keys().next().cloned().unwrap_or_default();
    validate_group(&group)?;
    Ok(group)
}

fn require_blank(lines: &[&str], index: &mut usize, group: &str, section: &str) -> Result<()> {
    if lines.get(*index) != Some(&"") {
        bail!("update-alternatives group '{group}' has no blank line after {section}");
    }
    *index += 1;
    Ok(())
}

fn serialize_group(group: &AlternativeGroup) -> Result<String> {
    validate_group(group)?;
    let mut output = String::new();
    push_line(&mut output, group.mode.as_str())?;
    push_line(&mut output, &group.master_link)?;
    for (name, link) in &group.slaves {
        push_line(&mut output, name)?;
        push_line(&mut output, link)?;
    }
    push_line(&mut output, "")?;
    for (path, choice) in &group.choices {
        push_line(&mut output, path)?;
        push_line(&mut output, &choice.priority.to_string())?;
        for slave_name in group.slaves.keys() {
            push_line(
                &mut output,
                choice
                    .slave_targets
                    .get(slave_name)
                    .map(String::as_str)
                    .unwrap_or(""),
            )?;
        }
    }
    push_line(&mut output, "")?;
    Ok(output)
}

fn push_line(output: &mut String, value: &str) -> Result<()> {
    if value.contains('\n') {
        bail!("newline cannot be represented in update-alternatives state");
    }
    output.push_str(value);
    output.push('\n');
    Ok(())
}

fn validate_group(group: &AlternativeGroup) -> Result<()> {
    validate_name(&group.name, "group")?;
    validate_absolute_path(&group.master_link, "master link")?;
    validate_absolute_path(&group.selected_path, "selected alternative")?;
    if group.choices.is_empty() {
        bail!("update-alternatives group '{}' has no choices", group.name);
    }
    if !group.choices.contains_key(&group.selected_path) {
        bail!(
            "selected alternative '{}' is not a choice in group '{}'",
            group.selected_path,
            group.name
        );
    }
    let mut links = BTreeSet::from([group.master_link.as_str()]);
    for (name, link) in &group.slaves {
        validate_name(name, "slave")?;
        validate_absolute_path(link, "slave link")?;
        if name == &group.name {
            bail!("alternatives group '{}' is also a slave name", group.name);
        }
        if !links.insert(link) {
            bail!(
                "alternatives group '{}' has duplicate link '{link}'",
                group.name
            );
        }
    }
    for (path, choice) in &group.choices {
        validate_absolute_path(path, "alternative path")?;
        if path == &group.master_link {
            bail!("alternatives choice '{path}' is also the master link");
        }
        if choice.slave_targets.len() != group.slaves.len() {
            bail!("alternatives choice '{path}' has an incomplete slave set");
        }
        for (slave, target) in &choice.slave_targets {
            let link = group.slaves.get(slave).with_context(|| {
                format!("alternatives choice '{path}' names unknown slave '{slave}'")
            })?;
            if !target.is_empty() {
                validate_absolute_path(target, "slave target")?;
                if target == link {
                    bail!("slave link '{link}' is also its alternative target");
                }
            }
        }
    }
    Ok(())
}

fn validate_name(name: &str, kind: &str) -> Result<()> {
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| matches!(byte, b'/' | b' ' | b'\t' | b'\n' | b'\r' | 0))
    {
        bail!("invalid update-alternatives {kind} name '{name}'");
    }
    Ok(())
}

fn validate_absolute_path(path: &str, kind: &str) -> Result<()> {
    let normalized = absolute_package_path(path)
        .with_context(|| format!("invalid update-alternatives {kind} '{path}'"))?;
    if normalized != path || path.contains('\n') || path.contains('\r') {
        bail!("update-alternatives {kind} '{path}' is not a canonical absolute path");
    }
    Ok(())
}

fn selected_path(root: &Path, group: &AlternativeGroup) -> Result<String> {
    let alternatives = live_root::target_path(root, ALTERNATIVES_DIRECTORY)?;
    let link = alternatives.join(&group.name);
    let target = fs::read_link(&link).with_context(|| {
        format!(
            "read selected alternative link {} for group '{}'",
            link.display(),
            group.name
        )
    })?;
    let target = target.to_str().with_context(|| {
        format!(
            "selected alternative link {} has a non-UTF-8 target",
            link.display()
        )
    })?;
    validate_absolute_path(target, "selected alternative")?;
    if !group.choices.contains_key(target) {
        bail!(
            "selected alternative '{target}' is not recorded in group '{}'",
            group.name
        );
    }
    Ok(target.to_string())
}

fn materialize_group_links(root: &Path, group: &AlternativeGroup) -> Result<()> {
    validate_group(group)?;
    let alternatives = live_root::target_path(root, ALTERNATIVES_DIRECTORY)?;
    ensure_plain_directories(root, &alternatives)?;
    replace_symlink(root, &alternatives.join(&group.name), &group.selected_path)?;
    replace_symlink(
        root,
        &live_root::target_path(root, &group.master_link)?,
        &format!("{ALTERNATIVES_DIRECTORY}/{}", group.name),
    )?;

    let selected = group
        .choices
        .get(&group.selected_path)
        .context("validated alternatives group lost its selected choice")?;
    for (name, link) in &group.slaves {
        let alternative_link = alternatives.join(name);
        let generic_link = live_root::target_path(root, link)?;
        match selected.slave_targets.get(name).map(String::as_str) {
            Some(target) if !target.is_empty() => {
                replace_symlink(root, &alternative_link, target)?;
                replace_symlink(
                    root,
                    &generic_link,
                    &format!("{ALTERNATIVES_DIRECTORY}/{name}"),
                )?;
            }
            _ => {
                remove_symlink_if_present(&alternative_link)?;
                remove_symlink_if_present(&generic_link)?;
            }
        }
    }
    Ok(())
}

fn replace_symlink(root: &Path, link: &Path, target: &str) -> Result<()> {
    let parent = link
        .parent()
        .with_context(|| format!("alternatives link {} has no parent", link.display()))?;
    ensure_plain_directories(root, parent)?;
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(link)? == Path::new(target) {
                return Ok(());
            }
            fs::remove_file(link)?;
        }
        Ok(_) => bail!(
            "cannot project update-alternatives link {} over a non-symlink",
            link.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    symlink(target, link).with_context(|| {
        format!(
            "create update-alternatives link {} -> {target}",
            link.display()
        )
    })
}

fn remove_symlink_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => bail!(
            "cannot remove update-alternatives path {} because it is not a symlink",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn persist_groups(conn: &Connection, groups: &BTreeMap<String, AlternativeGroup>) -> Result<()> {
    conn.execute_batch(&format!("SAVEPOINT {SAVEPOINT}"))?;
    let result = (|| -> Result<()> {
        for table in [
            "debian_alternative_choice_slaves",
            "debian_alternative_choices",
            "debian_alternative_slaves",
            "debian_alternative_groups",
        ] {
            conn.execute(&format!("DELETE FROM {table}"), [])?;
        }
        for group in groups.values() {
            validate_group(group)?;
            conn.execute(
                "INSERT INTO debian_alternative_groups
                    (name, mode, master_link, selected_path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    &group.name,
                    group.mode.as_str(),
                    &group.master_link,
                    &group.selected_path,
                ],
            )?;
            for (name, link) in &group.slaves {
                conn.execute(
                    "INSERT INTO debian_alternative_slaves
                        (group_name, name, link)
                     VALUES (?1, ?2, ?3)",
                    params![&group.name, name, link],
                )?;
            }
            for (path, choice) in &group.choices {
                conn.execute(
                    "INSERT INTO debian_alternative_choices
                        (group_name, path, priority)
                     VALUES (?1, ?2, ?3)",
                    params![&group.name, path, choice.priority],
                )?;
                for (slave_name, target) in &choice.slave_targets {
                    conn.execute(
                        "INSERT INTO debian_alternative_choice_slaves
                            (group_name, choice_path, slave_name, target)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![&group.name, path, slave_name, target],
                    )?;
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {SAVEPOINT}; RELEASE SAVEPOINT {SAVEPOINT}"
            ));
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "alternatives_state/tests.rs"]
mod tests;
