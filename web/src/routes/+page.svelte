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
		{ id: 'fedora', label: 'Fedora', desc: 'RPM-based, cutting-edge packages', color: 'fedora' },
		{ id: 'arch', label: 'Arch Linux', desc: 'Rolling release, latest upstream', color: 'arch' },
		{ id: 'ubuntu', label: 'Ubuntu', desc: 'DEB-based, stability-focused', color: 'ubuntu' }
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
	<div class="hero-glow" aria-hidden="true"></div>
	<div class="hero-glow-secondary" aria-hidden="true"></div>
	<div class="container hero-inner">
		<h1 class="animate-in" style="--stagger: 0">Conary Package Index</h1>
		<p class="hero-subtitle animate-in" style="--stagger: 2">
			Browse and discover packages across Linux distributions
		</p>
		<div class="animate-in" style="--stagger: 4">
			<SearchBar placeholder="Search packages across all distros..." autofocus />
		</div>
	</div>
</section>

{#if stats}
	<section class="stats-bar animate-in" style="--stagger: 6">
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
		<h2 class="section-heading">Supported Distributions</h2>
		<div class="distro-grid">
			{#each distros as d, i}
				<a href="/packages/{d.id}" class="distro-card distro-{d.color} animate-in" style="--stagger: {8 + i * 2}">
					<div class="distro-accent" aria-hidden="true"></div>
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
			<h2 class="section-heading">Popular Packages</h2>
			<div class="package-grid">
				{#each popular as pkg, i}
					<div class="animate-in" style="--stagger: {14 + i}">
						<PackageCard
							name={pkg.name}
							distro={pkg.distro}
							version={pkg.version}
							description={pkg.description ?? ''}
							downloads={pkg.download_count}
							size={pkg.size}
						/>
					</div>
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
		position: relative;
		padding: 5rem 0 4rem;
		text-align: center;
		overflow: hidden;
	}

	.hero-glow {
		position: absolute;
		top: -250px;
		left: 50%;
		transform: translateX(-50%);
		width: 900px;
		height: 600px;
		background: radial-gradient(ellipse, rgba(232, 133, 61, 0.06) 0%, transparent 65%);
		pointer-events: none;
	}

	.hero-glow-secondary {
		position: absolute;
		top: -150px;
		left: 25%;
		width: 500px;
		height: 400px;
		background: radial-gradient(circle, rgba(23, 147, 209, 0.03) 0%, transparent 65%);
		pointer-events: none;
	}

	.hero-inner {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: center;
	}

	.hero h1 {
		font-family: var(--font-display);
		font-size: 3.25rem;
		font-weight: 800;
		letter-spacing: -0.04em;
		margin-bottom: 0.75rem;
	}

	.hero-subtitle {
		font-size: 1.1875rem;
		color: var(--color-text-secondary);
		margin: 0 0 2.5rem;
		font-weight: 300;
	}

	.stats-bar {
		padding: 1.75rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.stats-inner {
		display: flex;
		justify-content: center;
		gap: 3.5rem;
	}

	.stat {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.125rem;
	}

	.stat-value {
		font-family: var(--font-mono);
		font-size: 1.625rem;
		font-weight: 700;
		color: var(--color-accent);
		text-shadow: 0 0 30px var(--color-accent-glow);
	}

	.stat-label {
		font-size: 0.75rem;
		color: var(--color-text-muted);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		font-weight: 500;
	}

	.distros-section,
	.popular-section {
		padding: 3.5rem 0;
	}

	.section-heading {
		font-family: var(--font-display);
		font-size: 1.375rem;
		font-weight: 700;
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
		padding: 1.375rem 1.375rem 1.375rem 1.625rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		text-decoration: none;
		color: var(--color-text);
		position: relative;
		overflow: hidden;
		transition: border-color 0.15s, box-shadow 0.15s, transform 0.15s;
	}

	.distro-card:hover {
		border-color: var(--color-border-hover);
		box-shadow: var(--shadow-md);
		transform: translateY(-2px);
		text-decoration: none;
		color: var(--color-text);
	}

	.distro-accent {
		position: absolute;
		left: 0;
		top: 0;
		bottom: 0;
		width: 3px;
		border-radius: 3px 0 0 3px;
		transition: width 0.15s;
	}

	.distro-card:hover .distro-accent {
		width: 4px;
	}

	.distro-fedora .distro-accent { background: var(--color-fedora); }
	.distro-arch .distro-accent { background: var(--color-arch); }
	.distro-ubuntu .distro-accent { background: var(--color-ubuntu); }

	.distro-name {
		font-family: var(--font-display);
		font-weight: 600;
		font-size: 1.0625rem;
		margin-bottom: 0.3rem;
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

	.loading-msg p {
		color: var(--color-text-secondary);
	}

	.error-msg p {
		color: var(--color-danger);
	}

	@media (max-width: 640px) {
		.hero {
			padding: 3rem 0 2.5rem;
		}

		.hero h1 {
			font-size: 2.25rem;
		}

		.hero-subtitle {
			font-size: 1rem;
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
