<svelte:head>
	<title>Conary - The Cross-Distribution Package Manager</title>
	<meta name="description" content="Unified package management across Fedora, Arch, and Ubuntu. Content-addressable storage, atomic transactions, and on-demand format conversion." />
</svelte:head>

<!-- Hero -->
<section class="hero">
	<div class="hero-glow" aria-hidden="true"></div>
	<div class="hero-glow-cool" aria-hidden="true"></div>
	<div class="container hero-inner">
		<h1 class="hero-title animate-in" style="--stagger: 0">Conary</h1>
		<p class="hero-tagline animate-in" style="--stagger: 2">
			The cross-distribution package manager
		</p>
		<p class="hero-desc animate-in" style="--stagger: 3">
			Unified package management across Fedora, Arch, and Ubuntu with
			content-addressable storage, atomic transactions, and on-demand
			format conversion.
		</p>
		<div class="hero-actions animate-in" style="--stagger: 5">
			<a href="/install" class="btn btn-primary">Get Started</a>
			<a href="https://packages.conary.io" class="btn btn-outline">Browse Packages</a>
		</div>
	</div>
</section>

<!-- Stats ribbon -->
<section class="stats-ribbon animate-in" style="--stagger: 7">
	<div class="container stats-inner">
		<div class="stat">
			<span class="stat-value">68k+</span>
			<span class="stat-label">Packages</span>
		</div>
		<div class="stat-divider" aria-hidden="true"></div>
		<div class="stat">
			<span class="stat-value">3</span>
			<span class="stat-label">Distributions</span>
		</div>
		<div class="stat-divider" aria-hidden="true"></div>
		<div class="stat">
			<span class="stat-value">100%</span>
			<span class="stat-label">Atomic</span>
		</div>
		<div class="stat-divider" aria-hidden="true"></div>
		<div class="stat">
			<span class="stat-value">CAS</span>
			<span class="stat-label">Backed</span>
		</div>
	</div>
</section>

<!-- Features -->
<section class="features">
	<div class="container">
		<h2 class="section-title animate-in" style="--stagger: 9">Why Conary?</h2>
		<div class="features-grid">
			<div class="feature-card animate-in" style="--stagger: 10">
				<h3>Cross-Distribution</h3>
				<p>
					One tool for Fedora, Arch, and Ubuntu. Install RPM, DEB, or Arch packages
					with the same commands, the same resolver, the same transaction engine.
				</p>
			</div>
			<div class="feature-card animate-in" style="--stagger: 11">
				<h3>Content-Addressable Storage</h3>
				<p>
					Files are stored by content hash, not by package. Identical files across
					packages are deduplicated automatically, saving disk space and bandwidth.
				</p>
			</div>
			<div class="feature-card animate-in" style="--stagger: 12">
				<h3>Binary Deltas</h3>
				<p>
					Upgrade packages by downloading only what changed. Zstd-compressed binary
					diffs between versions cut update sizes by 60-90%.
				</p>
			</div>
			<div class="feature-card animate-in" style="--stagger: 13">
				<h3>Atomic Transactions</h3>
				<p>
					Every install, remove, and upgrade is a crash-safe atomic transaction.
					Power loss mid-update? Conary recovers automatically on next boot.
				</p>
			</div>
			<div class="feature-card animate-in" style="--stagger: 14">
				<h3>System Model</h3>
				<p>
					Declare the packages your system should have. Conary computes the difference
					and applies it. Reproducible systems from a single file.
				</p>
			</div>
			<div class="feature-card animate-in" style="--stagger: 15">
				<h3>On-Demand Conversion</h3>
				<p>
					Remi, the conversion proxy, transparently converts upstream RPM/DEB/Arch
					packages into CCS format. No upstream changes required.
				</p>
			</div>
		</div>
	</div>
</section>

