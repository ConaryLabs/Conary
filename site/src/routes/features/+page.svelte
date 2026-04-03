<svelte:head>
	<title>Features - Conary</title>
	<meta name="description" content="Complete feature reference for Conary -- system generations, package management, build tools, and infrastructure." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">Features</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			Everything Conary can do, with examples.
		</p>
	</div>
</section>

<section class="features-page">
	<div class="container features-content">

		<!-- Category 1: System Management -->
		<div class="category animate-in" style="--stagger: 2">
			<h2 class="category-title">System Management</h2>

			<div class="feature-card">
				<h3>System Generations</h3>
				<p>
					Atomic, immutable filesystem snapshots using EROFS images and Linux composefs.
					Build a generation from current system state and switch between generations live,
					without rebooting. Every generation is a complete, verified filesystem snapshot.
				</p>
				<div class="feature-code">
					<code>conary system generation build --summary "Post-update"</code>
					<code>conary system generation list</code>
					<code>conary system generation switch 2</code>
					<code>conary system generation rollback</code>
					<code>conary system generation gc --keep 3</code>
					<code>conary system generation info 2</code>
				</div>
				<p class="feature-note">Requires Linux 6.2+ with composefs support.</p>
			</div>

			<div class="feature-card">
				<h3>System Takeover</h3>
				<p>
					Adopt an entire existing Linux system into Conary management. Analyzes all
					installed RPM/DEB/pacman packages, plans the adoption strategy, and atomically
					takes over the system with an initial generation.
				</p>
				<div class="feature-code">
					<code>conary system takeover --dry-run</code>
					<code>conary system takeover</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Bootstrap</h3>
				<p>
					Build a complete Conary-managed system from scratch using the current
					cross-tools, temp-tools, system, config, image, and optional tier2 stages.
					systemd-repart handles rootless image creation when available, with a
					fallback to sfdisk/mkfs. Outputs include EROFS images, CAS state, and
					SQLite metadata. Supports x86_64, aarch64, and riscv64 targets, and
					dry-run mode validates the pipeline without building.
				</p>
				<div class="feature-code">
					<code>conary bootstrap init --target x86_64</code>
					<code>conary bootstrap check</code>
					<code>conary bootstrap cross-tools</code>
					<code>conary bootstrap temp-tools</code>
					<code>conary bootstrap system</code>
					<code>conary bootstrap config</code>
					<code>conary bootstrap tier2</code>
					<code>conary bootstrap image --format qcow2</code>
					<code>conary bootstrap dry-run</code>
					<code>conary bootstrap status</code>
				</div>
				<p class="feature-note">RecipeGraph handles dependency ordering with automatic cycle breaking. SHA-256 checksum enforcement on all source downloads.</p>
			</div>

			<div class="feature-card">
				<h3>Instant Rollback</h3>
				<p>
					Every generation is an immutable EROFS image. Rolling back remounts a
					previous generation -- no file copying, no rebuilding. The kernel verifies
					integrity via fs-verity on every file read.
				</p>
				<div class="feature-code">
					<code>conary system generation rollback</code>
					<code>conary system generation switch 2</code>
					<code>conary system generation list</code>
					<code>conary system generation gc --keep 3</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>/etc Three-Way Merge</h3>
				<p>
					Configuration files in /etc are handled with a three-way merge (bootc model).
					User modifications survive generation switches while upstream changes are
					integrated cleanly.
				</p>
			</div>

			<div class="feature-card">
				<h3>Boot Recovery</h3>
				<p>
					4-step fallback if the active generation fails to boot: try previous generation,
					scan for any valid generation, emergency shell, Dracut module for early-boot repair.
				</p>
			</div>
		</div>

		<!-- Category 2: Package Management -->
		<div class="category animate-in" style="--stagger: 3">
			<h2 class="category-title">Package Management</h2>

			<div class="feature-card">
				<h3>Multi-Format Install</h3>
				<p>
					Install packages from any major Linux distribution format. Dependencies are
					resolved automatically using a SAT-based solver.
				</p>
				<div class="feature-code">
					<code>conary install ./package.rpm</code>
					<code>conary install ./package.deb</code>
					<code>conary install nginx postgresql redis</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>SAT-Based Resolver</h3>
				<p>
					resolvo-based dependency resolution with typed dependencies -- soname, python,
					perl, pkgconfig, cmake, binary, and more.
				</p>
				<div class="feature-code">
					<code>conary query deptree nginx</code>
					<code>conary query depends nginx</code>
					<code>conary query rdepends openssl</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Component Model</h3>
				<p>
					Packages are automatically split into components. Install only what you need.
				</p>
				<div class="feature-code">
					<code>conary install nginx:runtime</code>
					<code>conary install openssl:devel</code>
					<code>conary install bash:doc</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Derived Packages</h3>
				<p>
					Create custom variants of existing packages with patches and file overrides.
					Derived packages track their parent and can be rebuilt when the parent updates.
				</p>
				<div class="feature-code">
					<code>conary derive create my-nginx --from nginx</code>
					<code>conary derive patch my-nginx fix.patch</code>
					<code>conary derive override my-nginx /etc/nginx/nginx.conf --source ./my-nginx.conf</code>
					<code>conary derive build my-nginx</code>
					<code>conary derive stale</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Configuration Management</h3>
				<p>
					Track, diff, backup, and restore system configuration files. Honors noreplace
					flags from RPM/DEB to preserve user modifications during upgrades.
				</p>
				<div class="feature-code">
					<code>conary config list</code>
					<code>conary config diff /etc/nginx/nginx.conf</code>
					<code>conary config backup /path</code>
					<code>conary config restore /path</code>
					<code>conary config check</code>
				</div>
			</div>
		</div>

		<!-- Category 3: Build and Distribution -->
		<div class="category animate-in" style="--stagger: 4">
			<h2 class="category-title">Build and Distribution</h2>

			<div class="feature-card">
				<h3>CCS Native Format</h3>
				<p>
					Conary's native package format with CBOR manifests, Merkle tree verification,
					Ed25519 signatures, and content-defined chunking for cross-package deduplication.
				</p>
				<div class="feature-code">
					<code>conary ccs build .</code>
					<code>conary ccs sign package.ccs</code>
					<code>conary ccs verify package.ccs</code>
					<code>conary ccs inspect package.ccs</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Recipe System</h3>
				<p>
					Build packages from source using TOML recipe files. Hermetic builds use Linux
					namespaces for maximum isolation.
				</p>
				<div class="feature-code">
					<code>conary cook recipe.toml</code>
					<code>conary cook --hermetic recipe.toml</code>
					<code>conary cook --fetch-only recipe.toml</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Dev Shells</h3>
				<p>
					Temporary environments without permanent installation, similar to nix shell.
				</p>
				<div class="feature-code">
					<code>conary ccs shell python nodejs</code>
					<code>conary ccs run gcc -- make</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>OCI Export</h3>
				<p>
					Export any generation or package set as an OCI container image compatible
					with podman and docker. Ship your exact verified system state as a container.
				</p>
				<div class="feature-code">
					<code>conary export --output ./my-image</code>
					<code>conary ccs export ./package.ccs --output ./package.oci</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>EROFS Binary Deltas</h3>
				<p>
					Upgrade generations by downloading EROFS binary deltas instead of full images.
					Combined with CAS-level deduplication, updates transfer only the blocks
					that actually changed.
				</p>
			</div>

			<div class="feature-card">
				<h3>Declarative System Model</h3>
				<p>
					Define your system in TOML and apply it atomically. Drift detection with
					CI/CD-friendly exit codes.
				</p>
				<div class="feature-code">
					<code>conary model diff</code>
					<code>conary model apply</code>
					<code>conary model check</code>
					<code>conary model snapshot</code>
				</div>
			</div>
		</div>

		<!-- Category 4: Infrastructure -->
		<div class="category animate-in" style="--stagger: 5">
			<h2 class="category-title">Infrastructure</h2>

			<div class="feature-card">
				<h3>Content-Addressable Storage</h3>
				<p>
					Files stored by SHA-256 hash with XXH128 for fast dedup checks. Automatic
					deduplication across packages. FastCDC chunking for efficient distribution.
					CAS garbage collection uses DB reference counts -- only unreferenced chunks
					are reclaimed.
				</p>
				<div class="feature-code">
					<code>conary system verify</code>
					<code>conary system gc</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>CAS Federation</h3>
				<p>
					Distributed chunk sharing across Conary nodes with mDNS LAN discovery and
					hierarchical routing -- leaf to cell hub to region hub.
				</p>
				<div class="feature-code">
					<code>conary federation status</code>
					<code>conary federation peers</code>
					<code>conary federation scan</code>
					<code>conary federation stats --days 7</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Package Provenance (DNA)</h3>
				<p>
					Full provenance chain from source to deployment with SLSA attestation support.
					Sigstore integration for signing and verification.
				</p>
				<div class="feature-code">
					<code>conary provenance show nginx</code>
					<code>conary provenance export nginx --format spdx</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Trigger System</h3>
				<p>
					10+ built-in post-install triggers with DAG-ordered execution: ldconfig,
					systemd-reload, fc-cache, update-mime-database, gtk-update-icon-cache,
					depmod, and more. Triggers fire automatically during install and remove.
				</p>
			</div>

			<div class="feature-card">
				<h3>Capability Enforcement</h3>
				<p>
					Packages declare runtime capabilities. Enforcement uses Landlock for filesystem
					restrictions and seccomp-BPF for syscall filtering.
				</p>
				<div class="feature-code">
					<code>conary capability show nginx</code>
					<code>conary capability run nginx -- /usr/sbin/nginx -t</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Sandboxed Scriptlets</h3>
				<p>
					Package install scripts run in namespace isolation -- mount, PID, IPC, UTS --
					with resource limits by default.
				</p>
				<div class="feature-code">
					<code>conary install pkg --sandbox=always</code>
				</div>
			</div>
		</div>

	</div>
