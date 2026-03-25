<svelte:head>
	<title>About - Conary Package Index</title>
	<meta name="description" content="Learn about Conary, the cross-distribution package manager, and Remi, the on-demand conversion proxy." />
</svelte:head>

<div class="container page">
	<div class="content">
		<h1 class="animate-in" style="--stagger: 0">About Conary</h1>

		<p class="lead animate-in" style="--stagger: 1">
			Conary is a cross-distribution Linux package manager that unifies package management
			across Fedora, Arch Linux, and Ubuntu.
		</p>

		<p class="animate-in" style="--stagger: 2">
			Every operation produces an immutable EROFS image mounted via Linux composefs,
			with kernel-enforced integrity (fs-verity) on every file read. Conary uses the
			CCS (Conary Container Store) native format with content-addressable storage and
			EROFS binary deltas for efficient updates.
		</p>

		<h2>What is Remi?</h2>

		<p>
			Remi is Conary's on-demand conversion proxy. It sits between your system and upstream
			distribution repositories, transparently converting legacy packages (RPM, DEB, Arch)
			into the CCS format. This gives you the benefits of CCS -- content-addressable storage,
			binary deltas, hermetic builds -- without requiring upstream distributions to change
			anything.
		</p>

		<h2>Key Features</h2>

		<div class="features-grid">
			<div class="feature">
				<h3>Composefs-native</h3>
				<p>Every operation builds an immutable EROFS image mounted via composefs</p>
			</div>
			<div class="feature">
				<h3>Kernel-enforced integrity</h3>
				<p>fs-verity on every file -- the kernel verifies content on each read</p>
			</div>
			<div class="feature">
				<h3>Cross-distribution</h3>
				<p>Manage packages from Fedora, Arch, and Ubuntu with a single tool</p>
			</div>
			<div class="feature">
				<h3>Content-addressable storage</h3>
				<p>Deduplicate files across packages using CAS</p>
			</div>
			<div class="feature">
				<h3>Instant rollback</h3>
				<p>Remount a previous generation -- no file copying, no rebuilding</p>
			</div>
			<div class="feature">
				<h3>CAS federation</h3>
				<p>Peer-to-peer chunk sharing for bandwidth savings</p>
			</div>
		</div>

		<h2>Getting Started</h2>

		<p>Install a package with Conary:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>conary install nginx</code>
		</div>

		<p>Search for packages:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>conary search "web server"</code>
		</div>

		<p>View package details:</p>
		<div class="terminal-block">
			<span class="terminal-prompt">$</span>
			<code>conary info nginx</code>
		</div>

		<h2>Package Index</h2>

		<p>
			This web interface provides a searchable index of all packages available through Remi.
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
		padding: 2.5rem 1.5rem;
	}

	.content {
		max-width: 720px;
	}

	h1 {
		font-family: var(--font-display);
		font-size: 2.25rem;
		font-weight: 800;
		margin-bottom: 1.25rem;
	}

	h2 {
		font-family: var(--font-display);
		font-size: 1.375rem;
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
		border-radius: var(--radius-md);
	}

	.feature h3 {
		font-family: var(--font-display);
		font-size: 0.9375rem;
		font-weight: 700;
		margin-bottom: 0.375rem;
		color: var(--color-accent);
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
		border-radius: var(--radius-md);
		padding: 0.875rem 1.125rem;
		margin-bottom: 1.5rem;
	}

	.terminal-prompt {
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--color-accent);
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
		border-radius: var(--radius-md);
		text-decoration: none;
		color: var(--color-accent);
		font-size: 0.875rem;
		font-weight: 500;
		transition: border-color 0.15s, box-shadow 0.15s, transform 0.15s;
	}

	.link-card:hover {
		border-color: var(--color-accent);
		box-shadow: 0 0 0 3px var(--color-accent-subtle);
		transform: translateY(-1px);
		text-decoration: none;
	}
</style>
