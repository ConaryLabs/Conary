<script lang="ts">
	import PageMeta from '$lib/components/PageMeta.svelte';
	import DistroSelector from '$lib/components/DistroSelector.svelte';
	import PackageCard from '$lib/components/PackageCard.svelte';
	import { getPopularPackages } from '$lib/api';
	import { goto } from '$app/navigation';
	import type { PopularPackage } from '$lib/types';

	let packages: PopularPackage[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	$effect(() => {
		loadPackages();
	});

	async function loadPackages() {
		try {
			packages = await getPopularPackages(undefined, 50);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load packages';
		} finally {
			loading = false;
		}
	}

	function handleDistroSelect(distro: string) {
		if (distro === 'all') return;
		goto(`/packages/${distro}`);
	}
</script>

<PageMeta
	title="Browse packages — Conary"
	description="Browse public package metadata across the Fedora, Arch Linux, and Ubuntu indexes served by Remi."
	path="/packages"
/>

<div class="container page">
	<div class="page-header">
		<p class="eyebrow">Public package map</p>
		<h1>Browse Packages</h1>
		<p class="page-subtitle">Choose a source distribution or inspect the packages with the most observed demand.</p>
	</div>

	<DistroSelector selected="all" onselect={handleDistroSelect} />

	{#if loading}
		<p class="status-msg">Loading packages...</p>
	{:else if error}
		<p class="status-msg error">{error}</p>
	{:else}
		<div class="section">
			<h2>Popular Across All Distros</h2>
			<div class="package-grid">
				{#each packages as pkg}
					<PackageCard
						name={pkg.name}
						distro={pkg.distro}
						downloads={pkg.download_count}
					/>
				{/each}
			</div>
		</div>
	{/if}
</div>

<style>
	.page {
		padding-block: clamp(3rem, 7vw, 5.5rem);
	}

	.page-header {
		margin-bottom: 1.5rem;
	}

	.page-header h1 {
		font-size: clamp(2.6rem, 6vw, 4.8rem);
		font-weight: 700;
		letter-spacing: -0.055em;
		margin-bottom: 0.65rem;
	}

	.page-subtitle {
		color: var(--color-text-secondary);
		margin: 0;
	}

	.section {
		margin-top: 2.5rem;
	}

	.section h2 {
		font-size: 1.35rem;
		font-weight: 700;
		margin-bottom: 1rem;
	}

	.package-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 1px;
		padding: 1px;
		background: var(--color-border);
	}

	.status-msg {
		text-align: center;
		padding: 3rem 0;
		color: var(--color-text-secondary);
	}

	.status-msg.error {
		color: var(--color-danger);
	}
</style>
