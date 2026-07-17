<svelte:head>
	<title>Features - Conary</title>
	<meta name="description" content="Conary limited-preview capabilities, advanced experimental surfaces, and safe command examples." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">Features</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			Limited-preview capabilities and advanced surfaces, with their current boundaries.
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
					The advanced generation path builds EROFS artifacts from current system state.
					They can be selected for next boot or exported for image validation; composefs
					and fs-verity integration depends on compatible host support. This path is
					separate from the adoption-led package preview and should be tested in a VM.
				</p>
				<div class="feature-code">
					<code>sudo conary system generation build --summary "Post-update" --yes</code>
					<code>sudo conary system generation list</code>
					<code>sudo conary system generation switch 2 --yes</code>
					<code>sudo conary system generation rollback --yes</code>
					<code>sudo conary system generation gc --keep 3 --yes</code>
					<code>sudo conary system generation info 2</code>
				</div>
				<p class="feature-note">Generation experiments require Linux 6.2+ plus compatible EROFS, composefs, and fs-verity support. The basic package loop does not.</p>
			</div>

			<div class="feature-card">
				<h3>Adoption and Explicit Takeover</h3>
				<p>
					Start by adopting installed RPM/DEB/pacman packages into Conary tracking while
					the native package manager remains authoritative. Explicit takeover is a later
					opt-in path that prepares a Conary generation instead of being the default preview
					workflow.
				</p>
				<div class="feature-code">
					<code>sudo conary system adopt --system --dry-run</code>
					<code>sudo conary system unadopt --all --dry-run</code>
					<code>sudo conary system takeover --dry-run</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Bootstrap</h3>
				<p>
					Build a complete Conary-managed system from scratch using the current
					cross-tools, temp-tools, system, config, image, and optional tier2 stages.
					systemd-repart handles rootless image creation when available, with a
					fallback to sfdisk/mkfs. Outputs include EROFS images, CAS state, and
					SQLite metadata. The source-build pipeline has experimental x86_64,
					aarch64, and riscv64 targets; preview packages and the tester lane are
					x86_64 only. Dry-run mode validates the pipeline without building.
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
				<p class="feature-note">RecipeGraph handles dependency ordering with automatic cycle breaking. SHA-256 checksum enforcement on all source downloads. Generation artifact export is x86_64-first today; aarch64/riscv64 boot assets are reserved for follow-up work.</p>
			</div>

			<div class="feature-card">
				<h3>Generation Rollback</h3>
				<p>
					Rolling back selects a previous installed generation rather than copying
					package files back into place. This is a VM/debug generation workflow;
					it is not the rollback mechanism for every basic package operation.
				</p>
				<div class="feature-code">
					<code>sudo conary system generation rollback --yes</code>
					<code>sudo conary system generation switch 2 --yes</code>
					<code>sudo conary system generation list</code>
					<code>sudo conary system generation gc --keep 3 --yes</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>/etc Three-Way Merge</h3>
				<p>
					The generation path has three-way merge machinery for configuration files
					under /etc. Treat merge outcomes as advanced VM evidence and inspect them
					before selecting a generation.
				</p>
			</div>

			<div class="feature-card">
				<h3>Boot Recovery</h3>
				<p>
					Generation recovery can validate the selected artifact, rebuild it from DB
					state, and scan older generation artifacts. The initramfs/Dracut integration
					is an advanced recovery surface, not part of the first tester loop.
				</p>
			</div>
		</div>

		<!-- Category 2: Package Management -->
		<div class="category animate-in" style="--stagger: 3">
			<h2 class="category-title">Package Management</h2>

			<div class="feature-card">
				<h3>Multi-Format Install</h3>
				<p>
					Conary parses RPM, DEB, Arch, and CCS inputs and resolves supported dependency
					metadata with a SAT-based solver. An install can still be refused when package
					behavior or conversion policy is unsupported.
				</p>
				<div class="feature-code">
					<code>sudo conary install ./package.rpm --dry-run</code>
					<code>sudo conary install ./package.deb --dry-run</code>
					<code>sudo conary install nginx postgresql redis --dry-run</code>
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
					CCS metadata can expose runtime, development, documentation, and other
					components so a dry-run can select only the requested subset.
				</p>
				<div class="feature-code">
					<code>sudo conary install nginx:runtime --dry-run</code>
					<code>sudo conary install openssl:devel --dry-run</code>
					<code>sudo conary install bash:doc --dry-run</code>
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
					Build packages from source using TOML recipe files. The optional
					<code>--hermetic</code> mode adds Linux namespace isolation. It is not a complete reproducibility or containment guarantee.
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
					Export CCS packages as OCI artifacts compatible with OCI registries and
					container tooling. Generation-to-OCI export is reserved for the shared
					generation artifact follow-up.
				</p>
				<div class="feature-code">
					<code>conary ccs export ./package.ccs --output ./package.oci</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Generation Delta Follow-up</h3>
				<p>
					Generation delta and chunk-reuse work remain part of the broader
					artifact roadmap. Treat the current public release evidence as package-loop
					and generation-export evidence, not a public delta-size claim.
				</p>
			</div>

			<div class="feature-card">
				<h3>Declarative System Model</h3>
				<p>
					Define desired package state in TOML and inspect drift with CI-friendly exit
					codes. Live model application remains a VM-only follow-up until it has the
					same recovery evidence as package mutation.
				</p>
				<div class="feature-code">
					<code>conary model diff</code>
					<code>sudo conary model apply --dry-run</code>
					<code>sudo conary model apply --yes</code>
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
				<h3>Experimental CAS Federation</h3>
				<p>
					The source tree contains advanced peer discovery, chunk-sharing, and routing
					surfaces. Federation is outside the reliable limited-preview path and is not
					a supported onboarding workflow.
				</p>
				<div class="feature-code">
					<code>conary federation status</code>
					<code>conary federation peers</code>
					<code>conary federation scan</code>
					<code>conary federation stats --days 7</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Release Provenance</h3>
				<p>
					The preview release publishes checksums and a detached CCS signature.
					Complete SBOM/provenance sidecars for every release artifact remain
					follow-up work.
				</p>
			</div>

			<div class="feature-card">
				<h3>Trigger System</h3>
				<p>
					Built-in post-install trigger handlers include ldconfig,
					systemd-reload, fc-cache, update-mime-database, gtk-update-icon-cache,
					depmod, and others. Execution depends on the package path and current
					adapter/scriptlet policy.
				</p>
			</div>

			<div class="feature-card">
				<h3>Capability Enforcement</h3>
				<p>
					Packages can declare runtime capabilities. The explicit capability runner uses
					Landlock and seccomp-BPF where the host kernel exposes the required support;
					this does not sandbox every package operation automatically.
				</p>
				<div class="feature-code">
					<code>conary capability show nginx</code>
					<code>conary capability run nginx -- /usr/sbin/nginx -t</code>
				</div>
			</div>

			<div class="feature-card">
				<h3>Sandboxed Scriptlets</h3>
				<p>
					Supported package scriptlets can run through namespace and resource-limit
					machinery. Scriptlet-heavy or policy-blocked packages may still be refused in
					the limited preview.
				</p>
				<div class="feature-code">
					<code>sudo conary install pkg --sandbox=always --dry-run</code>
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
