<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import { previewRelease, project } from '$lib/preview-release';
</script>

<PageMeta
	title="Features and maturity — Conary"
	description="What Conary can do today on Fedora, Ubuntu, and Arch, what is still VM-only or experimental, and what is not built yet. Every capability carries its evidence label."
	path="/features/"
/>

<PageIntro
	eyebrow="Capability map"
	title="What works, what is early, and what is not there yet."
	description="Every capability below carries the label the roadmap gives it, not the one marketing would prefer. Proven means exercised on installed release packages across Fedora 44, Ubuntu 26.04 LTS, and Arch Linux."
/>

<nav class="category-index" aria-label="Feature maturity groups">
	<div class="container">
		<a href="#proven">Proven</a>
		<a href="#owned-packages">Package core</a>
		<a href="#vm-generations">Generations</a>
		<a href="#experimental">Experimental</a>
		<a href="#not-yet">Not yet</a>
		<a href="#direction">Direction</a>
	</div>
</nav>

<section class="features-page">
	<div class="container features-content">
		<div class="category" id="proven">
			<div class="category-heading">
				<span class="category-status preview">proven · three hosts</span>
				<h2 class="category-title">The cross-distro package loop</h2>
				<p>
					Exercised on the installed {previewRelease.tag} packages on all three supported
					hosts. Still pre-alpha: proven means it ran, not that it is safe on a machine
					you rely on.
				</p>
			</div>

			<div class="feature-list">
				<article class="feature-card feature-lead">
					<span class="feature-status preview">source-format exact</span>
					<h3>Cross-distro package install</h3>
					<p>
						Fedora 44, Ubuntu 26.04 LTS, and Arch consume RPM, DEB, Arch, and CCS inputs
						through the same transaction path. Each package keeps its source lifecycle,
						dependency, version, payload, and configuration semantics. Dry-run shows the
						source ABI and the typed target capabilities it needs; preflight rejects an
						unsatisfied capability before anything is mutated, and there is no bypass flag.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Cross-distro package install commands">
						<code>sudo conary install ./package.rpm --dry-run</code>
						<code>sudo conary install ./package.deb --yes</code>
						<code>sudo conary install ./package.pkg.tar.zst --yes</code>
						<code>sudo conary install htop --from ubuntu-26.04 --dry-run</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status preview">reversible</span>
					<h3>Adoption and unadoption</h3>
					<p>
						Adoption records packages that stay owned by dnf, apt, or pacman so Conary can
						see them without taking them over. Unadoption removes the tracking and deletes
						nothing. It is the migration bridge for a machine you already have, not the
						product's main path.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Adoption and unadoption commands">
						<code>sudo conary system adopt --system --dry-run</code>
						<code>sudo conary system adopt --system</code>
						<code>sudo conary system adopt --status</code>
						<code>sudo conary system unadopt --all --dry-run</code>
						<code>sudo conary system unadopt --all --yes</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status preview">signed release</span>
					<h3>Verified installation and self-update</h3>
					<p>
						Each release ships checksums, a detached signature for the CCS artifact, and an
						Ed25519-signed bootstrap manifest that binds every supported host to one exact
						package. The bootstrap script verifies the manifest before it reads a single
						selection field, and the installed client can check for and apply its own
						updates.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Self-update commands">
						<code>sudo conary self-update --check</code>
						<code>sudo conary self-update</code>
					</div>
					<a href="/install/" class="feature-action">Open the install guide <span aria-hidden="true">→</span></a>
				</article>
			</div>
		</div>

		<div class="category" id="owned-packages">
			<div class="category-heading">
				<span class="category-status limited">available · limited</span>
				<h2 class="category-title">Package machinery Conary owns</h2>
				<p>Working package internals whose policy and adapter boundaries still matter.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status limited">core machinery</span>
					<h3>Content-addressed storage and SAT resolution</h3>
					<p>
						Conary-owned files are stored by content hash so identical content is kept once.
						A SAT solver (resolvo) handles conflicts, virtual provides, and typed
						dependencies. Garbage collection of the store follows database reference counts.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Dependency and storage inspection commands">
						<code>conary query deptree nginx</code>
						<code>conary query depends nginx</code>
						<code>conary query rdepends openssl</code>
						<code>conary query whatprovides 'soname(libssl.so.3)'</code>
						<code>sudo conary system verify</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">native format</span>
					<h3>Build, sign, verify, and inspect CCS</h3>
					<p>
						CCS is Conary's native package format: a CBOR manifest, SHA-256 content
						verification, Ed25519 signatures, and FastCDC content-defined chunking for
						large files. Signing requires an explicit private key.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="CCS package commands">
						<code>conary ccs build .</code>
						<code>conary ccs sign --key private.pem package.ccs</code>
						<code>conary ccs verify package.ccs</code>
						<code>conary ccs inspect package.ccs</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">changeset-backed</span>
					<h3>Changesets and configuration handling</h3>
					<p>
						Every install, update, and remove is a durable changeset with an explicit
						pending, applied, failed, or rolled-back outcome. A changeset records database
						and file state; it does not imply a bootable generation for every install, and
						it does not promise universal filesystem rollback. Configuration files follow
						their source format's exact rules.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">shared service</span>
					<h3>Remi, the package service</h3>
					<p>
						<a href={project.packagesUrl}>remi.conary.io</a> authenticates the Fedora,
						Ubuntu, and Arch repositories, converts their packages to CCS on demand, and
						serves them with the source-format lifecycle contract intact. A package can
						fail conversion even when upstream serves it fine; that is feedback, not a
						verdict on the package. Its first complete signed public universe is still an
						open launch gate.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Repository commands">
						<code>sudo conary repo list</code>
						<code>sudo conary repo sync remi</code>
						<code>sudo conary search htop</code>
					</div>
				</article>
			</div>
		</div>

		<div class="category" id="vm-generations">
			<div class="category-heading">
				<span class="category-status vm">VM evidence</span>
				<h2 class="category-title">System generations</h2>
				<p>Whole-system state selection and recovery. Separate from the package loop, and proven only in virtual machines.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status vm">Linux 6.2+ · x86_64</span>
					<h3>Build, select, and export EROFS generations</h3>
					<p>
						A generation is an immutable EROFS image of the system, mounted through
						composefs with fs-verity where the kernel supports it. Rollback selects an
						earlier generation for the next boot. Generations can be exported as raw,
						qcow2, or x86_64 UEFI ISO images for validation.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="System generation commands">
						<code>sudo conary system generation build --summary "Post-update" --yes</code>
						<code>sudo conary system generation list</code>
						<code>sudo conary system generation switch 2 --yes</code>
						<code>sudo conary system generation rollback --yes</code>
						<code>sudo conary system generation gc --keep 3 --yes</code>
						<code>sudo conary system generation export --path /conary/generations/1 --format qcow2 --output gen1.qcow2</code>
					</div>
					<p class="feature-note">Use a VM. The package loop above does not need composefs, fs-verity, or any boot-stack change.</p>
				</article>

				<article class="feature-card">
					<span class="feature-status vm">explicit apply</span>
					<h3>Declarative system model</h3>
					<p>
						Desired package state can be described and diffed against the host. Live apply
						stays a VM-only path until it has the same recovery evidence as package mutation.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Declarative model commands">
						<code>conary model diff</code>
						<code>sudo conary model apply --dry-run</code>
						<code>sudo conary model apply --yes</code>
						<code>conary model check</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status vm">recovery surface</span>
					<h3>Configuration merge and recovery</h3>
					<p>
						The tree includes three-way merge for <code>/etc</code>, generation artifact
						validation, and database-backed rebuild paths. Treat all of it as advanced VM
						evidence, not first-run onboarding.
					</p>
				</article>
			</div>
		</div>

		<div class="category category-quiet" id="experimental">
			<div class="category-heading">
				<span class="category-status experimental">experimental</span>
				<h2 class="category-title">Infrastructure and build surfaces</h2>
				<p>Real interfaces in the source tree without a reliable onboarding contract for strangers.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status experimental">source-build research</span>
					<h3>Bootstrap and architecture targets</h3>
					<p>
						A staged bootstrap pipeline runs from cross-tools through image creation, with
						experimental x86_64, aarch64, and riscv64 source-build targets. Published packages
						and all generation evidence remain x86_64.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status experimental">not onboarding</span>
					<h3>conaryd and CAS federation</h3>
					<p>
						conaryd is a local daemon with Unix-socket REST and SSE scaffolding.
						Federation is outside the reliable limited-preview path: peer discovery,
						routing, and chunk-sharing surfaces exist, but coordinator and serving paths
						are not yet wired into one supported operating model. Neither is a fleet service.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Experimental federation inspection commands">
						<code>conary federation status</code>
						<code>conary federation peers</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status experimental">incomplete integration</span>
					<h3>Recipes and derivations</h3>
					<p>
						Conary can build a package from a recipe. The <code>--isolated</code> cook path
						adds Linux namespace isolation. It is
						not a complete reproducibility or containment guarantee. Derivation, lock, and
						update flows still have incomplete persisted inputs and integration edges.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Recipe build commands">
						<code>conary cook recipe.toml</code>
						<code>conary cook --isolated recipe.toml</code>
						<code>conary cook --fetch-only recipe.toml</code>
					</div>
				</article>
			</div>
		</div>

		<div class="category category-quiet" id="not-yet">
			<div class="category-heading">
				<span class="category-status roadmap">not built</span>
				<h2 class="category-title">What is not there yet</h2>
				<p>Stated so nobody plans around it.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status roadmap">not implemented</span>
					<h3>Native transaction-history import</h3>
					<p>
						Conary does not import dnf, apt, or pacman transaction history. Adoption records
						current package ownership, not the past.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">roadmap</span>
					<h3>Generation and payload deltas</h3>
					<p>
						Chunk reuse and generation-delta work remain roadmap items. No release makes a
						delta-size or bandwidth-reduction claim.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">proof expansion</span>
					<h3>More distributions and architectures</h3>
					<p>
						The package contract is not tied to three distro names, but a host joins the
						supported list only after installed-package proof on it. Non-x86_64 generation
						boot assets are still reserved.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">not published</span>
					<h3>SBOM and provenance sidecars</h3>
					<p>
						Releases carry checksums, a CCS signature, and a signed bootstrap manifest. They
						do not carry SBOM or provenance sidecars.
					</p>
				</article>
			</div>
		</div>

		<div class="category category-quiet" id="direction">
			<div class="category-heading">
				<span class="category-status roadmap">direction</span>
				<h2 class="category-title">Where it is going</h2>
				<p>Product direction, not an ordered release promise. Priorities after the preview will come from tester evidence.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status roadmap">direction</span>
					<h3>Third-party package building and publishing</h3>
					<p>
						Turn the existing recipe, isolated-build, CCS, signing, and static-publication
						machinery into a workflow other projects can use, not only an internal
						bootstrap pipeline.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">direction</span>
					<h3>Replatforming as a plan, not a ritual</h3>
					<p>
						Connect the declarative model to builders and source selection so a distribution
						move becomes an inspectable plan rather than a fresh install.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">direction</span>
					<h3>Agent-facing operations without hidden authority</h3>
					<p>
						A typed, versioned agent contract already exists in the tree and MCP adapts it.
						The longer path connects operator services and automation without making
						network authority the default.
					</p>
				</article>
			</div>
		</div>

		<section class="features-cta">
			<div>
				<p class="eyebrow">Start with the proven part</p>
				<h2>Install one package your distro did not ship.</h2>
				<p>The install guide pins the release, verifies it, and walks the bounded loop in order.</p>
			</div>
			<a href="/install/" class="btn btn-primary">Open the install guide</a>
		</section>
	</div>
