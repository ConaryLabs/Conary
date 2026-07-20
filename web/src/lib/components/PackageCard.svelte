<script lang="ts">
	let {
		name,
		distro,
		version = '',
		description = '',
		downloads = 0,
		converted = false,
		size = 0
	}: {
		name: string;
		distro: string;
		version?: string;
		description?: string;
		downloads?: number;
		converted?: boolean;
		size?: number;
	} = $props();

	function formatSize(bytes: number): string {
		if (bytes === 0) return '';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	function formatDownloads(count: number): string {
		if (count === 0) return '';
		if (count < 1000) return String(count);
		if (count < 1_000_000) return `${(count / 1000).toFixed(1)}k`;
		return `${(count / 1_000_000).toFixed(1)}M`;
	}
</script>

<a href="/packages/{distro}/{name}" class="package-card">
	<div class="card-header">
		<span class="card-name">{name}</span>
		<span class="card-arrow" aria-hidden="true">↗</span>
	</div>
	{#if version}
		<span class="card-version">{version}</span>
	{/if}
	{#if description}
		<p class="card-description">{description}</p>
	{/if}
	<div class="card-meta">
		<span class="card-distro distro-{distro}">{distro}</span>
		{#if converted}<span class="card-badge">CCS</span>{/if}
		{#if size > 0}<span class="card-stat">{formatSize(size)}</span>{/if}
		{#if downloads > 0}<span class="card-stat">{formatDownloads(downloads)} downloads</span>{/if}
	</div>
</a>

<style>
	.package-card {
		display: flex;
		min-height: 154px;
		flex-direction: column;
		padding: 1.15rem 1.25rem;
		color: var(--color-ivory);
		background: var(--color-layer);
		text-decoration: none;
		transition: color 150ms ease, background 150ms ease;
	}

	.package-card:hover {
		color: var(--color-ivory);
		background: var(--color-surface-hover);
		text-decoration: none;
	}

	.card-header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.75rem;
	}

	.card-name {
		min-width: 0;
		overflow-wrap: anywhere;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.9rem;
		font-weight: 500;
	}

	.card-arrow {
		color: var(--color-orange);
		font-size: 0.82rem;
	}

	.card-version {
		margin-top: 0.15rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.7rem;
	}

	.card-description {
		display: -webkit-box;
		margin: 0.7rem 0 1rem;
		overflow: hidden;
		color: var(--color-mist);
		font-size: 0.8rem;
		line-height: 1.45;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
	}

	.card-meta {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 0.45rem 0.65rem;
		margin-top: auto;
		color: var(--color-muted);
		font-size: 0.68rem;
	}

	.card-distro,
	.card-badge {
		padding: 0.12rem 0.42rem;
		border: 1px solid currentColor;
		border-radius: var(--radius-sm);
		font-family: var(--font-mono);
		font-size: 0.62rem;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.distro-fedora { color: #79abe3; }
	.distro-arch { color: var(--color-cyan); }
	.distro-ubuntu { color: #ff9154; }

	.card-badge {
		color: var(--color-success);
	}

	.card-stat {
		font-family: var(--font-mono);
		font-size: 0.66rem;
	}
</style>