</section>

<style>
	.page-hero {
		padding: 4rem 0 2rem;
		text-align: center;
	}

	.page-title {
		font-family: var(--font-display);
		font-size: 2.75rem;
		font-weight: 800;
		letter-spacing: -0.04em;
		margin-bottom: 0.5rem;
	}

	.page-desc {
		font-size: 1.0625rem;
		color: var(--color-text-secondary);
		font-weight: 300;
	}

	.features-page {
		padding: 2rem 0 5rem;
	}

	.features-content {
		max-width: 740px;
		margin: 0 auto;
	}

	.category {
		margin-bottom: 3.5rem;
	}

	.category-title {
		font-family: var(--font-display);
		font-size: 1.375rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 1.25rem;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--color-border);
	}

	.feature-card {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		padding: 1.75rem;
		margin-bottom: 1.25rem;
	}

	.feature-card:last-child {
		margin-bottom: 0;
	}

	.feature-card h3 {
		font-family: var(--font-display);
		font-size: 1.0625rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 0.625rem;
	}

	.feature-card p {
		font-size: 0.9375rem;
		color: var(--color-text-secondary);
		line-height: 1.7;
		margin: 0 0 0.75rem;
		font-weight: 300;
	}

	.feature-card p:last-child {
		margin-bottom: 0;
	}

	.feature-code {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		margin-bottom: 0.75rem;
	}

	.feature-code:last-child {
		margin-bottom: 0;
	}

	.feature-code code {
		display: block;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		background: var(--color-code-bg);
		border-radius: var(--radius-sm);
		padding: 0.5rem 0.75rem;
		color: var(--color-text);
		line-height: 1.5;
		overflow-x: auto;
	}

	.feature-note {
		font-size: 0.8125rem;
		color: var(--color-text-muted);
		font-style: italic;
		margin-bottom: 0;
	}

	@media (max-width: 768px) {
		.page-title {
			font-size: 2rem;
		}

		.feature-card {
			padding: 1.25rem;
		}

		.feature-code code {
			font-size: 0.75rem;
			padding: 0.375rem 0.625rem;
		}
	}
</style>
