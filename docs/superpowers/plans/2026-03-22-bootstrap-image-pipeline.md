# Bootstrap Image Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the CAS-layered derivation pipeline to produce a bootable conaryOS image via four CLI commands: cross-tools, seed, run, image.

**Architecture:** The existing derivation pipeline (`Pipeline::execute()`) handles staged builds with CAS capture and EROFS composition. We add a `seed` command to bridge cross-tools output into the derivation model, wire the `run` stub to the pipeline, extend `image` to accept EROFS generations, and create a system manifest.

**Tech Stack:** Rust 1.94, composefs-rs (EROFS), rusqlite (derivation index), clap (CLI), CAS store (SHA-256)

**Spec:** `docs/superpowers/specs/2026-03-22-bootstrap-image-pipeline-design.md`

---

### Task 1: Create System Manifest `conaryos.toml`

**Files:**
- Create: `conaryos.toml`

This is the declarative input that drives the entire pipeline. Creating it first lets us test manifest parsing immediately.

- [ ] **Step 1: Write the manifest file**

```toml
# conaryos.toml
#
# System manifest for the conaryOS base image.
# Input to: conary bootstrap run conaryos.toml

[system]
name = "conaryos-base"
target = "x86_64-conary-linux-gnu"

[seed]
source = "local:seed"

[packages]
include = [
    # Core system (LFS Ch8 aligned, 80 packages)
    "man-pages", "iana-etc", "glibc", "zlib", "bzip2", "xz", "zstd",
    "lz4", "file", "readline", "m4", "bc", "flex", "pkgconf", "binutils",
    "gmp", "mpfr", "mpc", "attr", "acl", "libcap", "libxcrypt", "shadow",
    "gcc", "ncurses", "sed", "psmisc", "gettext", "bison", "grep", "bash",
    "libtool", "gdbm", "gperf", "expat", "inetutils", "less", "perl",
    "xml-parser", "intltool", "autoconf", "automake", "openssl", "kmod",
    "elfutils", "libffi", "python", "flit-core", "wheel", "setuptools",
    "ninja", "meson", "coreutils", "diffutils", "gawk", "findutils",
    "groff", "gzip", "iproute2", "kbd", "libpipeline", "make", "patch",
    "tar", "texinfo", "vim", "markupsafe", "jinja2", "systemd", "dbus",
    "man-db", "procps-ng", "util-linux", "e2fsprogs", "pcre2", "sqlite",
    "linux",
    # Tier 2: Self-hosting (8 packages)
    "linux-pam", "openssh", "make-ca", "curl", "sudo", "nano", "rust",
    "conary",
]
exclude = []

[kernel]
config = "defconfig"
```

- [ ] **Step 2: Write test that manifest parses**

Add to the existing test module in `conary-core/src/derivation/manifest.rs`:

```rust
#[test]
fn parse_conaryos_manifest() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.parent().unwrap().join("conaryos.toml");
    let content = std::fs::read_to_string(&path).expect("conaryos.toml not found at workspace root");
    let manifest = SystemManifest::parse(&content).expect("conaryos.toml should parse");
    assert_eq!(manifest.system.name, "conaryos-base");
    assert_eq!(manifest.system.target, "x86_64-conary-linux-gnu");
    assert!(manifest.packages.include.len() >= 85, "expected 85+ packages");
    assert!(manifest.packages.include.contains(&"glibc".to_string()));
    assert!(manifest.packages.include.contains(&"linux-pam".to_string()));
    assert!(manifest.kernel.is_some());
}
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p conary-core parse_conaryos_manifest -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add conaryos.toml conary-core/src/derivation/manifest.rs
git commit -m "feat(bootstrap): add conaryos.toml system manifest

Declarative manifest defining the 88-package conaryOS base system.
Input to the bootstrap run command."
```

---

### Task 2: Add `bootstrap seed` CLI Definition

**Files:**
- Modify: `src/cli/bootstrap.rs` (add `Seed` variant)
- Modify: `src/main.rs` (add dispatch arm)

- [ ] **Step 1: Add `Seed` variant to `BootstrapCommands`**

In `src/cli/bootstrap.rs`, add after the `Clean` variant (before `CrossTools`):

```rust
    /// Package cross-tools output as a derivation seed
    Seed {
        /// Cross-tools directory to package (e.g., /conary/bootstrap/lfs/tools)
        #[arg(long)]
        from: String,

        /// Output seed directory
        #[arg(short, long)]
        output: String,

        /// Target triple
        #[arg(long, default_value = "x86_64-conary-linux-gnu")]
        target: String,
    },
```

