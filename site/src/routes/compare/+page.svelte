<svelte:head>
	<title>Compare - Conary</title>
	<meta name="description" content="See how Conary compares to apt, dnf, pacman, and Nix." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">How Conary Compares</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			An honest look at how Conary stacks up against existing package managers.
		</p>
	</div>
</section>

<section class="compare-section">
	<div class="container">
		<!-- Feature matrix -->
		<div class="compare-block animate-in" style="--stagger: 3">
			<div class="compare-table-wrap">
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
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check partial" aria-label="Partial"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Content-addressable storage</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Binary delta updates</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check partial" aria-label="Partial"></span></td>
							<td><span class="check partial" aria-label="Partial"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Atomic transactions</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Declarative system model</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">On-demand format conversion</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
						</tr>
						<tr>
							<td class="feature-name">File-level deduplication</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Rollback / undo</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check partial" aria-label="Partial"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">SAT-based resolution</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Uses existing upstream repos</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Immutable generations</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
						<tr>
							<td class="feature-name">System takeover</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
						</tr>
						<tr>
							<td class="feature-name">Bootstrap from scratch</td>
							<td class="highlight"><span class="check yes" aria-label="Yes"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check no" aria-label="No"></span></td>
							<td><span class="check yes" aria-label="Yes"></span></td>
						</tr>
					</tbody>
				</table>
			</div>
			<div class="legend animate-in" style="--stagger: 4">
				<span class="legend-item"><span class="check yes" aria-label="Yes"></span> Yes</span>
				<span class="legend-item"><span class="check partial" aria-label="Partial"></span> Partial</span>
				<span class="legend-item"><span class="check no" aria-label="No"></span> No</span>
			</div>
		</div>

		<!-- Detailed comparisons -->
		<div class="details">
			<div class="detail-card animate-in" style="--stagger: 5">
				<h2>vs. apt <span class="detail-distro">(Debian / Ubuntu)</span></h2>
				<p>
					apt is the workhorse of the Debian ecosystem -- battle-tested and reliable. Conary
					doesn't replace apt on Debian systems; it sits alongside it. The key differences:
					Conary adds content-addressable storage for deduplication, binary delta updates to
					reduce bandwidth, atomic transactions that survive crashes, and the ability to manage
					Fedora or Arch packages on the same machine.
				</p>
				<p>
					If you only run Ubuntu and don't need cross-distro support, apt works great. Conary
					becomes valuable when you want unified management, disk savings from CAS, or crash
					safety.
				</p>
			</div>

			<div class="detail-card animate-in" style="--stagger: 6">
				<h2>vs. dnf <span class="detail-distro">(Fedora / RHEL)</span></h2>
				<p>
					dnf is a capable package manager with delta RPM support and a SAT solver. Conary
					shares the SAT-based approach (via resolvo) and adds CAS deduplication, full
					atomic transactions (not just RPM's limited rollback), and cross-distro package
					management. dnf's delta RPM support is partial -- it depends on upstream providing
					delta files. Conary generates binary deltas on-demand from any two versions.
				</p>
			</div>

			<div class="detail-card animate-in" style="--stagger: 7">
				<h2>vs. pacman <span class="detail-distro">(Arch Linux)</span></h2>
				<p>
					pacman is fast, minimal, and trusts the user. Conary respects that philosophy while
					adding features pacman lacks: atomic transactions (no more broken systems from
					interrupted updates), content-addressable storage, dependency resolution through SAT
					solving, and the ability to install packages from Fedora or Ubuntu repos alongside
					Arch packages.
				</p>
			</div>

			<div class="detail-card animate-in" style="--stagger: 8">
				<h2>vs. Nix</h2>
				<p>
					Nix is the closest conceptual relative. Both use content-addressable storage, atomic
					transactions, and declarative system configuration. The difference is in approach: Nix
					builds its own package universe from scratch with custom derivations. Conary works with
					existing upstream packages -- the same RPMs, DEBs, and Arch packages you already know --
					and converts them transparently via Remi, the conversion proxy. This means Conary gets
					access to 68,000+ packages on day one without requiring maintainers to write Nix
					expressions.
				</p>
				<p>
					Trade-off: Nix's functional model enables reproducible builds at the cost of a large
					closure size and learning curve. Conary prioritizes pragmatism, smaller disk footprint
					through file-level CAS, and a familiar CLI.
				</p>
			</div>
		</div>

		<div class="details" style="margin-top: 1.25rem;">
			<div class="detail-card animate-in" style="--stagger: 9">
				<h2>vs. NixOS Generations</h2>
				<p>
					Both Conary and NixOS support immutable system generations, but the
					approach differs. NixOS builds generations from Nix expressions in a
					custom functional language. Conary builds generations from the actual
					installed package set -- EROFS images verified by composefs and backed
					by content-addressable storage. Conary generations work with existing
					RPM/DEB/Arch packages; NixOS requires packages to be rewritten as Nix
					derivations.
				</p>
			</div>

			<div class="detail-card animate-in" style="--stagger: 10">
				<h2>vs. Flatpak / Snap</h2>
				<p>
					Flatpak and Snap solve cross-distribution distribution for desktop applications
					using containerized runtimes. Conary is a system package manager -- it manages
					the base OS packages, libraries, and services that Flatpak apps depend on.
					They are complementary, not competing.
				</p>
			</div>

			<div class="detail-card animate-in" style="--stagger: 11">
				<h2>Where Conary Is Still Early</h2>
				<p>
					Conary is a 0.3.0 release. apt, dnf, and pacman have decades of battle-testing,
					ecosystem integration, and institutional trust. Nix has a large and active community
					building custom packages.
				</p>
				<p>
					Conary bridges the package gap through Remi (on-demand conversion of upstream
					packages), but native CCS packages are still few. The community is small.
					We believe the architecture is sound -- but we are asking for early adopters
					and contributors, not production deployments.
				</p>
			</div>
		</div>

		<!-- CTA -->
		<div class="compare-cta animate-in" style="--stagger: 12">
			<p>Ready to try it?</p>
			<a href="/install" class="btn btn-primary">Install Conary</a>
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

	.compare-section {
		padding: 2rem 0 5rem;
	}

	.compare-block {
		max-width: 820px;
		margin: 0 auto 3rem;
	}

	.compare-table-wrap { overflow-x: auto; }

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

	.legend {
		display: flex;
		gap: 1.5rem;
		justify-content: center;
		margin-top: 1rem;
		font-size: 0.75rem;
		color: var(--color-text-muted);
	}

	.legend-item {
		display: flex;
		align-items: center;
		gap: 0.375rem;
	}

	.details {
		max-width: 740px;
		margin: 0 auto;
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}

	.detail-card {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		padding: 1.75rem;
	}

	.detail-card h2 {
		font-family: var(--font-display);
		font-size: 1.125rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 0.75rem;
	}

	.detail-distro {
		font-size: 0.8125rem;
		color: var(--color-text-muted);
		font-weight: 400;
		font-family: var(--font-body);
	}

	.detail-card p {
		font-size: 0.9375rem;
		color: var(--color-text-secondary);
		line-height: 1.7;
		margin: 0 0 0.75rem;
		font-weight: 300;
	}

	.detail-card p:last-child { margin-bottom: 0; }

	.compare-cta {
		text-align: center;
		margin-top: 3rem;
	}

	.compare-cta p {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 700;
		margin-bottom: 1rem;
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

	@media (max-width: 768px) {
		.page-title { font-size: 2rem; }
		.compare-table { font-size: 0.75rem; }
		.compare-table th, .compare-table td { padding: 0.5rem 0.5rem; }
	}
</style>
