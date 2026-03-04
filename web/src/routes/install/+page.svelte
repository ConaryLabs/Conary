<svelte:head>
	<title>Install Conary</title>
	<meta name="description" content="Install the Conary cross-distribution package manager on Fedora, Arch, or Ubuntu." />
</svelte:head>

<div class="container page">
	<div class="content">
		<h1 class="animate-in" style="--stagger: 0">Install Conary</h1>
		<p class="lead animate-in" style="--stagger: 1">
			Get up and running in under a minute.
		</p>

		<!-- Quick install -->
		<section class="install-section animate-in" style="--stagger: 2">
			<h2>Quick Install</h2>
			<p>The fastest way to install Conary on any supported distribution:</p>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>curl -fsSL https://conary.io/install.sh | sh</code>
			</div>
			<p class="hint">
				This detects your distribution and installs the latest stable release.
				Review the script before running: <a href="https://conary.io/install.sh">install.sh</a>
			</p>
		</section>

		<!-- From source -->
		<section class="install-section animate-in" style="--stagger: 4">
			<h2>Build from Source</h2>
			<p>Requires Rust 1.85+ and a C compiler:</p>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>git clone https://github.com/ConaryLabs/Conary.git</code>
			</div>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>cd Conary && cargo build --release</code>
			</div>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>sudo install -m 755 target/release/conary /usr/local/bin/</code>
			</div>
		</section>

		<!-- Cargo install -->
		<section class="install-section animate-in" style="--stagger: 6">
			<h2>Via Cargo</h2>
			<p>If you already have the Rust toolchain:</p>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>cargo install conary</code>
			</div>
		</section>

		<!-- Verify -->
		<section class="install-section animate-in" style="--stagger: 8">
			<h2>Verify Installation</h2>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>conary --version</code>
			</div>
			<div class="terminal-block output-block">
				<code>conary 0.1.0</code>
			</div>
		</section>

		<!-- Configure Remi -->
		<section class="install-section animate-in" style="--stagger: 10">
			<h2>Configure Remi</h2>
			<p>
				Point Conary at the Remi conversion proxy to get CCS-converted packages
				from your distribution's upstream repositories:
			</p>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>conary config set proxy https://packages.conary.io</code>
			</div>
			<p>Then install packages as usual -- Remi handles conversion transparently:</p>
			<div class="terminal-block">
				<span class="t-prompt">$</span>
				<code>conary install nginx</code>
			</div>
		</section>

		<!-- System requirements -->
		<section class="install-section animate-in" style="--stagger: 12">
			<h2>System Requirements</h2>
			<div class="req-grid">
				<div class="req-card">
					<h3>Supported Distributions</h3>
					<ul>
						<li>Fedora 42+</li>
						<li>Arch Linux (rolling)</li>
						<li>Ubuntu 24.04+</li>
					</ul>
				</div>
				<div class="req-card">
					<h3>Architecture</h3>
					<ul>
						<li>x86_64 (primary)</li>
						<li>aarch64 (community)</li>
					</ul>
				</div>
				<div class="req-card">
					<h3>Dependencies</h3>
					<ul>
						<li>glibc 2.35+</li>
						<li>SQLite 3.35+</li>
						<li>OpenSSL 3.x or LibreSSL</li>
					</ul>
				</div>
			</div>
		</section>

		<!-- Next steps -->
		<section class="install-section animate-in" style="--stagger: 14">
			<h2>Next Steps</h2>
			<div class="next-links">
				<a href="/packages" class="next-card">
					<span class="next-title">Browse Packages</span>
					<span class="next-desc">Explore {'>'}68k packages across 3 distributions</span>
				</a>
				<a href="/compare" class="next-card">
					<span class="next-title">Compare</span>
					<span class="next-desc">See how Conary stacks up against apt, dnf, and pacman</span>
				</a>
			</div>
		</section>
	</div>
</div>

<style>
	.page {
		padding: 3rem 1.5rem;
	}

	.content {
		max-width: 720px;
	}

	h1 {
		font-family: var(--font-display);
		font-size: 2.5rem;
		font-weight: 800;
		margin-bottom: 0.75rem;
	}

	.lead {
		font-size: 1.125rem;
		color: var(--color-text-secondary);
		font-weight: 300;
		margin-bottom: 3rem;
	}

	.install-section {
		margin-bottom: 3rem;
	}

	.install-section h2 {
		font-family: var(--font-display);
		font-size: 1.375rem;
		font-weight: 700;
		margin-bottom: 0.75rem;
	}

	.install-section p {
		color: var(--color-text-secondary);
		line-height: 1.6;
		margin-bottom: 1rem;
	}

	.terminal-block {
		display: flex;
		align-items: center;
		gap: 0.625rem;
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		padding: 0.875rem 1.125rem;
		margin-bottom: 0.5rem;
	}

	.t-prompt {
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

	.output-block {
		border-left: 2px solid var(--color-border-hover);
		padding-left: 1.25rem;
	}

	.output-block code {
		color: var(--color-text-secondary);
	}

	.hint {
		font-size: 0.8125rem;
		color: var(--color-text-muted);
		margin-top: 0.5rem;
	}

	.hint a {
		color: var(--color-accent);
	}

	.req-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: 0.75rem;
	}

	.req-card {
		padding: 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.req-card h3 {
		font-family: var(--font-display);
		font-size: 0.875rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 0.625rem;
	}

	.req-card ul {
		list-style: none;
		padding: 0;
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		line-height: 1.8;
	}

	.next-links {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 0.75rem;
	}

	.next-card {
		display: flex;
		flex-direction: column;
		padding: 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		text-decoration: none;
		transition: border-color 0.15s, transform 0.15s;
	}

	.next-card:hover {
		border-color: var(--color-accent);
		transform: translateY(-2px);
		text-decoration: none;
	}

	.next-title {
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1rem;
		color: var(--color-accent);
		margin-bottom: 0.25rem;
	}

	.next-desc {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	@media (max-width: 640px) {
		h1 {
			font-size: 2rem;
		}

		.next-links {
			grid-template-columns: 1fr;
		}
	}
</style>
