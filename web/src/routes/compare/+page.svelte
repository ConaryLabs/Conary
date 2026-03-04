<svelte:head>
	<title>Compare Conary - vs apt, dnf, pacman, Nix</title>
	<meta name="description" content="See how Conary compares to apt, dnf, pacman, and Nix across features like cross-distribution support, CAS storage, and atomic transactions." />
</svelte:head>

<div class="container page">
	<div class="content">
		<h1 class="animate-in" style="--stagger: 0">How Conary Compares</h1>
		<p class="lead animate-in" style="--stagger: 1">
			A honest look at how Conary stacks up against established package managers.
		</p>

		<!-- Full comparison table -->
		<section class="compare-section animate-in" style="--stagger: 2">
			<h2>Feature Matrix</h2>
			<div class="table-wrap">
				<table>
					<thead>
						<tr>
							<th class="col-feature">Feature</th>
							<th class="col-hl">Conary</th>
							<th>apt</th>
							<th>dnf</th>
							<th>pacman</th>
							<th>Nix</th>
						</tr>
					</thead>
					<tbody>
						<tr>
							<td class="fname">Cross-distribution</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">Content-addressable storage</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="y"></span></td>
						</tr>
						<tr>
							<td class="fname">Binary delta updates</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="p" title="deltarpm (limited)"></span></td>
							<td><span class="p" title="xdelta (limited)"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">Atomic transactions</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="y"></span></td>
						</tr>
						<tr>
							<td class="fname">Declarative system model</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="y"></span></td>
						</tr>
						<tr>
							<td class="fname">SAT dependency resolver</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="y"></span></td>
							<td><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">On-demand format conversion</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">P2P chunk federation</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">Rollback / undo</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="p" title="via history undo"></span></td>
							<td><span class="n"></span></td>
							<td><span class="y"></span></td>
						</tr>
						<tr>
							<td class="fname">Sandboxed scriptlets</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="n"></span></td>
							<td><span class="y"></span></td>
						</tr>
						<tr>
							<td class="fname">Supply chain trust (TUF)</td>
							<td class="col-hl"><span class="y"></span></td>
							<td><span class="p" title="GPG signing"></span></td>
							<td><span class="p" title="GPG signing"></span></td>
							<td><span class="p" title="GPG signing"></span></td>
							<td><span class="n"></span></td>
						</tr>
						<tr>
							<td class="fname">Established ecosystem</td>
							<td class="col-hl"><span class="n"></span></td>
							<td><span class="y"></span></td>
							<td><span class="y"></span></td>
							<td><span class="y"></span></td>
							<td><span class="y"></span></td>
						</tr>
					</tbody>
				</table>
			</div>
			<div class="legend">
				<span><span class="y"></span> Full support</span>
				<span><span class="p"></span> Partial / limited</span>
				<span><span class="n"></span> Not available</span>
			</div>
		</section>

		<!-- Individual comparisons -->
		<section class="compare-section animate-in" style="--stagger: 4">
			<h2>Conary vs apt</h2>
			<p>
				apt is the standard on Debian and Ubuntu -- battle-tested over two decades.
				It does one thing well: manage .deb packages on Debian-derived systems.
			</p>
			<p>
				Conary extends that scope to any distribution. The same <code>conary install</code>
				command works whether the upstream package is RPM, DEB, or Arch. Conary also brings
				atomic transactions (apt can leave a system in a broken state if interrupted),
				content-addressable storage for deduplication, and binary deltas to cut download sizes.
			</p>
			<p>
				The tradeoff: apt has 20+ years of ecosystem maturity and is pre-installed on every
				Debian/Ubuntu system. Conary is new.
			</p>
		</section>

		<section class="compare-section animate-in" style="--stagger: 6">
			<h2>Conary vs dnf</h2>
			<p>
				dnf is Fedora's package manager with a solid SAT solver and built-in deltarpm support.
				It's well-designed for the RPM ecosystem.
			</p>
			<p>
				Conary offers binary deltas with finer granularity (chunk-level vs. package-level),
				CAS-backed storage for cross-package deduplication, and cross-distro support.
				dnf's <code>history undo</code> provides limited rollback; Conary's journal-based
				transactions provide full crash recovery.
			</p>
		</section>

		<section class="compare-section animate-in" style="--stagger: 8">
			<h2>Conary vs pacman</h2>
			<p>
				pacman is fast, minimal, and perfectly tailored to Arch's rolling-release model.
				Its simplicity is its strength -- no dependency resolver complexity, no plugin system.
			</p>
			<p>
				Conary adds the safety net that pacman lacks: atomic transactions mean a power
				failure during <code>-Syu</code> won't brick your system. CAS storage and binary deltas
				reduce the bandwidth cost of rolling updates. And if you run multiple distros,
				Conary provides one tool instead of maintaining separate muscle memory.
			</p>
		</section>

		<section class="compare-section animate-in" style="--stagger: 10">
			<h2>Conary vs Nix</h2>
			<p>
				Nix pioneered many ideas Conary draws from: content-addressable storage,
				atomic operations, declarative system configuration, and reproducible builds.
				It's a technically excellent system.
			</p>
			<p>
				The key difference is approach. Nix requires repackaging the world into its own format --
				a massive undertaking that creates a parallel ecosystem. Conary works with existing
				distribution packages via Remi's on-demand conversion. You get CAS benefits without
				abandoning your distro's package ecosystem.
			</p>
			<p>
				Nix also has a steep learning curve (its own language, store paths, derivations).
				Conary aims for familiar package manager UX: <code>install</code>,
				<code>remove</code>, <code>search</code>.
			</p>
		</section>

		<!-- When to use -->
		<section class="compare-section animate-in" style="--stagger: 12">
			<h2>When to Use Conary</h2>
			<div class="use-grid">
				<div class="use-card good">
					<h3>Good fit</h3>
					<ul>
						<li>You manage multiple Linux distributions</li>
						<li>You want atomic, crash-safe package operations</li>
						<li>You want delta updates to save bandwidth</li>
						<li>You like declarative system configuration but not Nix's complexity</li>
						<li>You want CAS deduplication across packages</li>
					</ul>
				</div>
				<div class="use-card neutral">
					<h3>Maybe not yet</h3>
					<ul>
						<li>You need a production-proven tool with years of track record</li>
						<li>You rely on distribution-specific tooling (PPAs, COPRs, AUR helpers)</li>
						<li>You're happy with your current package manager and don't need cross-distro</li>
					</ul>
				</div>
			</div>
		</section>

		<!-- CTA -->
		<section class="compare-section animate-in" style="--stagger: 14">
			<div class="bottom-cta">
				<a href="/install" class="btn btn-primary">Install Conary</a>
				<a href="/packages" class="btn btn-outline">Browse Packages</a>
			</div>
		</section>
	</div>
