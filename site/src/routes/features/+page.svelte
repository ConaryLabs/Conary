<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
</script>

<PageMeta
	title="Features and preview boundaries — Conary"
	description="Explore Conary's current package-management capabilities, experimental surfaces, and explicit preview boundaries."
	path="/features/"
/>

<PageIntro
	eyebrow="Capability map"
	title="Features, ordered by the evidence behind them."
	description="The bounded preview, Conary-owned package state, VM-only generations, experimental infrastructure, and future work are different commitments. This page keeps them visibly separate."
/>

<nav class="category-index" aria-label="Feature maturity groups">
	<div class="container">
		<a href="#preview-supported">Preview</a>
		<a href="#owned-packages">Owned packages</a>
		<a href="#vm-generations">VM generations</a>
		<a href="#experimental">Experimental</a>
		<a href="#roadmap">Not promised</a>
	</div>
</nav>

<section class="features-page">
	<div class="container features-content">
		<div class="category category-preview" id="preview-supported">
			<div class="category-heading">
				<span class="category-status preview">preview-supported</span>
				<h2 class="category-title">The bounded tester loop</h2>
				<p>Published, host-matched packages with a documented reversible workflow.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card feature-lead">
					<span class="feature-status preview">supported path</span>
					<h3>Host-matched package install</h3>
					<p>
						Fedora 44, Ubuntu 26.04 LTS, and Arch use the same Conary concepts but
						different packages and repositories for the exact host target. The bounded
						package and adoption mutations pair dry-runs with explicit approval; native
						installation and repository sync remain separately approved live steps.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Host-matched package install commands">
						<code>sudo conary install htop --dry-run --allow-capabilities</code>
						<code>sudo conary install htop --yes --allow-capabilities</code>
						<code>sudo conary update --dry-run</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status preview">reversible tracking</span>
					<h3>Adoption and unadoption</h3>
					<p>
						Adoption records packages that remain owned by dnf, apt, or pacman. Unadoption
						removes Conary tracking without deleting the native package files.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Adoption and unadoption commands">
						<code>sudo conary system adopt --system --dry-run</code>
						<code>sudo conary system adopt --system</code>
						<code>sudo conary list</code>
						<code>sudo conary search htop</code>
						<code>sudo conary system unadopt --all --dry-run</code>
						<code>sudo conary system unadopt --all --yes</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status preview">evidence boundary</span>
					<h3>Pinned release and feedback contract</h3>
					<p>
						The tester lane pins a release, verifies its checksum, records exact commands
						and exit statuses, and asks for concise public evidence without broad host dumps.
					</p>
					<a href="/install/" class="feature-action">Run the supported preview <span aria-hidden="true">→</span></a>
				</article>
			</div>
		</div>

		<div class="category" id="owned-packages">
			<div class="category-heading">
				<span class="category-status limited">available · limited</span>
				<h2 class="category-title">Conary-owned package capabilities</h2>
				<p>Useful package machinery whose policy and adapter boundaries still matter.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status limited">same-target only</span>
					<h3>RPM, DEB, Arch, and CCS inputs</h3>
					<p>
						Conary parses supported package formats and Remi can convert policy-approved
						inputs into CCS for the matching target. Public cross-target portability is not
						a current capability; unsupported package behavior fails closed.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Local package dry-run commands">
						<code>sudo conary install ./package.rpm --dry-run</code>
						<code>sudo conary install ./package.deb --dry-run</code>
						<code>sudo conary install ./package.pkg.tar.zst --dry-run</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">core machinery</span>
					<h3>CAS storage and SAT resolution</h3>
					<p>
						Conary-owned files are stored by content hash, while resolvo handles conflicts,
						virtual provides, and typed dependencies. CAS garbage collection remains tied to
						database reference counts.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Dependency and storage inspection commands">
						<code>conary query deptree nginx</code>
						<code>conary query depends nginx</code>
						<code>conary query rdepends openssl</code>
						<code>conary system verify</code>
					</div>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">native format</span>
					<h3>Build, sign, verify, and inspect CCS</h3>
					<p>
						CCS uses CBOR manifests, Merkle verification, Ed25519 signatures, and
						content-defined chunking. Signing requires an explicit private key path.
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
					<h3>Configuration handling</h3>
					<p>
						Package changesets track database and file state without implying a bootable
						generation for every install. Configuration paths can be inspected, backed up,
						and restored where current package metadata supports the operation.
					</p>
				</article>
			</div>
		</div>

		<div class="category" id="vm-generations">
			<div class="category-heading">
				<span class="category-status vm">VM-only</span>
				<h2 class="category-title">System generation work</h2>
				<p>Advanced state selection and recovery paths that are separate from the first package loop.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status vm">Linux 6.2+ host support</span>
					<h3>Build and select EROFS generations</h3>
					<p>
						The generation path builds EROFS artifacts and can use compatible composefs and
						fs-verity support. Rollback selects an earlier generation; it is not the undo
						mechanism for every package transaction.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="System generation commands">
						<code>sudo conary system generation build --summary "Post-update" --yes</code>
						<code>sudo conary system generation list</code>
						<code>sudo conary system generation switch 2 --yes</code>
						<code>sudo conary system generation rollback --yes</code>
						<code>sudo conary system generation gc --keep 3 --yes</code>
					</div>
					<p class="feature-note">Use a VM. The basic package loop does not require composefs, fs-verity, or boot-stack changes.</p>
				</article>

				<article class="feature-card">
					<span class="feature-status vm">explicit apply</span>
					<h3>Declarative system model</h3>
					<p>
						Desired package state can be described and compared with the host. Live apply
						remains a VM-only path until it has the same recovery evidence as package mutation.
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
					<h3>Configuration merge and boot recovery</h3>
					<p>
						The source tree includes three-way merge machinery for <code>/etc</code>, artifact
						validation, database-backed rebuild paths, and older-generation scanning. Treat
						all of it as advanced VM evidence, not first-run onboarding.
					</p>
				</article>
			</div>
		</div>

		<div class="category category-quiet" id="experimental">
			<div class="category-heading">
				<span class="category-status experimental">experimental</span>
				<h2 class="category-title">Infrastructure and build surfaces</h2>
				<p>Real interfaces in the source tree without a reliable stranger-operated onboarding contract.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status experimental">source-build research</span>
					<h3>Bootstrap and architecture targets</h3>
					<p>
						The staged bootstrap pipeline covers cross-tools through image creation, with
						experimental x86_64, aarch64, and riscv64 source-build targets. Published tester
						packages and current generation evidence remain x86_64.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status experimental">not onboarding</span>
					<h3>CAS federation</h3>
					<p>
						Federation is outside the reliable limited-preview path. Peer discovery, routing,
						and chunk-sharing surfaces exist, but coordinator and serving paths are not yet
						wired into one supported operating model.
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
						The public <code>--isolated</code> cook path adds Linux namespace isolation. It is
						not a complete reproducibility or containment guarantee. Advanced derivation,
						lock, and update flows still have incomplete persisted inputs and integration edges.
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

		<div class="category category-quiet" id="roadmap">
			<div class="category-heading">
				<span class="category-status roadmap">not preview commitments</span>
				<h2 class="category-title">Roadmap and refusal boundaries</h2>
				<p>Directions under discussion or explicit absences—not capabilities to plan production work around.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card">
					<span class="feature-status roadmap">roadmap</span>
					<h3>Generation delta work</h3>
					<p>
						Chunk reuse and generation-delta work remain broader artifact-roadmap items.
						The current release makes no public delta-size or bandwidth-reduction claim.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">not supported</span>
					<h3>Foreign-target package portability</h3>
					<p>
						Identity mapping can find equivalent packages, but the production target matrix
						has no public cross-distribution allowances. Different source and host targets
						fail closed without later explicit evidence.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status roadmap">not promised</span>
					<h3>Release sidecars and broader authority</h3>
					<p>
						No SBOM or provenance sidecars are published or planned for this limited preview.
						Broader scriptlet authority, non-x86 boot artifacts, and wider recovery proof must
						remain separate evidence-bearing work.
					</p>
				</article>
			</div>
		</div>

		<section class="features-cta">
			<div>
				<p class="eyebrow">Start with the strongest evidence</p>
				<h2>Run the bounded preview before exploring advanced surfaces.</h2>
				<p>The install runbook pins the package, checksum, command order, and feedback contract.</p>
			</div>
			<a href="/install/" class="btn btn-primary">Open the preview runbook</a>
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
		grid-template-columns: repeat(5, minmax(0, 1fr));
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
		font-size: 0.66rem;
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
		font-size: clamp(1.45rem, 2.7vw, 2.15rem);
		letter-spacing: -0.04em;
	}

	.category-heading > p {
		margin: 0;
		color: var(--color-muted);
		font-size: 0.86rem;
	}

	.category-status,
	.feature-status {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		font-family: var(--font-mono);
		font-size: 0.65rem;
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
		font-size: 0.77rem;
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
		font-size: 0.82rem !important;
	}

	.feature-action {
		display: inline-flex;
		align-items: center;
		gap: 0.55rem;
		font-family: var(--font-mono);
		font-size: 0.76rem;
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
		max-width: 17ch;
		margin-bottom: 0.75rem;
		font-size: clamp(1.9rem, 4vw, 3.2rem);
		letter-spacing: -0.045em;
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

		.category-index a:last-child {
			grid-column: 1 / -1;
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
