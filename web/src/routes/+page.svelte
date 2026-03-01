<script lang="ts">
	import SearchBar from '$lib/components/SearchBar.svelte';
	import PackageCard from '$lib/components/PackageCard.svelte';
	import { getStatsOverview, getPopularPackages } from '$lib/api';
	import type { StatsOverview, PopularPackage } from '$lib/types';

	let stats: StatsOverview | null = $state(null);
	let popular: PopularPackage[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	const distros = [
		{ id: 'fedora', label: 'Fedora', desc: 'RPM-based, cutting-edge packages' },
		{ id: 'arch', label: 'Arch Linux', desc: 'Rolling release, latest upstream' },
		{ id: 'ubuntu', label: 'Ubuntu', desc: 'DEB-based, stability-focused' }
	];

	$effect(() => {
		loadData();
	});

	async function loadData() {
		try {
			const [statsData, popularData] = await Promise.all([
				getStatsOverview(),
				getPopularPackages(undefined, 12)
			]);
			stats = statsData;
			popular = popularData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			loading = false;
		}
	}

	function formatNumber(n: number): string {
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
		return String(n);
	}
</script>

<svelte:head>
	<title>Conary Package Index</title>
	<meta name="description" content="Browse, search, and discover packages across Linux distributions with Conary Remi." />
</svelte:head>

<section class="hero">
	<div class="container hero-inner">
		<h1>Conary Package Index</h1>
		<p class="hero-subtitle">
			Browse, search, and discover packages across distributions
		</p>
		<SearchBar placeholder="Search packages across all distros..." autofocus />
	</div>
</section>

{#if stats}
	<section class="stats-bar">
		<div class="container stats-inner">
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_packages)}</span>
				<span class="stat-label">Packages</span>
			</div>
			<div class="stat">
				<span class="stat-value">{stats.total_distros}</span>
				<span class="stat-label">Distributions</span>
			</div>
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_downloads)}</span>
				<span class="stat-label">Downloads</span>
			</div>
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_converted)}</span>
				<span class="stat-label">CCS Packages</span>
			</div>
		</div>
	</section>
{/if}

<section class="distros-section">
	<div class="container">
		<h2>Supported Distributions</h2>
		<div class="distro-grid">
			{#each distros as d}
				<a href="/packages/{d.id}" class="distro-card">
					<span class="distro-name">{d.label}</span>
					<span class="distro-desc">{d.desc}</span>
				</a>
			{/each}
		</div>
	</div>
</section>

{#if popular.length > 0}
	<section class="popular-section">
		<div class="container">
			<h2>Popular Packages</h2>
			<div class="package-grid">
				{#each popular as pkg}
					<PackageCard
						name={pkg.name}
						distro={pkg.distro}
						version={pkg.version}
						description={pkg.description ?? ''}
						downloads={pkg.download_count}
						size={pkg.size}
					/>
				{/each}
			</div>
		</div>
	</section>
{/if}

{#if loading}
	<div class="container loading-msg">
		<p>Loading...</p>
	</div>
{/if}

{#if error}
	<div class="container error-msg">
		<p>{error}</p>
	</div>
{/if}

<style>
	.hero {
		padding: 4rem 0 3rem;
		text-align: center;
		background: var(--color-bg-secondary);
		border-bottom: 1px solid var(--color-border);
	}

	.hero-inner {
		display: flex;
		flex-direction: column;
		align-items: center;
	}

	.hero h1 {
		font-size: 2.5rem;
		font-weight: 700;
		letter-spacing: -0.03em;
		margin-bottom: 0.5rem;
	}

	.hero-subtitle {
		font-size: 1.125rem;
		color: var(--color-text-secondary);
		margin: 0 0 2rem;
	}

	.stats-bar {
		padding: 1.5rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.stats-inner {
		display: flex;
		justify-content: center;
		gap: 3rem;
	}

	.stat {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.125rem;
	}

	.stat-value {
		font-size: 1.5rem;
		font-weight: 700;
		font-family: var(--font-mono);
		color: var(--color-primary);
	}

	.stat-label {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.distros-section,
	.popular-section {
		padding: 3rem 0;
	}

	.distros-section h2,
	.popular-section h2 {
		font-size: 1.375rem;
		margin-bottom: 1.5rem;
	}

	.distro-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
		gap: 1rem;
	}

	.distro-card {
		display: flex;
		flex-direction: column;
		padding: 1.25rem;
		background: var(--color-card-bg);
		border: 1px solid var(--color-card-border);
		border-radius: var(--radius-md);
		text-decoration: none;
		color: var(--color-text);
		transition: border-color 0.15s, box-shadow 0.15s;
	}

	.distro-card:hover {
		border-color: var(--color-primary);
		box-shadow: var(--shadow-md);
		text-decoration: none;
	}

	.distro-name {
		font-weight: 600;
		font-size: 1.0625rem;
		margin-bottom: 0.25rem;
	}

	.distro-desc {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.package-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 0.75rem;
	}

	.loading-msg,
	.error-msg {
		text-align: center;
		padding: 3rem 0;
	}

	.error-msg {
		color: var(--color-danger);
	}

	@media (max-width: 640px) {
		.hero h1 {
			font-size: 1.75rem;
		}

		.stats-inner {
			gap: 1.5rem;
			flex-wrap: wrap;
		}

		.stat-value {
			font-size: 1.25rem;
		}
	}
</style>