<!-- Terminal demo -->
<section class="demo">
	<div class="container">
		<h2 class="section-title animate-in" style="--stagger: 16">See It in Action</h2>
		<div class="terminal animate-in" style="--stagger: 17">
			<div class="terminal-header">
				<span class="terminal-dot" aria-hidden="true"></span>
				<span class="terminal-dot" aria-hidden="true"></span>
				<span class="terminal-dot" aria-hidden="true"></span>
				<span class="terminal-title">terminal</span>
			</div>
			<div class="terminal-body">
				<div class="terminal-line">
					<span class="t-prompt">$</span>
					<span class="t-cmd">conary install nginx</span>
				</div>
				<div class="terminal-line t-output">Resolving dependencies...</div>
				<div class="terminal-line t-output">Installing 3 packages (12.4 MB)</div>
				<div class="terminal-line t-output t-success">Transaction complete.</div>
				<div class="terminal-line t-blank"></div>
				<div class="terminal-line">
					<span class="t-prompt">$</span>
					<span class="t-cmd">conary search "web server"</span>
				</div>
				<div class="terminal-line t-output">
					<span class="t-pkg">nginx</span>
					<span class="t-ver">1.27.3</span>
					<span class="t-desc">HTTP and reverse proxy server</span>
				</div>
				<div class="terminal-line t-output">
					<span class="t-pkg">caddy</span>
					<span class="t-ver">2.8.4</span>
					<span class="t-desc">Fast, multi-platform web server with automatic HTTPS</span>
				</div>
				<div class="terminal-line t-output">
					<span class="t-pkg">lighttpd</span>
					<span class="t-ver">1.4.76</span>
					<span class="t-desc">Lightweight high-performance web server</span>
				</div>
				<div class="terminal-line t-blank"></div>
				<div class="terminal-line">
					<span class="t-prompt">$</span>
					<span class="t-cmd">conary model add nginx postgresql redis</span>
				</div>
				<div class="terminal-line t-output t-success">System model updated. 3 packages added.</div>
			</div>
		</div>
	</div>
</section>

<!-- Comparison teaser -->
<section class="comparison-teaser">
	<div class="container">
		<h2 class="section-title animate-in" style="--stagger: 18">How It Compares</h2>
		<div class="compare-table-wrap animate-in" style="--stagger: 19">
			<table class="compare-table">
				<thead>
					<tr>
						<th>Feature</th>
						<th class="highlight">Conary</th>
						<th>apt</th>
						<th>dnf</th>
						<th>pacman</th>
						<th>Nix</th>
					</tr>
				</thead>
				<tbody>
					<tr>
						<td class="feature-name">Cross-distribution</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
					</tr>
					<tr>
						<td class="feature-name">CAS storage</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check yes"></span></td>
					</tr>
					<tr>
						<td class="feature-name">Binary deltas</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check partial"></span></td>
						<td><span class="check partial"></span></td>
						<td><span class="check no"></span></td>
					</tr>
					<tr>
						<td class="feature-name">Atomic transactions</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check yes"></span></td>
					</tr>
					<tr>
						<td class="feature-name">Declarative system model</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check yes"></span></td>
					</tr>
					<tr>
						<td class="feature-name">On-demand conversion</td>
						<td class="highlight"><span class="check yes"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
						<td><span class="check no"></span></td>
					</tr>
				</tbody>
			</table>
		</div>
		<div class="compare-cta animate-in" style="--stagger: 20">
			<a href="/compare" class="btn btn-outline">Full Comparison</a>
		</div>
	</div>
</section>

<!-- CTA -->
<section class="cta">
	<div class="container cta-inner">
		<h2 class="animate-in" style="--stagger: 21">Ready to get started?</h2>
		<p class="animate-in" style="--stagger: 22">
			Install Conary in under a minute and unify your package management.
		</p>
		<div class="cta-actions animate-in" style="--stagger: 23">
			<a href="/install" class="btn btn-primary">Install Conary</a>
			<a href="https://github.com/ConaryLabs/Conary" target="_blank" rel="noopener noreferrer" class="btn btn-outline">View on GitHub</a>
		</div>
	</div>
