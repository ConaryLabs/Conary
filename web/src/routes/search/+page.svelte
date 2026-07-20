<script lang="ts">
	import { page } from '$app/state';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import SearchBar from '$lib/components/SearchBar.svelte';
	import PackageCard from '$lib/components/PackageCard.svelte';
	import DistroSelector from '$lib/components/DistroSelector.svelte';
	import { searchPackages } from '$lib/api';
	import type { SearchResult } from '$lib/types';

	let results: SearchResult[] = $state([]);
	let total = $state(0);
	let loading = $state(false);
	let error: string | null = $state(null);
	let distroFilter = $state('all');

	let query = $derived(page.url.searchParams.get('q') ?? '');

	$effect(() => {
		if (query) {
			performSearch();
		}
	});

	async function performSearch() {
		loading = true;
		error = null;
		try {
			const distro = distroFilter === 'all' ? undefined : distroFilter;
			const resp = await searchPackages(query, distro, 50);
			results = resp.results;
			total = resp.total;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Search failed';
		} finally {
			loading = false;
		}
	}

	function handleDistroSelect(d: string) {
		distroFilter = d;
		if (query) performSearch();
	}
</script>

<PageMeta
	title={query ? `“${query}” package search — Conary` : 'Search packages — Conary'}
	description="Search public package metadata across the distribution indexes served by Remi."
	noindex
/>

<div class="container page">
	<div class="page-header">
		<p class="eyebrow">Package lookup</p>
		<h1>Search Packages</h1>
		<p class="page-subtitle">Search names and descriptions, then narrow the evidence by source distribution.</p>
	</div>

	<div class="search-controls">
		<SearchBar value={query} placeholder="Search packages..." />
		<DistroSelector selected={distroFilter} onselect={handleDistroSelect} />
	</div>

	{#if loading}
		<p class="status-msg">Searching...</p>
	{:else if error}
		<p class="status-msg error">{error}</p>
	{:else if query && results.length === 0}
		<p class="status-msg">No packages found for "{query}".</p>
	{:else if results.length > 0}
		<p class="result-count">{total} result{total !== 1 ? 's' : ''} for "{query}"</p>
		<div class="results-list">
			{#each results as r}
				<PackageCard
					name={r.name}
					distro={r.distro}
					version={r.version}
					description={r.description ?? ''}
					size={r.size}
					converted={r.converted}
				/>
			{/each}
		</div>
	{:else if !query}
		<p class="status-msg">Enter a search term to find packages.</p>
	{/if}
</div>

<style>
	.page {
		padding-block: clamp(3rem, 7vw, 5.5rem);
	}

	.page-header h1 {
		font-size: clamp(2.6rem, 6vw, 4.8rem);
		font-weight: 700;
		letter-spacing: -0.055em;
		margin-bottom: 0.65rem;
	}

	.search-controls {
		display: flex;
		flex-direction: column;
		gap: 1rem;
		margin-bottom: 2rem;
	}

	.result-count {
		font-size: 0.8125rem;
		color: var(--color-text-muted);
		margin-bottom: 1rem;
		font-family: var(--font-mono);
	}

	.results-list {
		display: grid;
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
