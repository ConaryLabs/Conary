<script lang="ts">
	import PageMeta from '$lib/components/PageMeta.svelte';
</script>

<PageMeta
	title="About Remi and the package index — Conary"
	description="Learn how the public package index relates to Conary, Remi, upstream repositories, and source-independent CCS conversion."
	path="/about"
/>

<div class="container page">
	<div class="content">
		<p class="eyebrow">System boundary</p>
		<h1 class="animate-in" style="--stagger: 0">About Conary</h1>

		<p class="lead animate-in" style="--stagger: 1">
			Conary is an early cross-distribution Linux package-manager preview for supported
			Fedora, Arch Linux, and Ubuntu test hosts.
		</p>

		<p class="animate-in" style="--stagger: 2">
			The supported tester path installs a package whose source format differs from the
			host's native package format. Conary owns that package operation; dnf, apt, or
			pacman is involved only when the user explicitly adopts or takes over existing
			native state. CCS uses content-addressable storage, while EROFS generations remain
			a separate advanced workflow.
		</p>

		<h2>What is Remi?</h2>

		<p>
			Remi is Conary's on-demand conversion proxy. It sits between your system and upstream
			distribution repositories and converts RPM, DEB, and Arch packages into CCS while
			preserving their exact source lifecycle, dependency, version, payload, and
			configuration contracts. A first conversion may be slower than a cache hit; a
			missing typed model is an engineering error, not an indefinite review state.
		</p>

		<h2>Key Features</h2>

		<div class="features-grid">
			<div class="feature">
				<h3>Cross-distro package execution</h3>
				<p>Run RPM, DEB, and Arch source semantics through Conary on any supported target</p>
			</div>
			<div class="feature">
				<h3>Explicit apply intent</h3>
				<p>Preview active-host changes with --dry-run and confirm supported mutations with --yes</p>
			</div>
			<div class="feature">
				<h3>Cross-distribution</h3>
				<p>Use one CLI and index across supported Fedora, Arch, and Ubuntu targets</p>
			</div>
			<div class="feature">
				<h3>Content-addressable storage</h3>
				<p>Store Conary-owned content by hash and reuse identical files across packages</p>
			</div>
			<div class="feature">
				<h3>Exact conversion contract</h3>
				<p>Serve current-schema artifacts from typed parsers and lifecycle ABIs, never string heuristics</p>
			</div>
			<div class="feature">
				<h3>Advanced paths stay separate</h3>
				<p>Generation, model-apply, conaryd, and federation work is outside the reliable preview loop</p>
			</div>
		</div>

		<h2>Getting Started</h2>

		<p>Install a package with Conary:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>sudo conary install nginx --dry-run</code>
		</div>

		<p>Search for packages:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>conary search "web server"</code>
		</div>

		<p>View package details:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>conary query repquery nginx --info</code>
		</div>

		<h2>Package Index</h2>

		<p>
			This web interface provides a searchable index of packages served publicly through Remi.
			You can browse packages by distribution, search by name or description, and view
			detailed information including versions, dependencies, and download statistics.
		</p>

		<div class="links-row">
			<a href="/packages" class="link-card">
				<span class="link-label">Browse packages</span>
				<svg viewBox="0 0 20 20" fill="currentColor" width="16" height="16" aria-hidden="true">
					<path fill-rule="evenodd" d="M10.293 3.293a1 1 0 011.414 0l6 6a1 1 0 010 1.414l-6 6a1 1 0 01-1.414-1.414L14.586 11H3a1 1 0 110-2h11.586l-4.293-4.293a1 1 0 010-1.414z" clip-rule="evenodd" />
				</svg>
			</a>
			<a href="/stats" class="link-card">
				<span class="link-label">View statistics</span>
				<svg viewBox="0 0 20 20" fill="currentColor" width="16" height="16" aria-hidden="true">
					<path fill-rule="evenodd" d="M10.293 3.293a1 1 0 011.414 0l6 6a1 1 0 010 1.414l-6 6a1 1 0 01-1.414-1.414L14.586 11H3a1 1 0 110-2h11.586l-4.293-4.293a1 1 0 010-1.414z" clip-rule="evenodd" />
				</svg>
			</a>
		</div>
	</div>
</div>

<style>
	.page {
		padding-block: clamp(3rem, 7vw, 5.5rem);
	}

	.content {
		max-width: 860px;
	}

	h1 {
		font-size: clamp(2.8rem, 7vw, 5.5rem);
		font-weight: 700;
		letter-spacing: -0.055em;
		margin-bottom: 1.25rem;
	}

	h2 {
		font-size: clamp(1.55rem, 3vw, 2.2rem);
		font-weight: 700;
		margin-top: 3rem;
		margin-bottom: 1rem;
	}

	p {
		line-height: 1.7;
		color: var(--color-text-secondary);
		margin-bottom: 1rem;
	}

	.lead {
		font-size: 1.125rem;
		color: var(--color-text);
		font-weight: 300;
	}

	.features-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		gap: 1rem;
		margin: 1rem 0;
	}

	.feature {
		padding: 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
	}

	.feature h3 {
		font-size: 0.9375rem;
		font-weight: 700;
		margin-bottom: 0.375rem;
		color: var(--color-cyan);
	}

	.feature p {
		font-size: 0.8125rem;
		margin: 0;
		line-height: 1.5;
	}

	.terminal-block {
		display: flex;
		align-items: center;
		gap: 0.625rem;
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-left: 3px solid var(--color-orange);
		padding: 0.875rem 1.125rem;
		margin-bottom: 1.5rem;
	}

	.terminal-prompt {
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--color-orange);
		font-weight: 500;
		user-select: none;
	}

	.terminal-block code {
		background: none;
		padding: 0;
		color: var(--color-text);
		font-size: 0.875rem;
	}

	.links-row {
		display: flex;
		gap: 0.75rem;
		margin-top: 1.5rem;
	}

	.link-card {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.75rem 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		text-decoration: none;
		color: var(--color-cyan);
		font-size: 0.875rem;
		font-weight: 500;
		transition: border-color 0.15s, box-shadow 0.15s, transform 0.15s;
	}

	.link-card:hover {
		border-color: var(--color-cyan);
		background: var(--color-accent-subtle);
		transform: translateY(-1px);
		text-decoration: none;
	}

	@media (max-width: 560px) {
		.links-row {
			flex-direction: column;
		}
	}
</style>