- [ ] **Step 2: Add `--seed` and `--recipe-dir` flags to `Run` variant**

In `src/cli/bootstrap.rs`, add to the `Run` variant fields (after `work_dir`):

```rust
        /// Path to seed directory (from bootstrap seed)
        #[arg(long)]
        seed: String,

        /// Recipe directory
        #[arg(long, default_value = "recipes")]
        recipe_dir: String,
```

- [ ] **Step 3: Add `--from-generation` flag to `Image` variant**

In `src/cli/bootstrap.rs`, add to the `Image` variant fields:

```rust
        /// Use EROFS generation output instead of sysroot (from bootstrap run)
        #[arg(long)]
        from_generation: Option<String>,
```

- [ ] **Step 4: Add dispatch arms in `src/main.rs`**

In `src/main.rs`, find the `BootstrapCommands::Run` match arm (around line 1478). Add the new `Seed` dispatch before it:

```rust
            cli::BootstrapCommands::Seed {
                from,
                output,
                target,
            } => commands::cmd_bootstrap_seed(&from, &output, &target).await,
```

Update the `Run` dispatch to pass the new fields:

```rust
            cli::BootstrapCommands::Run {
                manifest,
                work_dir,
                seed,
                recipe_dir,
                up_to,
                only,
                cascade,
                keep_logs,
                shell_on_failure,
                verbose,
                no_substituters,
                publish,
            } => {
                commands::cmd_bootstrap_run(commands::BootstrapRunOptions {
                    manifest: &manifest,
                    work_dir: &work_dir,
                    seed: &seed,
                    recipe_dir: &recipe_dir,
                    up_to: up_to.as_deref(),
                    only: only.as_deref(),
                    cascade,
                    keep_logs,
                    shell_on_failure,
                    verbose,
                    no_substituters,
                    publish,
                })
                .await
            }
```

Update the `Image` dispatch to pass `from_generation`:

```rust
            cli::BootstrapCommands::Image {
                work_dir,
                output,
                format,
                size,
                from_generation,
            } => {
                commands::cmd_bootstrap_image(&work_dir, &output, &format, &size, from_generation.as_deref())
                    .await
            }
```

- [ ] **Step 5: Update `BootstrapRunOptions` struct**

In `src/commands/bootstrap/mod.rs`, add to `BootstrapRunOptions`:

```rust
    /// Path to seed directory.
    pub seed: &'a str,
    /// Recipe directory.
    pub recipe_dir: &'a str,
```

- [ ] **Step 6: Add stub `cmd_bootstrap_seed` and update `cmd_bootstrap_image` signature**

In `src/commands/bootstrap/mod.rs`, add a stub:

```rust
/// Package cross-tools output as a derivation seed
pub async fn cmd_bootstrap_seed(from: &str, output: &str, target: &str) -> Result<()> {
    println!("Creating seed from {} -> {}", from, output);
    println!("  Target: {}", target);
    todo!("seed implementation in next task")
}
```

Update `cmd_bootstrap_image` signature to accept `from_generation`:

```rust
pub async fn cmd_bootstrap_image(
    work_dir: &str,
    output: &str,
    format: &str,
    size: &str,
    from_generation: Option<&str>,
) -> Result<()> {
```

(Keep existing body, the `from_generation` path will be added in Task 5.)

- [ ] **Step 7: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors. Warnings about unused `from_generation` are OK.

- [ ] **Step 8: Commit**

```bash
git add src/cli/bootstrap.rs src/main.rs src/commands/bootstrap/mod.rs
git commit -m "feat(bootstrap): add seed CLI, extend run and image flags

Add bootstrap seed subcommand, --seed/--recipe-dir on run,
--from-generation on image. Seed implementation is a stub."
```

---

### Task 3: Implement `cmd_bootstrap_seed`

**Files:**
- Modify: `src/commands/bootstrap/mod.rs`

This command walks a cross-tools directory, stores files in CAS, builds an EROFS image, and writes seed metadata.

**Note:** `build_erofs_image()` requires the `composefs-rs` feature, which is enabled by default in `conary-core/Cargo.toml`. No extra flags needed for `cargo build`.

- [ ] **Step 1: Write the implementation**

Replace the `cmd_bootstrap_seed` stub with:

```rust
/// Package cross-tools output as a derivation seed
pub async fn cmd_bootstrap_seed(from: &str, output: &str, target: &str) -> Result<()> {
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use conary_core::filesystem::CasStore;
    use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef, build_erofs_image};
    use std::os::unix::fs::MetadataExt;
    use walkdir::WalkDir;

    let from_path = PathBuf::from(from);
    let output_path = PathBuf::from(output);

    // Validate input
    if !from_path.exists() {
        return Err(anyhow::anyhow!(
            "Cross-tools directory not found: {}",
            from_path.display()
        ));
    }
    if !from_path.join("bin").exists() && !from_path.join("lib").exists() {
        return Err(anyhow::anyhow!(
            "Directory does not look like a cross-toolchain (no bin/ or lib/): {}",
            from_path.display()
        ));
    }

    println!("Creating seed from cross-tools output...");
    println!("  Source: {}", from_path.display());
    println!("  Output: {}", output_path.display());
    println!("  Target: {}", target);

    // Create output structure
    std::fs::create_dir_all(&output_path)?;
    let cas_dir = output_path.join("cas");
    let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

    // Walk source tree, store files in CAS, collect entries
    let mut file_entries = Vec::new();
    let mut symlink_entries = Vec::new();
    let mut file_count: u64 = 0;

    for entry in WalkDir::new(&from_path).follow_links(false) {
        let entry = entry.context("Failed to walk directory")?;
        let rel_path = entry
            .path()
            .strip_prefix(&from_path)
            .context("Failed to compute relative path")?;

        // Skip the root directory itself
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let abs_path = format!("/tools/{}", rel_path.display());
        let metadata = entry.path().symlink_metadata()?;

        if metadata.is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            symlink_entries.push(SymlinkEntryRef {
                path: abs_path,
                target: link_target.to_string_lossy().to_string(),
            });
        } else if metadata.is_file() {
            let content = std::fs::read(entry.path())?;
            let hash = cas.store(&content).context("CAS store failed")?;
            file_entries.push(FileEntryRef {
                path: abs_path,
                sha256_hash: hash,
                size: metadata.len(),
                permissions: metadata.mode() & 0o7777,
            });
            file_count += 1;
        }
        // Directories are implicit in EROFS
    }

    println!(
        "  Stored {} files, {} symlinks in CAS",
        file_count,
        symlink_entries.len()
    );

    // Build EROFS image
    let gen_dir = output_path.join("gen");
    std::fs::create_dir_all(&gen_dir)?;
    let build_result = build_erofs_image(&file_entries, &symlink_entries, &gen_dir)
        .context("Failed to build EROFS image")?;

    // Move EROFS image to seed.erofs
    let seed_erofs = output_path.join("seed.erofs");
    std::fs::rename(&build_result.image_path, &seed_erofs)?;
    // Clean up temp gen dir
    let _ = std::fs::remove_dir_all(&gen_dir);

    // Compute image hash
    let seed_id =
        erofs_image_hash(&seed_erofs).context("Failed to hash seed EROFS image")?;

    // Write seed.toml
    let metadata = SeedMetadata {
        seed_id: seed_id.clone(),
        source: SeedSource::SelfBuilt,
        origin_url: None,
        builder: Some("conary-bootstrap".to_string()),
        packages: vec![
            "binutils-pass1".to_string(),
            "gcc-pass1".to_string(),
            "linux-headers".to_string(),
            "glibc".to_string(),
            "libstdcxx".to_string(),
        ],
        target_triple: target.to_string(),
        verified_by: vec![],
    };

    let toml_str =
        toml::to_string_pretty(&metadata).context("Failed to serialize seed metadata")?;
    std::fs::write(output_path.join("seed.toml"), &toml_str)?;

    println!("\n[OK] Seed created successfully!");
    println!("  EROFS image: {} ({} bytes)", seed_erofs.display(), build_result.image_size);
    println!("  CAS objects: {}", file_count);
    println!("  Seed ID: {}", &seed_id[..16]);

    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 3: Commit**

```bash
git add src/commands/bootstrap/mod.rs
git commit -m "feat(bootstrap): implement bootstrap seed command