</div>

<style>
	.page {
		padding: 3rem 1.5rem;
	}

	.content {
		max-width: 780px;
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

	.compare-section {
		margin-bottom: 3rem;
	}

	.compare-section h2 {
		font-family: var(--font-display);
		font-size: 1.375rem;
		font-weight: 700;
		margin-bottom: 1rem;
	}

	.compare-section p {
		color: var(--color-text-secondary);
		line-height: 1.7;
		margin-bottom: 1rem;
	}

	.compare-section p code {
		font-size: 0.85em;
	}

	/* Table */
	.table-wrap {
		overflow-x: auto;
		margin-bottom: 1rem;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.8125rem;
	}

	th {
		padding: 0.75rem 0.75rem;
		text-align: center;
		font-size: 0.6875rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--color-text-muted);
		font-weight: 600;
		border-bottom: 1px solid var(--color-border);
	}

	th.col-feature {
		text-align: left;
		min-width: 180px;
	}

	th.col-hl {
		color: var(--color-accent);
	}

	td {
		padding: 0.5rem 0.75rem;
		text-align: center;
		border-bottom: 1px solid var(--color-border);
	}

	td.col-hl {
		background: var(--color-accent-subtle);
	}

	.fname {
		text-align: left;
		font-weight: 500;
		color: var(--color-text);
	}

	/* Indicators */
	.y, .n, .p {
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}

	.y {
		background: var(--color-success);
		box-shadow: 0 0 6px rgba(52, 211, 153, 0.4);
	}

	.n {
		background: var(--color-text-muted);
		opacity: 0.25;
	}

	.p {
		background: var(--color-warning);
		opacity: 0.7;
	}

	.legend {
		display: flex;
		gap: 1.5rem;
		font-size: 0.75rem;
		color: var(--color-text-muted);
	}

	.legend span {
		display: inline-flex;
		align-items: center;
		gap: 0.375rem;
	}

	/* Use cases */
	.use-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
	}

	.use-card {
		padding: 1.5rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.use-card h3 {
		font-family: var(--font-display);
		font-size: 0.9375rem;
		font-weight: 700;
		margin-bottom: 0.75rem;
	}

	.use-card.good h3 {
		color: var(--color-success);
	}

	.use-card.neutral h3 {
		color: var(--color-text-secondary);
	}

	.use-card ul {
		list-style: none;
		padding: 0;
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		line-height: 1.8;
	}

	.use-card li::before {
		content: '--';
		color: var(--color-text-muted);
		margin-right: 0.5rem;
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	/* Buttons */
	.btn {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
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
	}

	.bottom-cta {
		display: flex;
		gap: 0.75rem;
		padding-top: 1rem;
	}

	@media (max-width: 640px) {
		h1 {
			font-size: 2rem;
		}

		.use-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