</section>

<style>
	.hero {
		position: relative;
		padding: 6rem 0 5rem;
		text-align: center;
		overflow: hidden;
	}

	.hero-glow {
		position: absolute;
		top: -300px;
		left: 50%;
		transform: translateX(-50%);
		width: 1000px;
		height: 700px;
		background: radial-gradient(ellipse, rgba(232, 133, 61, 0.07) 0%, transparent 60%);
		pointer-events: none;
	}

	.hero-glow-cool {
		position: absolute;
		top: -150px;
		left: 20%;
		width: 600px;
		height: 500px;
		background: radial-gradient(circle, rgba(23, 147, 209, 0.035) 0%, transparent 60%);
		pointer-events: none;
	}

	.hero-inner {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: center;
	}

	.hero-title {
		font-family: var(--font-display);
		font-size: 5rem;
		font-weight: 800;
		letter-spacing: -0.05em;
		margin-bottom: 0.5rem;
		background: linear-gradient(135deg, var(--color-text) 0%, var(--color-accent) 100%);
		-webkit-background-clip: text;
		-webkit-text-fill-color: transparent;
		background-clip: text;
	}

	.hero-tagline {
		font-family: var(--font-display);
		font-size: 1.5rem;
		font-weight: 600;
		color: var(--color-text);
		margin: 0 0 1rem;
		letter-spacing: -0.02em;
	}

	.hero-desc {
		font-size: 1.0625rem;
		color: var(--color-text-secondary);
		max-width: 580px;
		margin: 0 0 2.5rem;
		line-height: 1.7;
		font-weight: 300;
	}

	.hero-actions, .cta-actions {
		display: flex;
		gap: 0.75rem;
	}

	.btn {
		display: inline-flex;
		align-items: center;
		padding: 0.75rem 1.5rem;
		border-radius: var(--radius-md);
		font-size: 0.9375rem;
		font-weight: 600;
		text-decoration: none;
		transition: all 0.15s;
	}

	.btn-primary {
		background: var(--color-accent);
		color: var(--color-bg);
	}

	.btn-primary:hover {
		background: var(--color-accent-hover);
		color: var(--color-bg);
		transform: translateY(-1px);
		box-shadow: var(--shadow-glow);
	}

	.btn-outline {
		background: transparent;
		color: var(--color-text);
		border: 1px solid var(--color-border-hover);
	}

	.btn-outline:hover {
		border-color: var(--color-accent);
		color: var(--color-accent);
		transform: translateY(-1px);
	}

	.stats-ribbon {
		padding: 1.75rem 0;
		border-top: 1px solid var(--color-border);
		border-bottom: 1px solid var(--color-border);
		background: var(--color-bg-subtle);
	}

	.stats-inner {
		display: flex;
		justify-content: center;
		align-items: center;
		gap: 2.5rem;
	}

	.stat {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.125rem;
	}

	.stat-value {
		font-family: var(--font-mono);
		font-size: 1.5rem;
		font-weight: 700;
		color: var(--color-accent);
		text-shadow: 0 0 30px var(--color-accent-glow);
	}

	.stat-label {
		font-size: 0.6875rem;
		color: var(--color-text-muted);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		font-weight: 500;
	}

	.stat-divider {
		width: 1px;
		height: 2rem;
		background: var(--color-border);
	}

	.section-title {
		font-family: var(--font-display);
		font-size: 1.75rem;
		font-weight: 700;
		text-align: center;
		margin-bottom: 2rem;
	}

	.features { padding: 5rem 0; }

	.features-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 1rem;
	}

	.feature-card {
		padding: 1.5rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		transition: border-color 0.15s, transform 0.15s;
	}

	.feature-card:hover {
		border-color: var(--color-border-hover);
		transform: translateY(-2px);
	}

	.feature-card h3 {
		font-family: var(--font-display);
		font-size: 1rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 0.5rem;
	}

	.feature-card p {
		font-size: 0.875rem;
		color: var(--color-text-secondary);
		line-height: 1.6;
		margin: 0;
	}

	.demo { padding: 3rem 0 5rem; }

	.terminal {
		max-width: 680px;
		margin: 0 auto;
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		overflow: hidden;
		box-shadow: var(--shadow-lg);
	}

	.terminal-header {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		padding: 0.75rem 1rem;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-border);
	}

	.terminal-dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--color-text-muted);
		opacity: 0.4;
	}

	.terminal-title {
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		color: var(--color-text-muted);
		margin-left: 0.5rem;
	}

	.terminal-body {
		padding: 1.25rem 1.5rem;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		line-height: 1.8;
	}

	.terminal-line { white-space: nowrap; overflow: hidden; }
	.t-prompt { color: var(--color-accent); font-weight: 500; margin-right: 0.625rem; user-select: none; }
	.t-cmd { color: var(--color-text); }
	.t-output { color: var(--color-text-secondary); padding-left: 1.375rem; }
	.t-success { color: var(--color-success); }
	.t-blank { height: 0.5rem; }
	.t-pkg { color: var(--color-accent); display: inline-block; min-width: 10ch; }
	.t-ver { color: var(--color-text-muted); display: inline-block; min-width: 8ch; }
	.t-desc { color: var(--color-text-secondary); }

	.comparison-teaser {
		padding: 5rem 0;
		background: var(--color-bg-subtle);
		border-top: 1px solid var(--color-border);
		border-bottom: 1px solid var(--color-border);
	}

	.compare-table-wrap { overflow-x: auto; max-width: 740px; margin: 0 auto; }

	.compare-table { width: 100%; border-collapse: collapse; font-size: 0.8125rem; }

	.compare-table th {
		padding: 0.75rem 0.875rem;
		text-align: center;
		font-size: 0.6875rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--color-text-muted);
		font-weight: 600;
		border-bottom: 1px solid var(--color-border);
	}

	.compare-table th:first-child { text-align: left; }
	.compare-table th.highlight { color: var(--color-accent); }

	.compare-table td {
		padding: 0.625rem 0.875rem;
		text-align: center;
		border-bottom: 1px solid var(--color-border);
	}

	.compare-table td.highlight { background: var(--color-accent-subtle); }
	.feature-name { text-align: left; font-weight: 500; color: var(--color-text); }

	.check { display: inline-block; width: 8px; height: 8px; border-radius: 50%; }
	.check.yes { background: var(--color-success); box-shadow: 0 0 6px rgba(52, 211, 153, 0.4); }
	.check.no { background: var(--color-text-muted); opacity: 0.3; }
	.check.partial { background: var(--color-warning); opacity: 0.7; }

	.compare-cta { text-align: center; margin-top: 2rem; }

	.cta { padding: 5rem 0; }
	.cta-inner { text-align: center; }

	.cta-inner h2 {
		font-family: var(--font-display);
		font-size: 2rem;
		font-weight: 800;
		margin-bottom: 0.75rem;
	}

	.cta-inner p {
		font-size: 1.0625rem;
		color: var(--color-text-secondary);
		margin-bottom: 2rem;
		font-weight: 300;
	}

	.cta-actions { justify-content: center; }

	@media (max-width: 768px) {
		.hero { padding: 4rem 0 3rem; }
		.hero-title { font-size: 3.25rem; }
		.hero-tagline { font-size: 1.25rem; }
		.features-grid { grid-template-columns: 1fr; }
		.stats-inner { gap: 1.25rem; flex-wrap: wrap; }
		.stat-divider { display: none; }
		.terminal-body { font-size: 0.75rem; overflow-x: auto; }
	}
</style>