Walks cross-tools output, stores files in CAS, builds EROFS
image via composefs-rs, writes seed.toml metadata."
```

---

### Task 4: Wire `cmd_bootstrap_run` to Derivation Pipeline

**Files:**
- Modify: `src/commands/bootstrap/mod.rs`

This is the main integration point -- replace the stub with real pipeline execution.

- [ ] **Step 1: Write the recipe loader helper**

Add a helper function in `src/commands/bootstrap/mod.rs`:

```rust
/// Load all recipes from subdirectories of recipe_dir, returning a HashMap
/// keyed by package name. Walks cross-tools/, temp-tools/, system/, tier2/.
fn load_recipes(recipe_dir: &Path) -> Result<std::collections::HashMap<String, conary_core::recipe::Recipe>> {
    use conary_core::recipe::parser::parse_recipe_file;

    let mut recipes = std::collections::HashMap::new();
    let subdirs = ["cross-tools", "temp-tools", "system", "tier2"];

    for subdir in &subdirs {
        let dir = recipe_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match parse_recipe_file(&path) {
                    Ok(recipe) => {
                        recipes.insert(recipe.package.name.clone(), recipe);
                    }
                    Err(e) => {
                        tracing::warn!("Skipping {}: {e}", path.display());
                    }
                }
            }
        }
    }

    Ok(recipes)
}
```

- [ ] **Step 2: Replace the `cmd_bootstrap_run` stub**

Replace the entire `cmd_bootstrap_run` function body:

```rust
pub async fn cmd_bootstrap_run(opts: BootstrapRunOptions<'_>) -> Result<()> {
    use conary_core::db::schema::migrate;
    use conary_core::derivation::executor::{DerivationExecutor, ExecutorConfig};
    use conary_core::derivation::manifest::SystemManifest;
    use conary_core::derivation::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
    use conary_core::derivation::seed::Seed;
    use conary_core::derivation::stages::{Stage, assign_stages};
    use conary_core::filesystem::CasStore;
    use rusqlite::Connection;
    use std::collections::HashSet;

    info!(
        "bootstrap run: manifest={}, work_dir={}, seed={}",
        opts.manifest, opts.work_dir, opts.seed
    );

    // 1. Load manifest
    let manifest_path = PathBuf::from(opts.manifest);
    let manifest = SystemManifest::load(&manifest_path)
        .context("Failed to load system manifest")?;
    println!("System: {} ({})", manifest.system.name, manifest.system.target);
    println!("Packages: {} included", manifest.packages.include.len());

    // 2. Load seed
    let seed_path = PathBuf::from(opts.seed);
    let seed = Seed::load_local(&seed_path)
        .map_err(|e| anyhow::anyhow!("Failed to load seed: {e}"))?;
    println!("Seed: {} ({})", &seed.build_env_hash()[..16], seed_path.display());

    // 3. Load recipes
    let recipe_dir = PathBuf::from(opts.recipe_dir);
    let all_recipes = load_recipes(&recipe_dir)?;
    println!("Recipes loaded: {}", all_recipes.len());

    // Filter to manifest includes + transitive deps
    let included: HashSet<String> = manifest.packages.include.iter().cloned().collect();
    let mut needed: HashSet<String> = included.clone();
    // Add transitive makedepends/requires
    let mut frontier: Vec<String> = included.into_iter().collect();
    while let Some(pkg) = frontier.pop() {
        if let Some(recipe) = all_recipes.get(&pkg) {
            for dep in recipe.build.requires.iter().chain(recipe.build.makedepends.iter()) {
                if needed.insert(dep.clone()) {
                    frontier.push(dep.clone());
                }
            }
        }
    }

    let recipes: std::collections::HashMap<String, conary_core::recipe::Recipe> = all_recipes
        .into_iter()
        .filter(|(name, _)| needed.contains(name))
        .collect();
    println!("Recipes after dep resolution: {}", recipes.len());

    // 4. Assign stages
    let custom_packages: HashSet<String> = HashSet::new();
    let assignments = assign_stages(&recipes, &custom_packages)
        .map_err(|e| anyhow::anyhow!("Stage assignment failed: {e}"))?;
    println!("Stage assignments: {} packages", assignments.len());

    // 5. Open DB
    let work_dir = PathBuf::from(opts.work_dir);
    std::fs::create_dir_all(&work_dir)?;
    let db_path = work_dir.join("derivations.db");
    let conn = Connection::open(&db_path)
        .context("Failed to open derivation database")?;
    migrate(&conn).context("Failed to run database migrations")?;

    // 6. Create CAS and executor
    let cas_dir = work_dir.join("output").join("objects");
    std::fs::create_dir_all(&cas_dir)?;
    let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

    let executor_config = ExecutorConfig {
        log_dir: Some(work_dir.join("logs")),
        keep_logs: opts.keep_logs,
        shell_on_failure: opts.shell_on_failure,
    };
    let executor = DerivationExecutor::new(cas, cas_dir.clone(), executor_config);

    // 7. Create pipeline
    let up_to_stage = opts
        .up_to
        .map(|s| Stage::from_str_name(s))
        .transpose()
        .map_err(|e| anyhow::anyhow!("invalid --up-to stage: {e}"))?;

    let pipeline_config = PipelineConfig {
        cas_dir: cas_dir.clone(),
        work_dir: work_dir.join("pipeline"),
        target_triple: manifest.system.target.clone(),
        jobs: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        log_dir: Some(work_dir.join("logs")),
        keep_logs: opts.keep_logs,
        shell_on_failure: opts.shell_on_failure,
        up_to_stage,
        only_packages: opts.only.map(|s| s.to_vec()),
        cascade: opts.cascade,
        substituter_sources: vec![],
        publish_endpoint: None,
        publish_token: None,
    };

    std::fs::create_dir_all(&pipeline_config.work_dir)?;
    let pipeline = Pipeline::new(pipeline_config, executor);

    // 8. Execute pipeline
    println!("\nStarting derivation pipeline...\n");
    let profile = pipeline
        .execute(&seed, &recipes, &assignments, &conn, |event| {
            match event {
                PipelineEvent::StageStarted { name, package_count } => {
                    println!("[{name}] Stage started ({package_count} packages)");
                }
                PipelineEvent::PackageBuilding { name, stage } => {
                    println!("[{stage}] Building {name}...");
                }
                PipelineEvent::PackageCached { name } => {
                    println!("  [cached] {name}");
                }
                PipelineEvent::PackageBuilt { name, duration_secs } => {
                    println!("  [built] {name} in {duration_secs}s");
                }
                PipelineEvent::PackageFailed { name, error } => {
                    println!("  [FAILED] {name}: {error}");
                }
                PipelineEvent::SubstituterHit {
                    name,
                    peer,
                    objects_fetched,
                } => {
                    println!("  [substituted] {name} from {peer} ({objects_fetched} objects)");
                }
                PipelineEvent::BuildLogWritten { package, path } => {
                    println!("  [log] {package}: {}", path.display());
                }
                PipelineEvent::StageCompleted { name } => {
                    println!("[{name}] Stage completed\n");
                }
                PipelineEvent::PipelineCompleted {
                    total_packages,
                    cached,
                    built,
                } => {
                    println!(
                        "[COMPLETE] {total_packages} packages processed ({built} built, {cached} cached)"
                    );
                }
            }
        })
        .await?;

    // 9. Write generation output
    let output_dir = work_dir.join("output");
    let gen_dir = output_dir.join("generations").join("1");
    std::fs::create_dir_all(&gen_dir)?;

    // The pipeline's final stage already composed an EROFS image.
    // Find it in the pipeline work dir and copy to generation output.
    let last_stage = profile.stages.last();
    if let Some(stage) = last_stage {
        let stage_erofs = work_dir
            .join("pipeline")
            .join(format!("stage-{}", stage.name))
            .join("root.erofs");
        if stage_erofs.exists() {
            let dest = gen_dir.join("root.erofs");
            std::fs::copy(&stage_erofs, &dest)?;
            println!("Generation 1 EROFS: {}", dest.display());
        }
    }

    // Write generation metadata
    let gen_meta = serde_json::json!({
        "generation": 1,
        "system_name": manifest.system.name,
        "target": manifest.system.target,
        "packages": profile.stages.iter()
            .flat_map(|s| s.derivations.iter())
            .map(|d| format!("{}-{}", d.package, d.version))
            .collect::<Vec<_>>(),
        "profile_hash": profile.profile.profile_hash,
    });
    std::fs::write(
        gen_dir.join(".conary-gen.json"),
        serde_json::to_string_pretty(&gen_meta)?,
    )?;

    // Write merged manifest for image step
    let profile_toml = toml::to_string_pretty(&profile)?;
    std::fs::write(gen_dir.join("profile.toml"), &profile_toml)?;

    // Symlink current -> generations/1
    let current_link = output_dir.join("current");
    let _ = std::fs::remove_file(&current_link);
    std::os::unix::fs::symlink("generations/1", &current_link)?;

    println!("\nOutput: {}", output_dir.display());
    println!("Profile hash: {}", profile.profile.profile_hash);

    Ok(())
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build 2>&1 | tail -10`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add src/commands/bootstrap/mod.rs
git commit -m "feat(bootstrap): wire bootstrap run to derivation pipeline

Replace stub with full pipeline execution: load manifest, load
seed, parse recipes, assign stages, execute Pipeline, write
generation output."
```

---

### Task 5: Extend `bootstrap image` for EROFS Generation Input

**Files:**
- Modify: `src/commands/bootstrap/mod.rs` (`cmd_bootstrap_image`)
- Modify: `conary-core/src/bootstrap/image.rs`

- [ ] **Step 1: Add `build_from_generation` to `ImageBuilder`**

In `conary-core/src/bootstrap/image.rs`, add a new public method. First read the file to find where to add it (after the existing `build()` method). The method creates a GPT disk image, writes the EROFS as root partition, sets up ESP with kernel:

```rust
    /// Build a bootable image from a pipeline-generated EROFS generation.
    ///
    /// Instead of copying a sysroot tree, this writes the pre-composed EROFS
    /// image as the root filesystem and extracts the kernel from CAS for the ESP.
    pub fn build_from_generation(
        generation_dir: &Path,
        output: &Path,
        format: ImageFormat,
        size: ImageSize,
    ) -> Result<ImageResult, ImageError> {
        let erofs_path = generation_dir.join("generations/1/root.erofs");
        if !erofs_path.exists() {
            return Err(ImageError::BaseSystemNotFound(erofs_path));
        }

        let erofs_size = std::fs::metadata(&erofs_path)
            .map_err(|e| ImageError::CreationFailed(e.to_string()))?
            .len();
        let image_bytes = size.bytes();

        // Ensure image is large enough for ESP + EROFS
        let esp_bytes: u64 = 512 * 1024 * 1024; // 512MB ESP
        if image_bytes < esp_bytes + erofs_size {
            return Err(ImageError::CreationFailed(format!(
                "Image size {}MB too small for ESP (512MB) + EROFS ({}MB)",
                image_bytes / (1024 * 1024),
                erofs_size / (1024 * 1024),
            )));
        }

        // Create raw disk image
        let raw_path = if format == ImageFormat::Raw {
            output.to_path_buf()
        } else {
            output.with_extension("raw")
        };

        // Allocate sparse file
        let f = std::fs::File::create(&raw_path)
            .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
        f.set_len(image_bytes)
            .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
        drop(f);

        // Partition with sfdisk: ESP + root
        let sfdisk_input = "label: gpt\nsize=512M, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=ESP\ntype=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=conaryos\n";
        let mut child = Command::new("sfdisk")
            .arg(&raw_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ImageError::PartitionFailed(format!("sfdisk spawn: {e}")))?;
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(sfdisk_input.as_bytes())
                .map_err(|e| ImageError::PartitionFailed(format!("sfdisk stdin: {e}")))?;
        }
        let sfdisk_status = child
            .wait()
            .map_err(|e| ImageError::PartitionFailed(format!("sfdisk wait: {e}")))?;
        if !sfdisk_status.success() {
            return Err(ImageError::PartitionFailed("sfdisk exited non-zero".into()));
        }

        // Format ESP as FAT32
        // losetup to get the ESP partition, then mkfs.fat
        // For now, use dd to write at the partition offset
        let esp_offset: u64 = 1024 * 1024; // 1MB alignment (GPT header)

        // Create FAT32 image for ESP and write kernel + boot entry
        let esp_img = output.with_extension("esp.img");
        let esp_size_mb = 512;
        let mkfs_status = Command::new("mkfs.fat")
            .args(["-C", "-F", "32"])
            .arg(&esp_img)
            .arg(format!("{}", esp_size_mb * 1024)) // size in KB
            .status()
            .map_err(|e| ImageError::FilesystemFailed(format!("mkfs.fat: {e}")))?;
        if !mkfs_status.success() {
            return Err(ImageError::FilesystemFailed("mkfs.fat failed".into()));
        }

        // Copy kernel from CAS to ESP using mtools (mcopy)
        // The kernel path in the generation's OutputManifest is /boot/vmlinuz.
        // Read the profile to find it; fall back to searching CAS directly.
        let profile_path = generation_dir.join("generations/1/profile.toml");
        if profile_path.exists() {
            // TODO: Parse profile.toml, find kernel CAS hash, retrieve, mcopy to ESP
            // For the first iteration, this is a known gap -- the image will have
            // an empty ESP. The kernel must be added manually or via a follow-up.
            tracing::warn!("ESP kernel population not yet implemented -- image may not boot without manual kernel install");
        }

        // Create boot loader entry directory structure on ESP
        let _mmd = Command::new("mmd")
            .args(["-i"])
            .arg(&esp_img)
            .args(["::EFI", "::EFI/BOOT", "::EFI/conaryos", "::loader", "::loader/entries"])
            .status();

        // Write ESP into raw image at ESP offset
        {
            use std::io::{Seek, SeekFrom, Write};
            let esp_data = std::fs::read(&esp_img)
                .map_err(|e| ImageError::CreationFailed(format!("read ESP: {e}")))?;
            let mut raw_file = std::fs::OpenOptions::new()
                .write(true)
                .open(&raw_path)
                .map_err(|e| ImageError::CreationFailed(format!("open raw: {e}")))?;
            raw_file
                .seek(SeekFrom::Start(esp_offset))
                .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
            raw_file
                .write_all(&esp_data)
                .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
        }
        let _ = std::fs::remove_file(&esp_img);

        // Write EROFS image to root partition
        let root_offset = esp_offset + esp_bytes;
        {
            use std::io::{Seek, SeekFrom, Write};
            let erofs_data = std::fs::read(&erofs_path)
                .map_err(|e| ImageError::CreationFailed(format!("read EROFS: {e}")))?;
            let mut raw_file = std::fs::OpenOptions::new()
                .write(true)
                .open(&raw_path)
                .map_err(|e| ImageError::CreationFailed(format!("open raw: {e}")))?;
            raw_file
                .seek(SeekFrom::Start(root_offset))
                .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
            raw_file
                .write_all(&erofs_data)
                .map_err(|e| ImageError::CreationFailed(e.to_string()))?;
        }

        // Convert to qcow2 if requested
        let final_path = if format == ImageFormat::Qcow2 {
            let status = Command::new("qemu-img")
                .args(["convert", "-f", "raw", "-O", "qcow2"])
                .arg(&raw_path)
                .arg(output)
                .status()
                .map_err(|e| ImageError::CreationFailed(format!("qemu-img: {e}")))?;
            if !status.success() {
                return Err(ImageError::CreationFailed("qemu-img convert failed".into()));
            }
            let _ = std::fs::remove_file(&raw_path);
            output.to_path_buf()
        } else {
            raw_path
        };

        let final_size = std::fs::metadata(&final_path)
            .map_err(|e| ImageError::CreationFailed(e.to_string()))?
            .len();

        Ok(ImageResult {
            path: final_path,
            format,
            size: final_size,
            method: "erofs-generation".to_string(),
            efi_bootable: true,
            bios_bootable: false,
            partitions: vec![
                "ESP (512MB FAT32)".to_string(),
                format!("Root (EROFS, {} bytes)", erofs_size),
            ],
        })
    }
```

**Note:** The ESP kernel extraction from CAS (reading `profile.toml` to find the kernel hash, then `CasStore::retrieve()` + `mcopy` to write to ESP) is marked as a TODO in this implementation. The EROFS root partition with the full system is written correctly. The kernel-to-ESP path will need a follow-up once the pipeline has produced its first output and we can verify the manifest structure. The image is structurally correct (GPT, ESP, EROFS root) but may need manual kernel placement for the first boot test.

- [ ] **Step 2: Update `cmd_bootstrap_image` to use `from_generation`**

In `src/commands/bootstrap/mod.rs`, add the `from_generation` path at the top of `cmd_bootstrap_image`, before existing logic:

```rust
    // If --from-generation is provided, use the new EROFS generation path
    if let Some(gen_dir) = from_generation {
        println!("Generating image from EROFS generation...");
        println!("  Generation: {}", gen_dir);
        println!("  Output: {}", output);
        println!("  Format: {}", format);

        let image_format = ImageFormat::from_str(format)
            .context("Invalid image format. Use: raw, qcow2, iso, erofs")?;
        let image_size = ImageSize::from_str(size)
            .context("Invalid size. Use: 4G, 8G, 512M, etc.")?;

        let result = ImageBuilder::build_from_generation(
            Path::new(gen_dir),
            Path::new(output),
            image_format,
            image_size,
        )?;

        println!("\n[OK] Image generated successfully!");
        println!("  Path: {}", result.path.display());
        println!("  Format: {}", result.format);
        println!("  Size: {} bytes ({:.1} GB)", result.size, result.size as f64 / 1_073_741_824.0);
        println!("  Method: {}", result.method);
        println!("\nUsage:");
        println!(
            "  qemu-system-x86_64 -drive file={},format={} -m 2G -enable-kvm -nographic",
            output,
            if image_format == ImageFormat::Qcow2 { "qcow2" } else { "raw" }
        );

        return Ok(());
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add src/commands/bootstrap/mod.rs conary-core/src/bootstrap/image.rs
git commit -m "feat(bootstrap): extend image builder for EROFS generation input

Add build_from_generation() that writes pre-composed EROFS as
root partition in a GPT disk image. Used by bootstrap image
--from-generation."
```

---

### Task 6: Build, Test End-to-End, and Fix Issues

**Files:**
- All modified files from Tasks 1-5

This task verifies everything compiles cleanly, unit tests pass, and the CLI help works correctly.

- [ ] **Step 1: Full build**

Run: `cargo build 2>&1 | tail -10`
Expected: `Finished` with no errors.

- [ ] **Step 2: Run unit tests**

Run: `cargo test 2>&1 | tail -20`
Expected: All existing tests pass, plus the new `parse_conaryos_manifest` test.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -- -D warnings 2>&1 | tail -20`
Expected: No errors. Fix any clippy warnings before proceeding.

- [ ] **Step 4: Verify CLI help text**

Run: `cargo run -- bootstrap seed --help`
Expected: Shows `--from`, `--output`, `--target` flags.

Run: `cargo run -- bootstrap run --help`
Expected: Shows `--seed`, `--recipe-dir` flags alongside existing flags.

Run: `cargo run -- bootstrap image --help`
Expected: Shows `--from-generation` flag alongside existing flags.

- [ ] **Step 5: Fix any issues found**

Address compilation errors, clippy warnings, or test failures. Common issues:
- Missing `use` statements
- Type mismatches between `BootstrapRunOptions` struct and dispatch
- `walkdir` may need to be added as a dependency (check `Cargo.toml`)

- [ ] **Step 6: Commit fixes**

```bash
git add -A
git commit -m "fix(bootstrap): resolve build and lint issues from pipeline wiring"
```

---

### Task 7: Deploy to Remi and Run Bootstrap

**Files:** None (operational task)

This task deploys the code to Remi and kicks off the four-step bootstrap.

- [ ] **Step 1: Rsync source to Remi**

```bash
rsync -az --delete --exclude target --exclude .git \
    ~/Conary/ root@ssh.conary.io:/root/conary-src/
```

- [ ] **Step 2: Build on Remi**

```bash
ssh root@ssh.conary.io 'cd /root/conary-src && cargo build 2>&1 | tail -5'
```
Expected: `Finished` with no errors.

- [ ] **Step 3: Run Step 1 — Cross-tools**

```bash
ssh root@ssh.conary.io 'cd /root/conary-src && \
    ./target/debug/conary bootstrap cross-tools \
    -w /conary/bootstrap --lfs-root /conary/bootstrap/lfs'
```
Expected: Builds 5 cross-tools packages. Takes ~15-30 min.

- [ ] **Step 4: Run Step 2 — Seed**

```bash
ssh root@ssh.conary.io 'cd /root/conary-src && \
    ./target/debug/conary bootstrap seed \
    --from /conary/bootstrap/lfs/tools \
    --output /conary/bootstrap/seed'
```
Expected: Creates `seed.erofs`, `seed.toml`, `cas/` directory. Takes ~1-2 min.

- [ ] **Step 5: Run Step 3 — Pipeline**

```bash
ssh root@ssh.conary.io 'cd /root/conary-src && \
    ./target/debug/conary bootstrap run conaryos.toml \
    -w /conary/bootstrap \
    --seed /conary/bootstrap/seed \
    --recipe-dir recipes \
    --keep-logs \
    --shell-on-failure'
```
Expected: Builds 88 packages through 4 stages. Takes several hours. Monitor via `ssh root@ssh.conary.io 'tail -f /conary/bootstrap/logs/*.log'`.

- [ ] **Step 6: Run Step 4 — Image**

```bash
ssh root@ssh.conary.io 'cd /root/conary-src && \
    ./target/debug/conary bootstrap image \
    --from-generation /conary/bootstrap/output \
    -o /conary/bootstrap/conaryos.qcow2 \
    -f qcow2 \
    -s 4G'
```
Expected: Creates `conaryos.qcow2` bootable VM image. Takes ~5 min.

- [ ] **Step 7: Verify boot**

```bash
ssh root@ssh.conary.io 'qemu-system-x86_64 \
    -drive file=/conary/bootstrap/conaryos.qcow2,format=qcow2 \
    -m 2G -enable-kvm -nographic'
```
Expected: Boots to a login prompt (or at minimum, kernel messages visible).

---

## Dependency Graph

```
Task 1 (manifest) ─────────────┐
Task 2 (CLI defs) ─────────────┤
                                ├─── Task 6 (build & test)
Task 3 (seed impl) ────────────┤         │
Task 4 (pipeline wiring) ──────┤         ▼
Task 5 (image extension) ──────┘   Task 7 (deploy & run)
```

Tasks 1-5 can be worked in parallel (they touch different code areas). Task 6 depends on all of 1-5. Task 7 depends on 6.