</section>

<style>
	.category-index {
		position: sticky;
		top: var(--header-height);
		z-index: 20;
		border-bottom: 1px solid var(--color-border);
		background: rgb(10 18 36 / 96%);
	}

	.category-index .container {
		display: grid;
		grid-template-columns: repeat(6, minmax(0, 1fr));
	}

	.category-index a {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 48px;
		padding: 0.65rem 0.4rem;
		border-left: 1px solid var(--color-border);
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: var(--text-label);
		text-align: center;
		text-decoration: none;
	}

	.category-index a:last-child {
		border-right: 1px solid var(--color-border);
	}

	.category-index a:hover {
		color: var(--color-field);
		background: var(--color-cyan);
	}

	.features-page {
		padding: var(--section-space) 0 1rem;
	}

	.features-content {
		max-width: 1080px;
	}

	.category {
		display: grid;
		grid-template-columns: minmax(190px, 0.75fr) minmax(0, 2fr);
		gap: clamp(2rem, 6vw, 5rem);
		scroll-margin-top: 8rem;
		padding-bottom: var(--section-space);
		margin-bottom: var(--section-space);
		border-bottom: 1px solid var(--color-border);
	}

	.category-heading {
		position: sticky;
		top: calc(var(--header-height) + 5.5rem);
		align-self: start;
		padding-top: 0.35rem;
	}

	.category-title {
		margin: 0.8rem 0 0.7rem;
		font-size: var(--step-section);
	}

	.category-heading > p {
		margin: 0;
		color: var(--color-muted);
		font-size: 0.9rem;
	}

	.category-status,
	.feature-status {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		font-family: var(--font-mono);
		font-size: var(--text-label);
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.category-status::before,
	.feature-status::before {
		content: '';
		width: 0.5rem;
		height: 0.5rem;
		flex: 0 0 auto;
	}

	.preview {
		color: var(--color-cyan);
	}

	.preview::before {
		background: var(--color-cyan);
	}

	.limited {
		color: var(--color-ivory);
	}

	.limited::before {
		border: 1px solid var(--color-cyan);
	}

	.vm {
		color: var(--color-orange);
	}

	.vm::before {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.8);
	}

	.experimental,
	.roadmap {
		color: var(--color-muted);
	}

	.experimental::before {
		border: 1px solid var(--color-muted);
	}

	.roadmap::before {
		width: 0.6rem;
		height: 1px;
		background: var(--color-muted);
	}

	.feature-list {
		min-width: 0;
	}

	.feature-card {
		padding: 0 0 2rem;
		margin-bottom: 2rem;
		border-bottom: 1px solid var(--color-border);
	}

	.feature-card:last-child {
		margin-bottom: 0;
	}

	.feature-lead {
		padding: clamp(1.25rem, 3vw, 2rem);
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.feature-card h3 {
		margin: 0.55rem 0 0.65rem;
		font-family: var(--font-body);
		font-size: 1.15rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.feature-card p {
		max-width: 70ch;
		margin: 0 0 1rem;
		color: var(--color-mist);
		font-size: 0.96rem;
		line-height: 1.72;
	}

	.feature-card p:last-child {
		margin-bottom: 0;
	}

	.feature-code {
		display: flex;
		flex-direction: column;
		gap: 1px;
		margin: 1.1rem 0;
		border: 1px solid var(--color-border);
		background: var(--color-border);
	}

	.feature-code code {
		display: block;
		min-width: max-content;
		padding: 0.55rem 0.8rem;
		border: 0;
		border-radius: 0;
		color: var(--color-ivory);
		background: var(--color-code-bg);
		font-family: var(--font-mono);
		font-size: 0.8rem;
		line-height: 1.55;
		white-space: nowrap;
	}

	.feature-code:focus-visible {
		outline-offset: 3px;
	}

	.feature-note {
		padding: 0.85rem 1rem;
		border-left: 3px solid var(--color-orange);
		color: var(--color-muted) !important;
		background: rgb(252 107 22 / 6%);
		font-size: var(--text-caption) !important;
	}

	.feature-action {
		display: inline-flex;
		align-items: center;
		gap: 0.55rem;
		margin-top: 0.75rem;
		font-family: var(--font-mono);
		font-size: 0.8rem;
		text-decoration: none;
	}

	.category-quiet .feature-card {
		opacity: 0.88;
	}

	.features-cta {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 3rem;
		padding: clamp(2rem, 5vw, 4rem);
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.features-cta h2 {
		max-width: 20ch;
		margin-bottom: 0.75rem;
		font-size: var(--step-lead);
	}

	.features-cta p:last-child {
		max-width: 58ch;
		margin: 0;
		color: var(--color-mist);
	}

	.features-cta .btn {
		flex: 0 0 auto;
	}

	@media (max-width: 760px) {
		.category-index {
			position: static;
		}

		.category-index .container {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			width: 100%;
		}

		.category-index a {
			border-bottom: 1px solid var(--color-border);
		}

		.category {
			grid-template-columns: 1fr;
			gap: 1.5rem;
			scroll-margin-top: 8rem;
		}

		.category-heading {
			position: static;
			padding-bottom: 1rem;
			border-bottom: 2px solid var(--color-cyan);
		}

		.features-cta {
			align-items: stretch;
			flex-direction: column;
		}
	}
</style>
