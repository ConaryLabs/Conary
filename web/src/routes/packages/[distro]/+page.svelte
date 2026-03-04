<script lang="ts">
	import { page } from '$app/state';
	import PackageCard from '$lib/components/PackageCard.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import SearchBar from '$lib/components/SearchBar.svelte';
	import { listPackages } from '$lib/api';

	const PER_PAGE = 50;

	let packages: string[] = $state([]);
	let totalPackages = $state(0);
	let currentPage = $state(1);
	let loading = $state(true);
	let error: string | null = $state(null);

	let distro = $derived(page.params.distro ?? '');
	let totalPages = $derived(Math.ceil(totalPackages / PER_PAGE));

	$effect(() => {
		currentPage = 1;
		loadPage();
	});

	async function loadPage() {
		loading = true;
		error = null;
		try {
			const resp = await listPackages(distro, currentPage, PER_PAGE);
			packages = resp.packages;
			totalPackages = resp.total;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load packages';
		} finally {
			loading = false;
		}
	}

	function handleNavigate(p: number) {
		currentPage = p;
		loadPage();
		window.scrollTo({ top: 0, behavior: 'smooth' });
	}

	function distroLabel(id: string): string {
		const labels: Record<string, string> = {
			fedora: 'Fedora',
			arch: 'Arch Linux',
			ubuntu: 'Ubuntu'
		};
		return labels[id] ?? id;
	}
</script>

<svelte:head>
	<title>{distroLabel(distro)} Packages - Conary</title>
</svelte:head>

<div class="container page">
	<div class="page-header">
		<div class="title-row">
			<span class="distro-indicator distro-{distro}" aria-hidden="true"></span>
			<h1>{distroLabel(distro)} Packages</h1>
		</div>
		{#if totalPackages > 0}
			<p class="page-subtitle">{totalPackages.toLocaleString()} packages available</p>
		{/if}
	</div>

	<div class="toolbar">
		<SearchBar placeholder="Search {distroLabel(distro)} packages..." />
	</div>

	{#if loading}
		<p class="status-msg">Loading packages...</p>
	{:else if error}
		<p class="status-msg error">{error}</p>
	{:else if packages.length === 0}
		<p class="status-msg">No packages found for {distroLabel(distro)}.</p>
	{:else}
		<div class="package-list">
			{#each packages as pkg}
				<PackageCard
					name={pkg}
					{distro}
				/>
			{/each}
		</div>

		<Pagination
			page={currentPage}
			{totalPages}
			onnavigate={handleNavigate}
		/>
	{/if}
</div>

<style>
	.page {
		padding: 2.5rem 1.5rem;
	}

	.page-header {
		margin-bottom: 1.5rem;
	}

	.title-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}

	.distro-indicator {
		width: 4px;
		height: 1.75rem;
		border-radius: 2px;
	}

	.distro-indicator.distro-fedora { background: var(--color-fedora); }
	.distro-indicator.distro-arch { background: var(--color-arch); }
	.distro-indicator.distro-ubuntu { background: var(--color-ubuntu); }

	.title-row h1 {
		font-family: var(--font-display);
		font-size: 1.75rem;
		font-weight: 700;
		margin-bottom: 0;
	}

	.page-subtitle {
		color: var(--color-text-secondary);
		margin: 0.25rem 0 0;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
	}

	.toolbar {
		margin-bottom: 2rem;
	}

	.package-list {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 0.75rem;
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
