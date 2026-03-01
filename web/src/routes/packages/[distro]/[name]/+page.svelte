<script lang="ts">
	import { page } from '$app/state';
	import { getPackageDetail, getReverseDependencies } from '$lib/api';
	import type { PackageDetail } from '$lib/types';

	let pkg: PackageDetail | null = $state(null);
	let rdepends: string[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let showAllVersions = $state(false);
	let showAllDeps = $state(false);
	let showAllRdeps = $state(false);

	let distro = $derived(page.params.distro);
	let name = $derived(page.params.name);

	$effect(() => {
		loadPackage();
	});

	async function loadPackage() {
		loading = true;
		error = null;
		try {
			const [detail, rdeps] = await Promise.all([
				getPackageDetail(distro, name),
				getReverseDependencies(distro, name).catch(() => [] as string[])
			]);
			pkg = detail;
			rdepends = rdeps;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load package';
		} finally {
			loading = false;
		}
	}

	function formatSize(bytes: number): string {
		if (bytes === 0) return 'Unknown';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	function formatNumber(n: number): string {
		return n.toLocaleString();
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
	<title>{name} - {distroLabel(distro)} - Conary</title>
	{#if pkg}
		<meta name="description" content="{pkg.description ?? `${pkg.name} package for ${distroLabel(pkg.distro)}`}" />
	{/if}
</svelte:head>

<div class="container page">
	{#if loading}
		<p class="status-msg">Loading package details...</p>
	{:else if error}
		<p class="status-msg error">{error}</p>
	{:else if pkg}
		<div class="pkg-header">
			<div class="pkg-title-row">
				<h1>{pkg.name}</h1>
				{#if pkg.converted}
					<span class="badge badge-converted">CCS</span>
				{/if}
			</div>
			<p class="pkg-version">
				<span class="version-label">Latest:</span>
				<code>{pkg.latest_version}</code>
				<span class="pkg-distro">{distroLabel(pkg.distro)}</span>
			</p>
			{#if pkg.description}
				<p class="pkg-description">{pkg.description}</p>
			{/if}
		</div>

		<div class="pkg-grid">
			<div class="pkg-main">
				<!-- Versions -->
				<section class="pkg-section">
					<h2>Versions</h2>
					{#if pkg.versions.length === 0}
						<p class="muted">No version information available.</p>
					{:else}
						<table class="version-table">
							<thead>
								<tr>
									<th>Version</th>
									<th>Architecture</th>
									<th>Size</th>
									<th>Format</th>
								</tr>
							</thead>
							<tbody>
								{#each (showAllVersions ? pkg.versions : pkg.versions.slice(0, 10)) as v}
									<tr>
										<td><code>{v.version}</code></td>
										<td>{v.architecture ?? '-'}</td>
										<td>{formatSize(v.size)}</td>
										<td>
											{#if v.converted}
												<span class="badge badge-converted">CCS</span>
											{:else}
												<span class="badge badge-legacy">Legacy</span>
											{/if}
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
						{#if pkg.versions.length > 10 && !showAllVersions}
							<button class="show-more" onclick={() => showAllVersions = true}>
								Show all {pkg.versions.length} versions
							</button>
						{/if}
					{/if}
				</section>

				<!-- Dependencies -->
				<section class="pkg-section">
					<h2>Dependencies ({pkg.dependencies.length})</h2>
					{#if pkg.dependencies.length === 0}
						<p class="muted">No dependencies.</p>
					{:else}
						<ul class="dep-list">
							{#each (showAllDeps ? pkg.dependencies : pkg.dependencies.slice(0, 20)) as dep}
								<li><code>{dep}</code></li>
							{/each}
						</ul>
						{#if pkg.dependencies.length > 20 && !showAllDeps}
							<button class="show-more" onclick={() => showAllDeps = true}>
								Show all {pkg.dependencies.length} dependencies
							</button>
						{/if}
					{/if}
				</section>

				<!-- Reverse Dependencies -->
				{#if rdepends.length > 0}
					<section class="pkg-section">
						<h2>Reverse Dependencies ({rdepends.length})</h2>
						<ul class="dep-list">
							{#each (showAllRdeps ? rdepends : rdepends.slice(0, 20)) as dep}
								<li>
									<a href="/packages/{distro}/{dep}"><code>{dep}</code></a>
								</li>
							{/each}
						</ul>
						{#if rdepends.length > 20 && !showAllRdeps}
							<button class="show-more" onclick={() => showAllRdeps = true}>
								Show all {rdepends.length} reverse dependencies
							</button>
						{/if}
					</section>
				{/if}
			</div>

			<aside class="pkg-sidebar">
				<div class="sidebar-section">
					<h3>Details</h3>
					<dl class="detail-list">
						<dt>Distribution</dt>
						<dd>{distroLabel(pkg.distro)}</dd>

						<dt>Size</dt>
						<dd>{formatSize(pkg.size_bytes)}</dd>

						{#if pkg.license}
							<dt>License</dt>
							<dd>{pkg.license}</dd>
						{/if}

						<dt>Downloads</dt>
						<dd>{formatNumber(pkg.download_count)}</dd>

						<dt>Downloads (30d)</dt>
						<dd>{formatNumber(pkg.download_count_30d)}</dd>

						<dt>Format</dt>
						<dd>{pkg.converted ? 'CCS (converted)' : 'Legacy'}</dd>
					</dl>
				</div>

				{#if pkg.homepage}
					<div class="sidebar-section">
						<h3>Links</h3>
						<a href={pkg.homepage} target="_blank" rel="noopener noreferrer" class="external-link">
							Homepage
						</a>
					</div>
				{/if}

				<div class="sidebar-section">
					<h3>Install</h3>
					<pre><code>conary install {pkg.name}</code></pre>
				</div>
			</aside>
		</div>
	{/if}
</div>

<style>
	.page {
		padding: 2rem 1.5rem;
	}

	.pkg-header {
		margin-bottom: 2rem;
		padding-bottom: 1.5rem;
		border-bottom: 1px solid var(--color-border);
	}

	.pkg-title-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}

	.pkg-title-row h1 {
		font-size: 2rem;
		margin-bottom: 0;
	}

	.pkg-version {
		margin: 0.5rem 0 0;
		font-size: 0.9375rem;
		color: var(--color-text-secondary);
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.version-label {
		font-weight: 500;
	}

	.pkg-distro {
		padding: 0.1em 0.5em;
		background: var(--color-bg-secondary);
		border-radius: var(--radius-sm);
		font-size: 0.8125rem;
		text-transform: capitalize;
	}

	.pkg-description {
		margin: 0.75rem 0 0;
		font-size: 1.0625rem;
		line-height: 1.6;
	}

	.pkg-grid {
		display: grid;
		grid-template-columns: 1fr 300px;
		gap: 2rem;
		align-items: start;
	}

	.pkg-section {
		margin-bottom: 2rem;
	}

	.pkg-section h2 {
		font-size: 1.25rem;
		margin-bottom: 1rem;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--color-border);
	}

	.version-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.875rem;
	}

	.version-table th {
		text-align: left;
		font-weight: 600;
		padding: 0.625rem 0.75rem;
		border-bottom: 2px solid var(--color-border);
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.version-table td {
		padding: 0.5rem 0.75rem;
		border-bottom: 1px solid var(--color-border);
	}

	.dep-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.375rem;
	}

	.dep-list li {
		padding: 0.25rem 0.625rem;
		background: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		font-size: 0.8125rem;
	}

	.dep-list a {
		text-decoration: none;
	}

	.dep-list a:hover code {
		color: var(--color-primary);
	}

	.show-more {
		margin-top: 0.75rem;
		padding: 0.375rem 0.75rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		background: none;
		color: var(--color-primary);
		font-size: 0.8125rem;
	}

	.show-more:hover {
		background: var(--color-bg-secondary);
	}

	.badge {
		padding: 0.1em 0.5em;
		border-radius: var(--radius-sm);
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.03em;
	}

	.badge-converted {
		background: var(--color-success);
		color: #fff;
	}

	.badge-legacy {
		background: var(--color-secondary);
		color: #fff;
	}

	.pkg-sidebar {
		position: sticky;
		top: 4.5rem;
	}

	.sidebar-section {
		padding: 1.25rem;
		background: var(--color-card-bg);
		border: 1px solid var(--color-card-border);
		border-radius: var(--radius-md);
		margin-bottom: 1rem;
	}

	.sidebar-section h3 {
		font-size: 0.875rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-secondary);
		margin-bottom: 0.75rem;
	}

	.detail-list {
		margin: 0;
		font-size: 0.875rem;
	}

	.detail-list dt {
		font-weight: 500;
		color: var(--color-text-secondary);
		margin-top: 0.5rem;
	}

	.detail-list dt:first-child {
		margin-top: 0;
	}

	.detail-list dd {
		margin: 0.125rem 0 0;
		padding: 0;
	}

	.external-link {
		font-size: 0.875rem;
	}

	.sidebar-section pre {
		margin: 0;
		font-size: 0.8125rem;
	}

	.muted {
		color: var(--color-text-secondary);
		font-size: 0.875rem;
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
		.pkg-grid {
			grid-template-columns: 1fr;
		}

		.pkg-sidebar {
			position: static;
		}
	}
</style>
