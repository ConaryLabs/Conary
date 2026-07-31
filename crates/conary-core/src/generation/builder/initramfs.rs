// conary-core/src/generation/builder/initramfs.rs

use std::path::Path;

use super::kernel::{kernel_module_dir, regular_file_exists};

pub(super) const CONARY_DRACUT_MODULE_SETUP: &str =
    include_str!("../../../../../packaging/dracut/90conary/module-setup.sh");
const CONARY_DRACUT_INIT: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-init.sh");
const CONARY_DRACUT_GENERATOR: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-generator.sh");
pub(super) const RUNTIME_DRACUT_ADD_MODULES: &str = "conary";
pub(super) const RUNTIME_DRACUT_OMIT_MODULES: &str = "systemd";

pub(super) fn generate_runtime_initramfs(
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
    system_root: &Path,
    release: &str,
    initramfs: &Path,
) -> crate::Result<()> {
    let Some(parent) = initramfs.parent() else {
        return Err(crate::error::Error::InvalidPath(format!(
            "initramfs destination has no parent: {}",
            initramfs.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    ensure_initramfs_tool_available(cpio, "cpio")?;
    ensure_kernel_module_metadata(depmod, system_root, release)?;
    let (runtime_module_dir, _module_dir_arg) =
        kernel_module_dir(system_root, release).ok_or_else(|| {
            crate::error::Error::NotFound(format!(
                "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
            ))
        })?;

    let modules_workspace = tempfile::Builder::new()
        .prefix("conary-dracut-")
        .tempdir()
        .map_err(|e| {
            crate::error::Error::IoError(format!("failed to create dracut workspace: {e}"))
        })?;
    prepare_dracut_workspace(modules_workspace.path())?;
    let module_dir = modules_workspace.path().join("modules.d/90conary");
    std::fs::create_dir_all(&module_dir)?;
    write_dracut_module_file(
        &module_dir.join("module-setup.sh"),
        CONARY_DRACUT_MODULE_SETUP,
    )?;
    write_dracut_module_file(&module_dir.join("conary-init.sh"), CONARY_DRACUT_INIT)?;
    write_dracut_module_file(
        &module_dir.join("conary-generator.sh"),
        CONARY_DRACUT_GENERATOR,
    )?;

    let output = std::process::Command::new(dracut)
        .env("dracutbasedir", modules_workspace.path())
        .arg("--force")
        .arg("--no-hostonly")
        // Force dracut's shell init path. The default systemd module alone
        // creates a partial initramfs without the initrd systemd contract.
        .arg("--omit")
        .arg(RUNTIME_DRACUT_OMIT_MODULES)
        .arg("--add")
        .arg(RUNTIME_DRACUT_ADD_MODULES)
        .arg("--sysroot")
        .arg(system_root)
        .arg("--kmoddir")
        .arg(&runtime_module_dir)
        .arg(initramfs)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run dracut to generate {} for {release}: {e}",
                initramfs.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "dracut failed to generate {} for {release} with status {}:\nstdout:\n{}\nstderr:\n{}",
            initramfs.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

fn ensure_initramfs_tool_available(tool: &Path, name: &str) -> crate::Result<()> {
    match std::process::Command::new(tool).arg("--version").output() {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(crate::error::Error::NotFound(format!(
                "missing required initramfs tool {name} at {}; source images that build runtime generations must include the initramfs toolchain because dracut emits initramfs archives through {name}",
                tool.display()
            )))
        }
        Err(e) => Err(crate::error::Error::IoError(format!(
            "failed to check required initramfs tool {name} at {}: {e}",
            tool.display()
        ))),
    }
}

fn prepare_dracut_workspace(workspace: &Path) -> crate::Result<()> {
    let modules_dir = workspace.join("modules.d");
    std::fs::create_dir_all(&modules_dir)?;

    let system_dracut = Path::new("/usr/lib/dracut");
    if !system_dracut.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(system_dracut)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "modules.d" {
            continue;
        }
        link_or_copy_dracut_entry(&entry.path(), &workspace.join(name))?;
    }

    let system_modules = system_dracut.join("modules.d");
    if system_modules.is_dir() {
        for entry in std::fs::read_dir(system_modules)? {
            let entry = entry?;
            link_or_copy_dracut_entry(&entry.path(), &modules_dir.join(entry.file_name()))?;
        }
    }

    Ok(())
}

fn link_or_copy_dracut_entry(source: &Path, dest: &Path) -> crate::Result<()> {
    if dest.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, dest)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        if source.is_file() {
            std::fs::copy(source, dest)?;
        }
        Ok(())
    }
}

fn ensure_kernel_module_metadata(
    depmod: &Path,
    system_root: &Path,
    release: &str,
) -> crate::Result<()> {
    let (module_dir, module_dir_arg) = kernel_module_dir(system_root, release).ok_or_else(|| {
        crate::error::Error::NotFound(format!(
            "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
        ))
    })?;
    let modules_dep = module_dir.join("modules.dep");
    if regular_file_exists(&modules_dep) {
        return Ok(());
    }

    let output = std::process::Command::new(depmod)
        .arg("-b")
        .arg(system_root)
        .arg("-m")
        .arg(module_dir_arg)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run depmod for kernel {release} under {}: {e}",
                system_root.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "depmod failed for kernel {release} under {} with status {}:\nstdout:\n{}\nstderr:\n{}",
            system_root.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    if !regular_file_exists(&modules_dep) {
        return Err(crate::error::Error::NotFound(format!(
            "depmod completed but did not create {}",
            modules_dep.display()
        )));
    }

    Ok(())
}

fn write_dracut_module_file(path: &Path, contents: &str) -> crate::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tree_sitter::{Node, Parser};

    use super::*;
    use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;

    #[test]
    fn conary_runtime_scripts_only_invoke_declared_module_tools() {
        let declared = declared_module_tools();

        for (name, script) in [
            ("conary-init", CONARY_DRACUT_INIT),
            ("conary-generator", CONARY_DRACUT_GENERATOR),
        ] {
            let functions = shell_function_names(script);
            let invocations =
                extract_invocations_from_shell_text(name, script, Some("initramfs")).unwrap();
            let missing = invocations
                .iter()
                .map(|invocation| invocation.command.as_str())
                .filter(|command| {
                    !shell_builtin(command)
                        && !functions.contains(*command)
                        && !declared.contains(*command)
                })
                .collect::<BTreeSet<_>>();

            assert!(
                missing.is_empty(),
                "{name} directly invokes initramfs tools that the Conary dracut module does not declare: {missing:?}"
            );
        }
    }

    fn declared_module_tools() -> BTreeSet<String> {
        let invocations = extract_invocations_from_shell_text(
            "conary-module-setup",
            CONARY_DRACUT_MODULE_SETUP,
            Some("initramfs-build"),
        )
        .unwrap();
        let mut declared = invocations
            .iter()
            .filter(|invocation| invocation.command == "inst_multiple")
            .flat_map(|invocation| invocation.argv.iter())
            .filter(|argument| !argument.starts_with('-'))
            .map(|argument| argument.rsplit('/').next().unwrap_or_default().to_string())
            .collect::<BTreeSet<_>>();
        declared.extend(
            invocations
                .iter()
                .filter(|invocation| invocation.command == "install_conary_script")
                .filter_map(|invocation| invocation.argv.get(1))
                .map(|destination| {
                    destination
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_string()
                }),
        );
        declared
    }

    fn shell_function_names(script: &str) -> BTreeSet<String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(script, None).unwrap();
        assert!(!tree.root_node().has_error(), "invalid shell fixture");
        let mut names = BTreeSet::new();
        collect_function_names(tree.root_node(), script, &mut names);
        names
    }

    fn collect_function_names(node: Node<'_>, script: &str, names: &mut BTreeSet<String>) {
        if node.kind() == "function_definition"
            && let Some(name) = node.child_by_field_name("name")
        {
            names.insert(
                name.utf8_text(script.as_bytes())
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_function_names(child, script, names);
        }
    }

    fn shell_builtin(command: &str) -> bool {
        matches!(
            command,
            "!" | ":"
                | "["
                | "break"
                | "command"
                | "continue"
                | "echo"
                | "exec"
                | "exit"
                | "export"
                | "local"
                | "printf"
                | "read"
                | "return"
                | "set"
                | "shift"
                | "test"
                | "true"
                | "unset"
        )
    }
}
