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

	let distro = $derived(page.params.distro ?? '');
	let name = $derived(page.params.name ?? '');

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
		<div class="pkg-header animate-in" style="--stagger: 0">
			<div class="pkg-title-row">
				<h1>{pkg.name}</h1>
				{#if pkg.converted}
					<span class="badge badge-converted">CCS</span>
				{/if}
			</div>
			<p class="pkg-version">
				<span class="version-label">latest</span>
				<code>{pkg.latest_version}</code>
				<span class="pkg-distro distro-{pkg.distro}">{distroLabel(pkg.distro)}</span>
			</p>
			{#if pkg.description}
				<p class="pkg-description">{pkg.description}</p>
			{/if}
		</div>

		<div class="pkg-grid">
			<div class="pkg-main">
				<!-- Versions -->
				<section class="pkg-section animate-in" style="--stagger: 2">
					<h2>Versions</h2>
					{#if pkg.versions.length === 0}
						<p class="muted">No version information available.</p>
					{:else}
						<div class="table-wrap">
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
											<td class="text-muted">{v.architecture ?? '-'}</td>
											<td class="mono-cell">{formatSize(v.size)}</td>
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
						</div>
						{#if pkg.versions.length > 10 && !showAllVersions}
							<button class="show-more" onclick={() => showAllVersions = true}>
								Show all {pkg.versions.length} versions
							</button>
						{/if}
					{/if}
				</section>

				<!-- Dependencies -->
				<section class="pkg-section animate-in" style="--stagger: 4">
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
					<section class="pkg-section animate-in" style="--stagger: 6">
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

			<aside class="pkg-sidebar animate-in" style="--stagger: 3">
				<div class="sidebar-section">
					<h3>Details</h3>
					<dl class="detail-list">
						<dt>Distribution</dt>
						<dd>{distroLabel(pkg.distro)}</dd>

						<dt>Size</dt>
						<dd class="mono-value">{formatSize(pkg.size_bytes)}</dd>

						{#if pkg.license}
							<dt>License</dt>
							<dd>{pkg.license}</dd>
						{/if}

						<dt>Downloads</dt>
						<dd class="mono-value">{formatNumber(pkg.download_count)}</dd>

						<dt>Downloads (30d)</dt>
						<dd class="mono-value">{formatNumber(pkg.download_count_30d)}</dd>

						<dt>Format</dt>
						<dd>{pkg.converted ? 'CCS (converted)' : 'Legacy'}</dd>
					</dl>
				</div>

				{#if pkg.homepage}
					<div class="sidebar-section">
						<h3>Links</h3>
						<a href={pkg.homepage} target="_blank" rel="noopener noreferrer" class="external-link">
							<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" width="14" height="14">
								<path d="M3.75 2h3.5a.75.75 0 010 1.5h-3.5a.25.25 0 00-.25.25v8.5c0 .138.112.25.25.25h8.5a.25.25 0 00.25-.25v-3.5a.75.75 0 011.5 0v3.5A1.75 1.75 0 0112.25 14h-8.5A1.75 1.75 0 012 12.25v-8.5C2 2.784 2.784 2 3.75 2zm6.854-1h4.146a.25.25 0 01.25.25v4.146a.25.25 0 01-.427.177L13.03 4.03 9.28 7.78a.751.751 0 01-1.042-.018.751.751 0 01-.018-1.042l3.75-3.75-1.543-1.543A.25.25 0 0110.604 1z"/>
							</svg>
							Homepage
						</a>
					</div>
				{/if}

				<div class="sidebar-section">
					<h3>Install</h3>
					<div class="terminal-block">
						<span class="terminal-prompt">$</span>
						<code>conary install {pkg.name}</code>
					</div>
				</div>
			</aside>
		</div>
	{/if}
</div>

<style>
	.page {
		padding: 2.5rem 1.5rem;
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
		font-family: var(--font-display);
		font-size: 2rem;
		font-weight: 800;
		margin-bottom: 0;
	}

	.pkg-version {
		margin: 0.5rem 0 0;
		font-size: 0.875rem;
		color: var(--color-text-secondary);
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.version-label {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.pkg-distro {
		padding: 0.1em 0.5em;
		border-radius: var(--radius-sm);
		font-size: 0.75rem;
		text-transform: capitalize;
		font-weight: 500;
	}

	.distro-fedora { background: rgba(60, 110, 180, 0.15); color: #6B9FE0; }
	.distro-arch { background: rgba(23, 147, 209, 0.15); color: #4DB8E8; }
	.distro-ubuntu { background: rgba(233, 84, 32, 0.15); color: #F08060; }

	.pkg-description {
		margin: 0.75rem 0 0;
		font-size: 1rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
	}

	.pkg-grid {
		display: grid;
		grid-template-columns: 1fr 300px;
		gap: 2rem;
		align-items: start;
	}

	.pkg-section {
		margin-bottom: 2.5rem;
	}

	.pkg-section h2 {
		font-family: var(--font-display);
		font-size: 1.125rem;
		font-weight: 700;
		margin-bottom: 1rem;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--color-border);
	}

	.table-wrap {
		overflow-x: auto;
	}

	.version-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.8125rem;
	}

	.version-table th {
		text-align: left;
		font-weight: 600;
		padding: 0.625rem 0.75rem;
		border-bottom: 1px solid var(--color-border);
		font-size: 0.75rem;
		color: var(--color-text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.version-table td {
		padding: 0.5rem 0.75rem;
		border-bottom: 1px solid var(--color-border);
	}

	.text-muted {
		color: var(--color-text-secondary);
	}

	.mono-cell {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-secondary);
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
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		font-size: 0.8125rem;
	}

	.dep-list li code {
		background: none;
		padding: 0;
		color: var(--color-text-secondary);
		font-size: 0.8125rem;
	}

	.dep-list a {
		text-decoration: none;
	}

	.dep-list a:hover code {
		color: var(--color-accent);
	}

	.show-more {
		margin-top: 0.75rem;
		padding: 0.375rem 0.75rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		background: none;
		color: var(--color-accent);
		font-size: 0.8125rem;
		transition: background 0.15s;
	}

	.show-more:hover {
		background: var(--color-accent-subtle);
	}

	.badge {
		padding: 0.1em 0.4em;
		border-radius: var(--radius-sm);
		font-size: 0.625rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.badge-converted {
		background: rgba(52, 211, 153, 0.15);
		color: var(--color-success);
	}

	.badge-legacy {
		background: var(--color-surface);
		color: var(--color-text-muted);
		border: 1px solid var(--color-border);
	}

	.pkg-sidebar {
		position: sticky;
		top: calc(var(--header-height) + 1rem);
	}

	.sidebar-section {
		padding: 1.25rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		margin-bottom: 0.75rem;
	}

	.sidebar-section h3 {
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--color-text-muted);
		margin-bottom: 0.75rem;
	}

	.detail-list {
		margin: 0;
		font-size: 0.8125rem;
	}

	.detail-list dt {
		font-weight: 500;
		color: var(--color-text-muted);
		font-size: 0.75rem;
		margin-top: 0.625rem;
	}

	.detail-list dt:first-child {
		margin-top: 0;
	}

	.detail-list dd {
		margin: 0.125rem 0 0;
		padding: 0;
		color: var(--color-text);
	}

	.mono-value {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
	}

	.external-link {
		font-size: 0.8125rem;
		display: inline-flex;
		align-items: center;
		gap: 0.375rem;
	}

	.terminal-block {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		padding: 0.625rem 0.875rem;
	}

	.terminal-prompt {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		color: var(--color-accent);
		font-weight: 500;
		user-select: none;
	}

	.terminal-block code {
		background: none;
		padding: 0;
		color: var(--color-text);
		font-size: 0.8125rem;
	}

	.muted {
		color: var(--color-text-secondary);
		font-size: 0.8125rem;
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
