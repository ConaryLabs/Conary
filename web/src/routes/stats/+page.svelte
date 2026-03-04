<script lang="ts">
	import DistroSelector from '$lib/components/DistroSelector.svelte';
	import { getStatsOverview, getPopularPackages, getRecentPackages } from '$lib/api';
	import type { StatsOverview, PopularPackage, RecentPackage } from '$lib/types';

	let stats: StatsOverview | null = $state(null);
	let popular: PopularPackage[] = $state([]);
	let recent: RecentPackage[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let distroFilter = $state('all');

	$effect(() => {
		loadStats();
	});

	async function loadStats() {
		loading = true;
		error = null;
		try {
			const distro = distroFilter === 'all' ? undefined : distroFilter;
			const [s, p, r] = await Promise.all([
				getStatsOverview(),
				getPopularPackages(distro, 25),
				getRecentPackages(distro, 25)
			]);
			stats = s;
			popular = p;
			recent = r;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load stats';
		} finally {
			loading = false;
		}
	}

	function handleDistroSelect(d: string) {
		distroFilter = d;
		loadStats();
	}

	function formatNumber(n: number): string {
		return n.toLocaleString();
	}

	function formatSize(bytes: number): string {
		if (bytes === 0) return '';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}
</script>

<svelte:head>
	<title>Statistics - Conary</title>
</svelte:head>

<div class="container page">
	<div class="page-header">
		<h1>Statistics</h1>
	</div>

	{#if stats}
		<div class="overview-grid animate-in" style="--stagger: 0">
			<div class="overview-card">
				<span class="overview-value">{formatNumber(stats.total_packages)}</span>
				<span class="overview-label">Total Packages</span>
			</div>
			<div class="overview-card">
				<span class="overview-value">{formatNumber(stats.total_downloads)}</span>
				<span class="overview-label">Total Downloads</span>
			</div>
			<div class="overview-card">
				<span class="overview-value">{formatNumber(stats.downloads_30d)}</span>
				<span class="overview-label">Downloads (30d)</span>
			</div>
			<div class="overview-card">
				<span class="overview-value">{stats.total_distros}</span>
				<span class="overview-label">Distributions</span>
			</div>
			<div class="overview-card">
				<span class="overview-value">{formatNumber(stats.total_converted)}</span>
				<span class="overview-label">CCS Packages</span>
			</div>
		</div>
	{/if}

	<div class="filter-bar">
		<DistroSelector selected={distroFilter} onselect={handleDistroSelect} />
	</div>

	{#if loading}
		<p class="status-msg">Loading statistics...</p>
	{:else if error}
		<p class="status-msg error">{error}</p>
	{:else}
		<div class="tables-grid">
			<section class="table-section animate-in" style="--stagger: 2">
				<h2>Most Popular</h2>
				<div class="table-wrap">
					<table>
						<thead>
							<tr>
								<th class="col-rank">#</th>
								<th>Package</th>
								<th>Distro</th>
								<th>Version</th>
								<th class="col-right">Downloads</th>
							</tr>
						</thead>
						<tbody>
							{#each popular as pkg, i}
								<tr>
									<td class="rank">{i + 1}</td>
									<td>
										<a href="/packages/{pkg.distro}/{pkg.name}">
											{pkg.name}
										</a>
									</td>
									<td class="distro">{pkg.distro}</td>
									<td><code>{pkg.version}</code></td>
									<td class="number">{formatNumber(pkg.download_count)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</section>

			<section class="table-section animate-in" style="--stagger: 4">
				<h2>Recently Updated</h2>
				<div class="table-wrap">
					<table>
						<thead>
							<tr>
								<th>Package</th>
								<th>Version</th>
								<th>Distro</th>
								<th class="col-right">Size</th>
							</tr>
						</thead>
						<tbody>
							{#each recent as pkg}
								<tr>
									<td>
										<a href="/packages/{pkg.distro}/{pkg.name}">
											{pkg.name}
										</a>
									</td>
									<td><code>{pkg.version}</code></td>
									<td class="distro">{pkg.distro}</td>
									<td class="number">{formatSize(pkg.size)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</section>
		</div>
	{/if}
</div>

<style>
	.page {
		padding: 2.5rem 1.5rem;
	}

	.page-header h1 {
		font-family: var(--font-display);
		font-size: 1.75rem;
		font-weight: 700;
		margin-bottom: 1.5rem;
	}

	.overview-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: 0.75rem;
		margin-bottom: 2rem;
	}

	.overview-card {
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
	}

	.overview-value {
		font-family: var(--font-mono);
		font-size: 1.75rem;
		font-weight: 700;
		color: var(--color-accent);
		text-shadow: 0 0 30px var(--color-accent-glow);
	}

	.overview-label {
		font-size: 0.6875rem;
		color: var(--color-text-muted);
		margin-top: 0.25rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		font-weight: 500;
	}

	.filter-bar {
		margin-bottom: 2rem;
	}

	.tables-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 2rem;
	}

	.table-section h2 {
		font-family: var(--font-display);
		font-size: 1.125rem;
		font-weight: 700;
		margin-bottom: 1rem;
	}

	.table-wrap {
		overflow-x: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.8125rem;
	}

	th {
		text-align: left;
		font-weight: 600;
		padding: 0.625rem 0.5rem;
		border-bottom: 1px solid var(--color-border);
		font-size: 0.6875rem;
		color: var(--color-text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	td {
		padding: 0.5rem;
		border-bottom: 1px solid var(--color-border);
	}

	td.rank {
		color: var(--color-text-muted);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		width: 2rem;
	}

	td.distro {
		text-transform: capitalize;
		color: var(--color-text-secondary);
	}

	td.number {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		text-align: right;
		color: var(--color-text-secondary);
	}

	.col-rank {
		width: 2rem;
	}

	.col-right,
	th:last-child {
		text-align: right;
	}

	.status-msg {
		text-align: center;
		padding: 3rem 0;
		color: var(--color-text-secondary);
	}

	.status-msg.error {
		color: var(--color-danger);
	}

	@media (max-width: 768px) {
		.tables-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
