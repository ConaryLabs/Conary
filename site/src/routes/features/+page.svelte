<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
</script>

<PageMeta
	title="Product vision and features — Conary"
	description="Explore Conary's cross-distro package model, current capabilities, and the evidence behind each package and system-management layer."
	path="/features/"
/>

<PageIntro
	eyebrow="Product vision and capability map"
	title="The package manager is only the beginning."
	description="Conary starts by making RPM, DEB, and Arch packages source-independent, then connects that package model to building, describing, moving, and recovering Linux systems. The evidence labels show which parts you can rely on today."
/>

<nav class="category-index" aria-label="Feature maturity groups">
	<div class="container">
		<a href="#direction">Direction</a>
		<a href="#preview-supported">Preview</a>
		<a href="#owned-packages">Package core</a>
		<a href="#vm-generations">Systems</a>
		<a href="#experimental">Infrastructure</a>
		<a href="#roadmap">Horizon</a>
	</div>
</nav>

<section class="features-page">
	<div class="container features-content">
		<div class="category category-direction" id="direction">
			<div class="category-heading">
				<span class="category-status preview">product direction</span>
				<h2 class="category-title">What Conary is building toward</h2>
				<p>A connected system, not a pile of package-manager features.</p>
			</div>

			<div class="feature-list direction-list">
				<article class="feature-card feature-lead">
					<span class="feature-status preview">foundation available</span>
					<h3>Source-native semantics, distribution-independent execution</h3>
					<p>
						Install RPM, DEB, and Arch artifacts through one engine while their source
						format retains version comparison, dependency relations, lifecycle calls,
						payload metadata, and configuration rules. Target capabilities are explicit
						inputs; the host distro does not rewrite the package ABI.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status preview">working today</span>
					<h3>Gradual authority transfer</h3>
					<p>
						Start with the machine already running. Conary can observe native packages,
						track them reversibly, and keep ownership visible so migration becomes a series
						of deliberate boundaries instead of one destructive cutover.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">post-milestone direction</span>
					<h3>Third-party package building and publishing</h3>
					<p>
						Turn the existing recipe, isolated-build, CCS, signing, and static-publication
						machinery into a coherent workflow that other projects can use—not only an
						internal bootstrap pipeline.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status limited">available · limited</span>
					<h3>Declarative systems and replatform planning</h3>
					<p>
						Describe desired package state, compare it with a host, lock inputs, and connect
						models to builders and source selection. Over time, that same model can turn a
						distribution move into an inspectable plan rather than a fresh-install ritual.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status vm">VM evidence · limited</span>
					<h3>Selectable system artifacts and explicit trust</h3>
					<p>
						Compose package state into content-reusing generations, carry the boot assets
						needed to run them, and make selection, export, verification, and recovery
						explicit. Current proof is x86_64 and VM-focused; signed boot authority and
						broader trust are later work.
					</p>
				</article>

				<article class="feature-card">
					<span class="feature-status experimental">service today · research ahead</span>
					<h3>Shared package services and automation</h3>
					<p>
						Remi already indexes package metadata and serves policy-approved conversions.
						The longer path connects reliable operator services, agent-facing operations,
						and trusted federation without making hidden network authority the default.
					</p>
				</article>
				<p class="direction-note">
					This is product direction, not an ordered release promise. Post-preview priorities
					will be selected from external tester evidence.
				</p>
			</div>
		</div>

		<div class="category category-preview" id="preview-supported">
			<div class="category-heading">
				<span class="category-status preview">preview-supported</span>
				<h2 class="category-title">The bounded tester loop</h2>
				<p>Cross-distro packages with a documented, inspectable transaction workflow.</p>
			</div>

			<div class="feature-list">
				<article class="feature-card feature-lead">
					<span class="feature-status preview">supported path</span>
					<h3>Cross-distro package install</h3>
					<p>
							Fedora 44, Ubuntu 26.04 LTS, and Arch can consume RPM, DEB, Arch, and
							CCS inputs through the same Conary transaction path. Dry-run shows the
							source ABI and typed target capabilities before automatic target preflight.
					</p>
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal command list needs keyboard scrolling) -->
					<div class="feature-code scroll-region" role="region" tabindex="0" aria-label="Cross-distro package install commands">
							<code>sudo conary install ./package.rpm --dry-run</code>
							<code>sudo conary install ./package.deb --yes</code>
							<code>sudo conary install ./package.pkg.tar.zst --yes</code>
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
					<span class="feature-status limited">cross-distro core</span>
					<h3>RPM, DEB, Arch, and CCS inputs</h3>
					<p>
						Conary converts each supported source format into CCS without erasing its
						native lifecycle ABI, then executes that transaction on any supported target
						that supplies the declared capabilities. Missing capability models are
						engineering gaps, not string-matching review policy.
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
				<span class="category-status roadmap">longer horizon</span>
				<h2 class="category-title">What still needs its proof</h2>
				<p>Valuable directions that are not capabilities to plan production work around yet.</p>
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
					<span class="feature-status roadmap">proof expansion</span>
					<h3>Broader distro certification</h3>
					<p>
						The source-independent package contract is not tied to three distro names,
						but release claims still need installed-binary matrix proof on additional
						mainstream targets and architectures before they join the supported list.
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
				<p class="eyebrow">The first step is ready</p>
				<h2>Try the reversible foundation of the larger system.</h2>
				<p>The install runbook pins the package, checksum, command order, and feedback contract so the vision begins with real evidence.</p>
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

	.direction-note {
		padding: 1rem 1.1rem;
		border-left: 3px solid var(--color-cyan);
		color: var(--color-muted) !important;
		background: rgb(70 199 211 / 6%);
		font-family: var(--font-mono);
		font-size: 0.72rem !important;
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
